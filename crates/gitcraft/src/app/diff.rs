//! A file's changes, opened over the graph: the Hunk view with staging per hunk and per line,
//! the Inline and Split comparisons of the whole file, the File view and the editor that saves
//! the working-folder copy, and before / after pictures.

use super::widgets::Tip;
use super::App;
use super::ctxmenu::Later;
use super::highlight::{self, Runs};
use super::icons::{self, Icon};
use super::repo::{DiffTarget, Loaded, RepoTab};
use super::textdiff::{self, Comparison, Hunk, RowKind};
use super::theme::{ACCENT, BAD, GOOD, p};
use super::widgets::{self, Kind, REMOVED};
use egui::{Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use gitcraft_core::actions::Action;
use gitcraft_core::files::{FileVersions, PatchOp};
use gitcraft_core::git::{DiffLine, DiffLineKind, DiffSource, WORKDIR};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};

mod clipboard;
mod cursor_history;
mod editing;
mod editor_view;
mod find;
mod folding;
mod preview_selection;
mod split_view;
pub(super) mod unicode_highlight;

pub use editor_view::EditorPrefs;
use editor_view::{FileEditor, edit_comparison, editor_line_height, file_editor, split_ratio};
use preview_selection::PreviewSelection;

const LINE: f32 = 19.0;
const HUNK_HEAD: f32 = 28.0;
const GUTTER: f32 = 44.0;

/// A diff read for a tab.
#[derive(Default)]
pub struct DiffCache {
    /// Which read filled this entry: a read for an entry dropped since doesn't fill a new one.
    pub generation: u64,
    /// The editor and its revision the comparison was made of (`None`: the file as read).
    pub compared_edits: Option<(u64, u64)>,
    pub versions: Option<Loaded<FileVersions>>,
    pub lines: Option<Loaded<Vec<DiffLine>>>,
    pub comparison: Option<(bool, Comparison)>,
    pub hl: Option<(bool, Vec<Runs>, Vec<Runs>)>,
    pub(super) unicode: Option<ComparisonUnicode>,
    pub hunk_hl: Option<(bool, HashMap<(usize, usize), Runs>)>,
    /// The single comparison in flight for this entry (whitespace, editor revision, dark).
    /// If typing makes it stale, the next request uses the latest text.
    pub comparing: Option<(bool, Option<(u64, u64)>, bool)>,
    /// The hunk highlighting being worked out (dark).
    pub highlighting: Option<bool>,
    pub images: Option<(Option<Vec<u8>>, Option<Vec<u8>>)>,
}

impl DiffCache {
    fn limited_hunk(&self) -> bool {
        matches!(&self.lines, Some(Loaded::Ready(lines))
            if lines.first().is_some_and(|line| line.kind == DiffLineKind::Limit))
    }
}

/// What the open diff shows: the view, the edit in progress, picked lines.
#[derive(Default)]
pub struct DiffUi {
    pub key: String,
    /// The file this view is of, which a save writes to.
    pub target: Option<DiffTarget>,
    pub file_view: bool,
    /// File View of a Markdown file shows it rendered (Preview) instead of its text (Code).
    pub md_preview: bool,
    pub editing: bool,
    pub draft: Option<String>,
    pub token: Option<String>,
    /// Picked lines: hunk, line indexes, the line a Shift+click range starts from.
    pub picked: Option<(usize, Vec<usize>, usize)>,
    pub jump: Option<f32>,
    pub allow_large: bool,
    pub saving: bool,
    pub find: find::FileFind,
    pub(super) find_original: bool,
    pub(super) json_check: Option<(FileRevision, Option<String>)>,
    pub(super) cursors: editing::Cursors,
    pub(super) folds: folding::Folds,
    pub(super) editor_positions: Option<Arc<folding::Positions>>,
    pub(super) editor_cache: editor_view::EditorCache,
    pub(super) comparison_change: Option<usize>,
    /// Counts edits in the editor, so a save or a comparison knows which text it was of.
    pub edits: u64,
    /// This view of the file (a run-wide id): a save or comparison from a view closed since
    /// changes nothing here.
    pub editor: u64,
}

pub(super) type ComparisonUnicode = [Arc<unicode_highlight::Report>; 2];
type FileRevision = (u64, Option<u64>);
type FileHighlightKey = (FileRevision, bool, bool);
type FileRowsKey = (FileRevision, usize, u32, u64);
const VIRTUAL_PREVIEW_THRESHOLD: usize = 512 * 1024;

// At most one background parse per cache. Results for an earlier revision are never displayed.
struct ParseCache<K, T> {
    ready: Option<(K, Arc<T>)>,
    pending: Option<(K, mpsc::Receiver<T>)>,
}

impl<K, T> Default for ParseCache<K, T> {
    fn default() -> Self {
        Self {
            ready: None,
            pending: None,
        }
    }
}

impl<K: Copy + Eq, T: Send + 'static> ParseCache<K, T> {
    fn get(
        &mut self,
        ctx: &egui::Context,
        key: K,
        file: &str,
        text: &str,
        parse: impl FnOnce(&str, &str) -> T + Send + 'static,
    ) -> Option<Arc<T>> {
        if let Some((requested, pending)) = &self.pending {
            match pending.try_recv() {
                Ok(result) => {
                    if *requested == key {
                        self.ready = Some((key, Arc::new(result)));
                    }
                    self.pending = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => self.pending = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some((_, value)) = self.ready.as_ref().filter(|(cached, _)| *cached == key) {
            return Some(value.clone());
        }
        self.ready = None;
        if text.len() <= VIRTUAL_PREVIEW_THRESHOLD {
            let value = Arc::new(parse(file, text));
            self.ready = Some((key, value.clone()));
            return Some(value);
        }
        if self.pending.is_none() {
            let (send, receive) = mpsc::channel();
            let (file, text, ctx) = (file.to_owned(), text.to_owned(), ctx.clone());
            self.pending = Some((key, receive));
            std::thread::spawn(move || {
                if send.send(parse(&file, &text)).is_ok() {
                    ctx.request_repaint();
                }
            });
        }
        None
    }
}

fn syntax_runs(
    cache: &mut ParseCache<FileHighlightKey, Vec<Runs>>,
    ctx: &egui::Context,
    key: FileHighlightKey,
    file: &str,
    text: &str,
) -> Option<Arc<Vec<Runs>>> {
    let (_, dark, enabled) = key;
    if !enabled {
        cache.ready = None;
        return None;
    }
    cache.get(ctx, key, file, text, move |file, text| highlight::whole(file, text, dark))
}

fn editor_id(editor: u64) -> egui::Id {
    egui::Id::NULL.with(("file_editor", editor))
}

pub fn source_label(src: &DiffSource) -> String {
    let short = |s: &str| s.chars().take(6).collect::<String>();
    match src {
        DiffSource::Unstaged => "Unstaged".into(),
        DiffSource::Staged => "Staged".into(),
        DiffSource::Commit(sha) => short(sha),
        DiffSource::Range(a, b) => format!("{} → {}", short(a), if b == WORKDIR { "Working folder".into() } else { short(b) }),
    }
}

pub fn diff_key(t: &DiffTarget, allow_large: bool) -> String {
    format!("{:?}|{}|{}|{:?}|{}", t.source, t.file, t.untracked, t.parent, allow_large)
}

fn data_url_bytes(url: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let (_, data) = url.split_once(";base64,")?;
    base64::engine::general_purpose::STANDARD.decode(data).ok()
}

impl App {
    pub(super) fn file_editor_shortcut(&self, ctx: &egui::Context, key: egui::Key, mods: egui::Modifiers) -> bool {
        self.repo(self.active).is_some_and(|r| {
            r.ui.diff.is_some() && (r.diff_ui.file_view || r.diff_ui.editing) && editing::shortcut(key, mods)
                && ctx.memory(|memory| memory.has_focus(editor_id(r.diff_ui.editor)) || memory.has_focus(editor_id(r.diff_ui.editor).with("folded")))
        })
    }

    pub(crate) fn open_file_find(&mut self, replace: bool) -> bool {
        let tab = self.active;
        let mode = self.settings.string("diffMode", "hunk");
        let Some(r) = self.repo(tab) else { return false };
        let Some(target) = &r.ui.diff else { return false };
        if r.diff_ui.key != diff_key(target, false) || (replace && !r.diff_ui.editing) {
            return false;
        }
        let key = diff_key(target, r.diff_ui.allow_large);
        let text_file = self.diffs.get(&(tab, key))
            .filter(|entry| r.diff_ui.file_view || mode != "hunk" || entry.limited_hunk())
            .and_then(|entry| entry.versions.as_ref())
            .is_some_and(|versions| matches!(versions, Loaded::Ready(v) if !v.binary && !v.too_large));
        if text_file && let Some(r) = self.repo_mut(tab) {
            if replace {
                r.diff_ui.find.show_replace();
            } else {
                r.diff_ui.find.show();
            }
        }
        text_file
    }

    /// Read the file's versions and diff once per target.
    pub(crate) fn want_diff(&mut self, tab: u64, target: DiffTarget, allow_large: bool) {
        let key = diff_key(&target, allow_large);
        if self.diffs.contains_key(&(tab, key.clone())) {
            return;
        }
        let Some(path) = self.repo(tab).map(|r| r.path.clone()) else { return };
        self.diff_reads += 1;
        let generation = self.diff_reads;
        self.diffs.insert((tab, key.clone()), DiffCache { generation, versions: Some(Loaded::Loading), ..Default::default() });
        self.spawn(move || {
            let dir = std::path::Path::new(&path);
            let versions = gitcraft_core::files::versions_with_limit(dir, &target.source, &target.file, target.parent.as_deref(), target.untracked, allow_large);
            let lines = (!versions.too_large && !versions.binary).then(|| gitcraft_core::git::file_diff_opts(dir, &target.source, &target.file, target.untracked, target.parent.as_deref(), false));
            super::Msg::Diff { tab, key, generation, versions: Box::new(versions), lines }
        });
    }

    /// A finished read: it fills the cache entry it was started for, and nothing once that entry
    /// was dropped (the file changed) or replaced by a newer read.
    pub(crate) fn diff_loaded(&mut self, tab: u64, key: String, generation: u64, versions: FileVersions, lines: Option<Vec<DiffLine>>) {
        let Some(entry) = self.diffs.get_mut(&(tab, key)).filter(|e| e.generation == generation) else { return };
        let images = (versions.old_image.is_some() || versions.new_image.is_some()).then(|| (versions.old_image.as_deref().and_then(data_url_bytes), versions.new_image.as_deref().and_then(data_url_bytes)));
        entry.images = images;
        entry.versions = Some(Loaded::Ready(versions));
        entry.lines = lines.map(Loaded::Ready);
    }

    /// Compare and highlight the two texts off the UI thread. Only the active cache entry
    /// accepts the result, and callers check the completed revision before showing it.
    pub(super) fn compare_later(&mut self, tab: u64, key: String, file: String, old: String, new: String, wanted: (bool, Option<(u64, u64)>, bool)) {
        let Some(e) = self.diffs.get_mut(&(tab, key.clone())).filter(|entry| entry.comparing.is_none()) else { return };
        e.comparing = Some(wanted);
        let generation = e.generation;
        let old_unicode = e.unicode.as_ref().map(|reports| reports[0].clone());
        self.spawn(move || {
            let (ignore_ws, edits, dark) = wanted;
            let cmp = textdiff::compare(&old, &new, ignore_ws);
            let hl_old = highlight::whole(&file, &old, dark);
            let hl_new = highlight::whole(&file, &new, dark);
            let unicode = [
                old_unicode.unwrap_or_else(|| Arc::new(unicode_highlight::Report::scan(&file, &old))),
                Arc::new(if edits.is_none() { unicode_highlight::Report::scan(&file, &new) } else { unicode_highlight::Report::default() }),
            ];
            super::Msg::Apply(Box::new(move |app: &mut App| {
                if let Some(e) = app.diffs.get_mut(&(tab, key)).filter(|e| e.generation == generation && e.comparing == Some(wanted)) {
                    e.comparison = Some((ignore_ws, cmp));
                    e.compared_edits = edits;
                    e.hl = Some((dark, hl_old, hl_new));
                    e.unicode = Some(unicode);
                    e.comparing = None;
                }
            }))
        });
    }

    /// Save the editor's text to the working-folder copy. Returns whether a save was started.
    pub(crate) fn save_draft(&mut self, tab: u64, utf8: bool) -> bool {
        let Some(r) = self.repo_mut(tab) else { return false };
        let (Some(target), Some(draft)) = (r.diff_ui.target.clone(), r.diff_ui.draft.clone()) else { return false };
        if r.diff_ui.saving {
            return false;
        }
        r.diff_ui.saving = true;
        let (path, token, editor, edits) = (r.path.clone(), r.diff_ui.token.clone(), r.diff_ui.editor, r.diff_ui.edits);
        self.spawn(move || {
            let dir = std::path::Path::new(&path);
            // The token is of the bytes this save wrote, for the next save of edits made meanwhile.
            let saved = gitcraft_core::files::save_checked_token(dir, &target.file, &draft, token.as_deref(), utf8);
            let (token, result) = match saved {
                Ok(token) => (Some(token), Ok(())),
                Err(error) => (None, Err(error)),
            };
            super::Msg::Saved { tab, file: target.file, editor, edits, token, result }
        });
        true
    }

    /// A save finished. It ends editing only if the same editor still shows that file with the
    /// text that was saved; edits made meanwhile stay, to be saved over what was written. The
    /// repository is read again either way.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn saved(&mut self, tab: u64, file: String, editor: u64, edits: u64, token: Option<String>, result: Result<(), String>) {
        let same_editor = self.repo(tab).is_some_and(|r| r.diff_ui.editor == editor && r.diff_ui.target.as_ref().is_some_and(|t| t.file == file));
        if same_editor && let Some(r) = self.repo_mut(tab) {
            r.diff_ui.saving = false;
        }
        match result {
            Ok(()) => {
                let mut all_saved = false;
                if same_editor && let Some(r) = self.repo_mut(tab) {
                    if r.diff_ui.edits == edits {
                        r.diff_ui.draft = None;
                        r.diff_ui.editing = false;
                        all_saved = true;
                    } else if token.is_some() {
                        r.diff_ui.token = token;
                    }
                }
                let name = file.rsplit('/').next().unwrap_or(&file).to_owned();
                self.toast(true, format!("Saved {name}"));
                self.diff_changed(tab);
                self.load_repo(tab);
                // Edits made while saving are still unsaved: whatever waited for the save stays put.
                self.left_saved(tab, super::guard::Editor::File, all_saved);
            }
            Err(e) if e.contains("ENCODING_UNREPRESENTABLE") || e.contains("ENCODING_DECODE_LOSS") => {
                let held = self.hold_for_conversion(tab, super::guard::Editor::File);
                self.dialog = Some(super::dialogs::Dialog::confirm_with(
                    "Save this file as UTF-8?",
                    "The current encoding cannot represent every character. The original file is unchanged. Convert this file to UTF-8 with a byte-order mark to preserve your text?",
                    "Save as UTF-8",
                    false,
                    move |app, _| {
                        app.resume_after_conversion(&held);
                        app.save_draft(tab, true);
                    },
                ));
            }
            Err(e) => {
                self.left_saved(tab, super::guard::Editor::File, false);
                self.toast(false, e);
            }
        }
    }

    pub(crate) fn diff_view(&self, ui: &mut Ui, tab: u64, r: &mut RepoTab, target: &DiffTarget, later: &mut Vec<Later>) {
        let c = p();
        let key0 = diff_key(target, false);
        if r.diff_ui.key != key0 {
            // Unsaved text of another file waits for the unsaved-changes prompt (`guard`).
            if super::guard::file_dirty(&r.diff_ui) {
                return;
            }
            r.diff_ui = DiffUi { key: key0, target: Some(target.clone()), editor: super::unique_id(), ..Default::default() };
        }
        let key = diff_key(target, r.diff_ui.allow_large);
        let cache = self.diffs.get(&(tab, key.clone()));
        if cache.is_none() {
            let (t, large) = (target.clone(), r.diff_ui.allow_large);
            later.push(Box::new(move |app| app.want_diff(tab, t, large)));
        }
        let v = match cache.and_then(|c| c.versions.as_ref()) {
            Some(Loaded::Ready(v)) => Some(v),
            _ => None,
        };
        let requested_mode = self.settings.string("diffMode", "hunk");
        let limited_hunk = requested_mode == "hunk" && cache.is_some_and(DiffCache::limited_hunk);
        let mode = if limited_hunk { "inline" } else { requested_mode.as_str() };
        let ignore_ws = self.settings.flag("ignoreWhitespace", false);
        let wrap = self.settings.flag("wordWrap", false);
        let status_list = match target.source {
            DiffSource::Unstaged => Some(&r.view.as_ref().expect("view").data.status.unstaged),
            DiffSource::Staged => Some(&r.view.as_ref().expect("view").data.status.staged),
            _ => None,
        };
        let in_list = status_list.is_some_and(|l| l.iter().any(|f| f.path == target.file));
        let can_stage = target.source == DiffSource::Unstaged && in_list;
        let can_unstage = target.source == DiffSource::Staged && in_list;
        let status = status_list.and_then(|l| l.iter().find(|f| f.path == target.file)).map(|f| f.status).unwrap_or(if target.untracked { '?' } else { 'M' });
        let (dir, name) = match target.file.rfind('/') {
            Some(i) => (&target.file[..=i], &target.file[i + 1..]),
            None => ("", target.file.as_str()),
        };
        let lines = match cache.and_then(|c| c.lines.as_ref()) {
            Some(Loaded::Ready(l)) => Some(l),
            _ => None,
        };
        let added = lines.map_or(0, |l| l.iter().filter(|x| x.kind == DiffLineKind::Added).count());
        let removed = lines.map_or(0, |l| l.iter().filter(|x| x.kind == DiffLineKind::Removed).count());
        let lockable = v.is_some_and(|v| v.lockable);
        let lock = r.side.locks.as_ref().and_then(|l| l.as_ref().ok()).and_then(|l| l.iter().find(|x| x.path == target.file).cloned());
        if lockable && r.side.locks.is_none() {
            later.push(Box::new(move |app| app.want_lfs_locks(tab)));
        }
        let can_edit_locked = !lockable || lock.as_ref().is_some_and(|l| l.ours);
        let editable = v.is_some_and(|v| v.editable && !v.binary) && can_edit_locked;
        let w = ui.available_width();
        if v.is_some_and(|v| v.decoding_loss) {
            let (bar, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
            ui.painter().rect_filled(bar, CornerRadius::ZERO, super::theme::WARN.gamma_multiply(0.1));
            ui.painter().text(pos2(bar.left() + 12.0, bar.center().y), Align2::LEFT_CENTER, "Some bytes could not be decoded. Choose the correct repository encoding before editing.", FontId::proportional(12.0), super::theme::WARN);
        }
        // Path bar.
        let (bar, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
        ui.painter().rect_filled(bar, CornerRadius::ZERO, c.toolbar.gamma_multiply(0.6));
        ui.painter().line_segment([bar.left_bottom(), bar.right_bottom()], Stroke::new(1.0, c.black(0.3)));
        widgets::status_mark(ui, Rect::from_min_size(pos2(bar.left() + 12.0, bar.center().y - 7.0), Vec2::splat(14.0)), status);
        let mut right = bar.right() - 8.0;
        let close = Rect::from_min_size(pos2(right - 24.0, bar.center().y - 12.0), Vec2::splat(24.0));
        let xr = ui.interact(close, ui.scope_id().with("diff_close"), Sense::click()).tip("Close (Esc)");
        icons::paint(ui, close, Icon::X, 16.0, if xr.hovered() { c.white(1.0) } else { c.fg2 });
        if xr.clicked() {
            r.ui.diff = None;
        }
        right -= 30.0;
        let mut bar_ui = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(pos2(bar.left() + 32.0, bar.top()), pos2(right, bar.bottom()))).layout(egui::Layout::right_to_left(egui::Align::Center)));
        bar_ui.spacing_mut().item_spacing.x = 8.0;
        if can_unstage && widgets::small(&mut bar_ui, Kind::Danger, "Unstage File").clicked() {
            let f = target.file.clone();
            later.push(Box::new(move |app| app.act_quiet(tab, Action::Unstage { paths: vec![f] }, true)));
        }
        if can_stage && widgets::small(&mut bar_ui, Kind::Success, "Stage File").clicked() {
            let f = target.file.clone();
            later.push(Box::new(move |app| app.act_quiet(tab, Action::Stage { paths: vec![f] }, true)));
        }
        bar_ui.label(egui::RichText::new(v.map_or("UTF-8", |v| v.encoding.as_str())).size(11.0).color(c.fg3));
        // Lines removed and added: from the hunks, or from the whole-file comparison in the
        // Inline and Split views.
        let counts = if mode == "hunk" {
            lines.map(|_| (removed, added))
        } else {
            cache.and_then(|c| c.comparison.as_ref()).filter(|(ws, _)| *ws == ignore_ws).map(|(_, cmp)| (cmp.removed, cmp.added))
        };
        if let Some((removed, added)) = counts.filter(|_| !r.diff_ui.file_view) {
            bar_ui.label(egui::RichText::new(format!("−{removed}")).size(11.5).color(REMOVED));
            bar_ui.label(egui::RichText::new(format!("+{added}")).size(11.5).color(GOOD));
        }
        let rest = bar_ui.available_width();
        let mut job = egui::text::LayoutJob::default();
        job.append(dir, 0.0, egui::TextFormat::simple(FontId::proportional(13.0), c.fg2));
        job.append(name, 0.0, egui::TextFormat::simple(FontId::proportional(13.0), c.white(1.0)));
        job.wrap = egui::text::TextWrapping::truncate_at_width(rest);
        let g = ui.painter().layout_job(job);
        ui.painter().galley(pos2(bar.left() + 34.0, bar.center().y - g.size().y / 2.0), g, c.fg);
        // Tools bar.
        let (tools, _) = ui.allocate_exact_size(vec2(w, 32.0), Sense::hover());
        ui.painter().line_segment([tools.left_bottom(), tools.right_bottom()], Stroke::new(1.0, c.white(0.05)));
        let mut left_ui = ui.new_child(egui::UiBuilder::new().max_rect(tools.shrink2(vec2(8.0, 0.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
        left_ui.spacing_mut().item_spacing.x = 6.0;
        if lockable {
            let text = match (&lock, &r.side.locks) {
                (Some(l), _) if l.ours => "LFS locked by you".to_owned(),
                (Some(l), _) => format!("LFS locked by {}", if l.owner.is_empty() { "another user" } else { &l.owner }),
                (None, Some(Err(_))) => "Could not verify LFS lock".into(),
                (None, None) => "Checking LFS lock…".into(),
                (None, Some(Ok(_))) => "LFS read-only".into(),
            };
            left_ui.label(egui::RichText::new(text).size(11.0).color(c.fg3));
            if lock.is_none() && matches!(r.side.locks, Some(Ok(_))) && widgets::small(&mut left_ui, Kind::Default, "Lock file").clicked() {
                let f = target.file.clone();
                later.push(Box::new(move |app| app.act(tab, Action::LfsLock { path: f })));
            }
        }
        if editable && !r.diff_ui.editing {
            let label = if target.source == DiffSource::Staged { "Edit in Working Directory" } else { "Edit This File" };
            if widgets::button(&mut left_ui, Kind::Primary, Some(Icon::Pencil), label, 22.0, true).tip("Edit the working-folder copy of this file").clicked() {
                r.diff_ui.token = v.and_then(|v| v.new_token.clone());
                r.diff_ui.editor = super::unique_id();
                r.diff_ui.cursors.clear();
                r.diff_ui.folds.unfold_all();
                r.diff_ui.editor_positions = None;
                r.diff_ui.find_original = false;
                r.diff_ui.find.source_changed();
                r.diff_ui.editing = true;
                r.diff_ui.file_view = true;
            }
        }
        if r.diff_ui.editing {
            if widgets::button(&mut left_ui, Kind::Success, Some(Icon::Save), "Save Changes", 22.0, r.diff_ui.draft.is_some()).tip("Save (Ctrl+S)").clicked() {
                later.push(Box::new(move |app| {
                    app.save_draft(tab, false);
                }));
            }
            if widgets::button(&mut left_ui, Kind::Default, None, "Cancel", 22.0, true).clicked() {
                if r.diff_ui.draft.is_none() {
                    r.diff_ui.editing = false;
                } else {
                    later.push(Box::new(move |app| {
                        app.ask_unsaved(vec![tab], true, Box::new(|_| {}));
                    }));
                }
            }
        }
        // The source and File / Diff switch, centred.
        let src = source_label(&target.source);
        let sw: f32 = [src.as_str(), "File View", "Diff View"].iter().map(|t| ui.painter().layout_no_wrap(t.to_string(), FontId::proportional(12.0), c.fg).size().x + 20.0).sum();
        // Centred, but clear of the buttons on both sides.
        let left_end = left_ui.min_rect().right() + 12.0;
        let controls_width = |file_view| if file_view { 50.0 } else { 24.0 * 7.0 + 2.0 * 6.0 + 16.0 };
        let right_start = tools.right() - controls_width(r.diff_ui.file_view) - 20.0;
        let sx = (tools.center().x - sw / 2.0).min(right_start - sw).max(left_end);
        let mut x = sx;
        let seg = Rect::from_min_size(pos2(sx, tools.center().y - 10.0), vec2(sw, 20.0));
        ui.painter().rect_stroke(seg, CornerRadius::same(3), Stroke::new(1.0, c.white(0.1)), StrokeKind::Inside);
        for (i, t) in [src.as_str(), "File View", "Diff View"].iter().enumerate() {
            let tw = ui.painter().layout_no_wrap(t.to_string(), FontId::proportional(12.0), c.fg).size().x + 20.0;
            let pr = Rect::from_min_size(pos2(x, seg.top()), vec2(tw, 20.0));
            let on = (i == 1 && r.diff_ui.file_view) || (i == 2 && !r.diff_ui.file_view);
            let fill = if i == 0 { ACCENT.gamma_multiply(0.25) } else if on { c.white(0.15) } else { Color32::TRANSPARENT };
            ui.painter().rect_filled(pr.shrink(1.0), CornerRadius::ZERO, fill);
            let resp = ui.interact(pr, ui.scope_id().with(("fileview_seg", i)), if i == 0 { Sense::hover() } else { Sense::click() });
            ui.painter().text(pr.center(), Align2::CENTER_CENTER, *t, FontId::proportional(12.0), if i == 0 || on || resp.hovered() { c.white(1.0) } else { c.fg2 });
            if resp.clicked() {
                if r.diff_ui.file_view != (i == 1) {
                    r.diff_ui.find.source_changed();
                }
                r.diff_ui.file_view = i == 1;
            }
            x += tw;
        }
        let controls = if tools.right() - seg.right() >= controls_width(r.diff_ui.file_view) + 20.0 {
            tools
        } else {
            let (row, _) = ui.allocate_exact_size(vec2(tools.width(), 26.0), Sense::hover());
            ui.painter().rect_filled(row, CornerRadius::ZERO, c.toolbar);
            row
        };
        let mut tools_right = ui.new_child(egui::UiBuilder::new().max_rect(controls.shrink2(vec2(8.0, 0.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
        tools_right.spacing_mut().item_spacing.x = 2.0;
        let toggle = |ui: &mut Ui, icon: Icon, tip: &str, on: bool| -> bool {
            let (rect, resp) = ui.allocate_exact_size(vec2(24.0, 22.0), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), on, tip));
            if on {
                ui.painter().rect_filled(rect, CornerRadius::same(3), ACCENT.gamma_multiply(0.3));
            } else if resp.hovered() {
                ui.painter().rect_filled(rect, CornerRadius::same(3), c.white(0.1));
            }
            icons::paint(ui, rect, icon, 14.0, if on || resp.hovered() { c.white(1.0) } else { c.fg2 });
            resp.tip(tip).clicked()
        };
        if toggle(&mut tools_right, Icon::WrapText, "Word Wrap", wrap) {
            later.push(Box::new(move |app| app.settings.set("wordWrap", !wrap)));
        }
        let (next, prev) = if r.diff_ui.file_view {
            let markdown = is_markdown(&target.file) && v.is_some_and(|v| !v.binary && !v.too_large) && !r.diff_ui.editing;
            if markdown && toggle(&mut tools_right, Icon::FileText, if r.diff_ui.md_preview { "Show the Markdown code" } else { "Preview the Markdown" }, r.diff_ui.md_preview) {
                r.diff_ui.md_preview = !r.diff_ui.md_preview;
            }
            if v.is_some_and(|v| !v.binary && !v.too_large)
                && toggle(&mut tools_right, Icon::Search, "Find in file", r.diff_ui.find.open)
            {
                r.diff_ui.find.show();
            }
            (false, false)
        } else {
            if toggle(&mut tools_right, Icon::Pilcrow, "Ignore Leading/Trailing Whitespace", ignore_ws) {
                later.push(Box::new(move |app| app.settings.set("ignoreWhitespace", !ignore_ws)));
            }
            tools_right.add_space(8.0);
            for (m, icon, tip) in [("split", Icon::Columns2, "Split View"), ("inline", Icon::TextQuote, "Inline View"), ("hunk", Icon::Rows3, "Hunk View")] {
                if toggle(&mut tools_right, icon, tip, requested_mode == m) {
                    later.push(Box::new(move |app| app.settings.set("diffMode", m)));
                }
            }
            tools_right.add_space(8.0);
            let next = toggle(&mut tools_right, Icon::ArrowDown, "Next change", false);
            let prev = toggle(&mut tools_right, Icon::ArrowUp, "Previous change", false);
            (next, prev)
        };
        // Keys: Esc closes, S / U stage and unstage, Ctrl+S saves.
        let typing = ui.ctx().egui_wants_keyboard_input();
        if !super::dialog_open(ui.ctx()) {
            let (mut esc, s_key, u_key, save) = ui.input(|i| (i.key_pressed(egui::Key::Escape), i.key_pressed(egui::Key::S) && i.modifiers.is_none(), i.key_pressed(egui::Key::U) && i.modifiers.is_none(), i.key_pressed(egui::Key::S) && i.modifiers.command));
            if esc && r.diff_ui.find.open && (r.diff_ui.file_view || mode != "hunk") {
                ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
                r.diff_ui.find.open = false;
                if r.diff_ui.editing {
                    let id = editor_id(r.diff_ui.editor);
                    let id = if r.diff_ui.editor_positions.is_some() { id.with("folded") } else { id };
                    ui.memory_mut(|memory| memory.request_focus(id));
                }
                esc = false;
            }
            if save && r.diff_ui.editing && r.diff_ui.draft.is_some() {
                later.push(Box::new(move |app| {
                    app.save_draft(tab, false);
                }));
            }
            if !typing {
                if esc && !r.diff_ui.editing {
                    r.ui.diff = None;
                }
                if s_key && can_stage {
                    let f = target.file.clone();
                    later.push(Box::new(move |app| app.act_quiet(tab, Action::Stage { paths: vec![f] }, true)));
                }
                if u_key && can_unstage {
                    let f = target.file.clone();
                    later.push(Box::new(move |app| app.act_quiet(tab, Action::Unstage { paths: vec![f] }, true)));
                }
            }
        }
        // Body.
        let note = |ui: &mut Ui, text: &str, error: bool| {
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                ui.add(egui::Label::new(egui::RichText::new(text).size(13.0).color(if error { REMOVED } else { c.fg3 })).wrap());
            });
        };
        let Some(v) = v else {
            note(ui, "Loading…", false);
            return;
        };
        if v.too_large {
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                ui.label(egui::RichText::new(format!("This file exceeds the {} MB preview limit.", (v.preview_limit as f64 / 1048576.0).round())).size(13.0).color(c.fg3));
                if r.diff_ui.allow_large {
                    ui.label(egui::RichText::new("Open it in an external editor to inspect it.").size(13.0).color(c.fg3));
                } else if widgets::link(ui, "Load preview (up to 20 MB)", 13.0).clicked() {
                    r.diff_ui.allow_large = true;
                }
            });
            return;
        }
        if let Some((before, after)) = cache.and_then(|c| c.images.as_ref()) {
            return image_diff(ui, &key, before.as_deref(), after.as_deref());
        }
        if v.binary {
            return note(ui, "This is a binary file, so there is no text to compare.", false);
        }
        let dark = c.dark;
        let editing = r.diff_ui.editing && can_edit_locked;
        let generation = cache.map_or(0, |entry| entry.generation);
        if r.diff_ui.file_view && r.diff_ui.md_preview && !editing && is_markdown(&target.file) {
            let text = v.new.as_deref().or(v.old.as_deref()).unwrap_or("");
            egui::ScrollArea::vertical().id_salt(("md_preview", &key)).auto_shrink([false, false]).show(ui, |ui| {
                egui::Frame::new().inner_margin(egui::Margin::same(16)).show(ui, |ui| {
                    if let Some(link) = super::markdown::show(ui, text, "") {
                        later.push(Box::new(move |app| app.open_url(&link)));
                    }
                });
            });
            return;
        }
        if r.diff_ui.file_view {
            let prefs = self.editor_prefs(ui.ctx());
            let file = FileEditor { file: &target.file, base: v.new.as_deref().or(v.old.as_deref()).unwrap_or(""),
                generation, wrap, dark, prefs: &prefs, editable: editing, changed: &[], removed: &[], old_runs: &[], jump: None, scroll: None, spacers: false };
            file_editor(ui, &mut r.diff_ui, file);
            return;
        }
        if mode != "hunk" {
            if limited_hunk {
                ui.label("This diff exceeded the hunk limit. Showing the full comparison with file-level actions.");
            }
            let Some(cache) = cache else { return };
            // The comparison is of the text shown: the edit in progress (by its revision) or the
            // file as read, so an edit or Cancel compares again.
            let edits = r.diff_ui.draft.is_some().then_some((r.diff_ui.editor, r.diff_ui.edits));
            let comparison = match &cache.comparison {
                Some((ws, cmp)) if *ws == ignore_ws && cache.compared_edits == edits && cache.hl.as_ref().is_some_and(|h| h.0 == dark) => Some(cmp),
                _ => {
                    let wanted = (ignore_ws, edits, dark);
                    if cache.comparing.is_none() {
                        let old = v.old.clone().unwrap_or_default();
                        let new = r.diff_ui.draft.clone().unwrap_or_else(|| v.new.clone().unwrap_or_default());
                        let (t, f) = (key.clone(), target.file.clone());
                        later.push(Box::new(move |app| app.compare_later(tab, t, f, old, new, wanted)));
                    }
                    None
                }
            };
            let empty = (dark, Vec::new(), Vec::new());
            let hl = cache.hl.as_ref().filter(|h| h.0 == dark).unwrap_or(&empty);
            let comparison_id = ui.scope_id().with(("cmp", &key));
            if editing {
                let prefs = self.editor_prefs(ui.ctx());
                let file = FileEditor { file: &target.file, base: v.new.as_deref().unwrap_or(""),
                    generation, wrap, dark, prefs: &prefs, editable: true, changed: &[], removed: &[], old_runs: &[], jump: None, scroll: None, spacers: false };
                let previous = cache.comparison.as_ref().map(|(_, cmp)| cmp);
                return edit_comparison(ui, comparison_id, mode, v.old.as_deref().unwrap_or(""), comparison, previous, hl, cache.unicode.as_ref().map(|reports| reports[0].as_ref()), file, &mut r.diff_ui, (next, prev));
            }
            if let Some(comparison) = comparison {
                let prefs = self.editor_prefs(ui.ctx());
                if r.diff_ui.find.open && v.old.is_some() && v.new.is_some() {
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        let previous = r.diff_ui.find_original;
                        ui.selectable_value(&mut r.diff_ui.find_original, false, "Find in Modified");
                        ui.selectable_value(&mut r.diff_ui.find_original, true, "Find in Original");
                        if previous != r.diff_ui.find_original {
                            r.diff_ui.find.source_changed();
                        }
                    });
                }
                let original = v.new.is_none() || r.diff_ui.find_original && v.old.is_some();
                let text = if original { v.old.as_deref() } else { v.new.as_deref() }.unwrap_or("");
                let reveal = r.diff_ui.find.bar(ui, text, (generation, None));
                let search = r.diff_ui.find.hit().map(|hit| ComparisonSearch { hit, original, reveal });
                let input = ComparisonSource {
                    texts: [v.old.as_deref().unwrap_or(""), v.new.as_deref().unwrap_or("")],
                    unicode: cache.unicode.as_ref(),
                };
                return compare_view(ui, comparison_id, (generation, ignore_ws), comparison, &prefs, mode == "split", wrap, hl, input, next, prev, &mut r.diff_ui.jump, search);
            }
            return note(ui, "Comparing…", false);
        }
        let Some(lines) = lines else { return note(ui, "Loading diff…", false) };
        if lines.is_empty() {
            return note(ui, "No changes to show (the file may be empty, or only its mode changed).", false);
        }
        let actions = (can_stage || can_unstage || matches!(target.source, DiffSource::Commit(_))) && !ignore_ws;
        let hunk_hl = cache.and_then(|c| c.hunk_hl.as_ref()).filter(|h| h.0 == dark).map(|h| &h.1);
        if hunk_hl.is_none() && cache.is_some_and(|c| c.highlighting != Some(dark)) && highlight::syntax_for(&target.file).is_some() {
            let (l, f, k) = (lines.clone(), target.file.clone(), key.clone());
            later.push(Box::new(move |app| {
                let Some(e) = app.diffs.get_mut(&(tab, k.clone())) else { return };
                e.highlighting = Some(dark);
                let generation = e.generation;
                app.spawn(move || {
                    let hl = hunk_highlights(&f, &l, dark);
                    super::Msg::Apply(Box::new(move |app: &mut App| {
                        if let Some(e) = app.diffs.get_mut(&(tab, k)).filter(|e| e.generation == generation && e.highlighting == Some(dark)) {
                            e.hunk_hl = Some((dark, hl));
                            e.highlighting = None;
                        }
                    }))
                });
            }));
        }
        self.hunk_view(ui, tab, r, target, generation, lines, hunk_hl, actions, wrap, next, prev, later);
    }

    #[allow(clippy::too_many_arguments)]
    fn hunk_view(&self, ui: &mut Ui, tab: u64, r: &mut RepoTab, target: &DiffTarget, generation: u64, lines: &[DiffLine], hl: Option<&HashMap<(usize, usize), Runs>>, actions: bool, wrap: bool, next: bool, prev: bool, later: &mut Vec<Later>) {
        let c = p();
        let (pre, hunks) = textdiff::split_hunks(lines);
        let pairs: Vec<HashMap<usize, usize>> = hunks.iter().map(|h| textdiff::replacement_lines(&h.lines)).collect();
        let font = FontId::monospace(12.0);
        let compose_emoji = super::fonts::use_emoji_font(ui.ctx());
        let id = ui.scope_id().with(("hunks", &r.diff_ui.key));
        let wrap_width = wrap.then(|| (ui.available_width() - GUTTER * 2.0 - 30.0).max(1.0));
        let measurement = wrap_width.map(|width| {
            let key = id.with("wrapped_hunk_rows");
            let revision = id.with((generation, width.to_bits(), ui.pixels_per_point().to_bits(), compose_emoji));
            let cached = ui.data_mut(|data| data.get_temp::<(egui::Id, Arc<Vec<Vec<f32>>>)>(key));
            cached.filter(|(old, _)| *old == revision).map(|(_, tops)| tops).unwrap_or_else(|| {
                let tops = Arc::new(hunks.iter().map(|hunk| hunk_row_tops(ui, &hunk.lines, &font, width, compose_emoji)).collect::<Vec<_>>());
                ui.data_mut(|data| data.insert_temp(key, (revision, tops.clone())));
                tops
            })
        });
        // Header, measured logical rows, and the gap to the next hunk.
        let row_bottom = |hi: usize| measurement.as_ref().map_or(hunks[hi].lines.len() as f32 * LINE, |tops| {
            tops[hi].last().copied().unwrap_or(0.0)
        });
        let mut tops: Vec<f32> = Vec::with_capacity(hunks.len());
        let mut y = pre.len() as f32 * 26.0;
        for hi in 0..hunks.len() {
            tops.push(y);
            y += HUNK_HEAD + row_bottom(hi) + 8.0;
        }
        let total = y;
        let scroll_key = id.with("hunk_scroll_position");
        let cur = ui.data_mut(|data| data.get_temp::<f32>(scroll_key)).unwrap_or(0.0);
        let mut scroll = egui::ScrollArea::both().id_salt("hunk_scroll").auto_shrink([false, false]).content_margin(0);
        if next {
            if let Some(t) = tops.iter().find(|t| **t > cur + 4.0) {
                scroll = scroll.vertical_scroll_offset(*t);
            }
        } else if prev && let Some(t) = tops.iter().rev().find(|t| **t < cur - 8.0) {
            scroll = scroll.vertical_scroll_offset(*t);
        }
        let mut run: Option<(PatchOp, usize, Option<Vec<usize>>)> = None;
        let mut stash: Option<(usize, Option<Vec<usize>>)> = None;
        let mut revert: Option<(usize, Option<Vec<usize>>)> = None;
        let mut pick: Option<(usize, usize, bool)> = None;
        let picked = r.diff_ui.picked.clone();
        let longest = lines.iter().map(|l| l.text.len()).max().unwrap_or(0) as f32 * 7.3 + 140.0;
        let output = ui.push_id(id, |ui| {
            scroll.show_viewport(ui, |ui, viewport| {
                let width = if wrap { ui.available_width() } else { longest.max(ui.available_width()) };
                let (_, _) = ui.allocate_exact_size(vec2(width, total.max(viewport.height())), Sense::hover());
                let origin = ui.min_rect().min;
                let painter = ui.painter().clone();
                for (i, l) in pre.iter().enumerate() {
                    painter.text(origin + vec2(12.0, i as f32 * 26.0 + 13.0), Align2::LEFT_CENTER, &l.text, FontId::proportional(12.5), c.fg3);
                }
                for (hi, h) in hunks.iter().enumerate() {
                    let top = tops[hi];
                    let bottom = top + HUNK_HEAD + row_bottom(hi);
                    if bottom < viewport.top() || top > viewport.bottom() {
                        continue;
                    }
                    let mine: Vec<usize> = picked.as_ref().filter(|p| p.0 == hi).map(|p| p.1.clone()).unwrap_or_default();
                    let n = mine.len();
                    let lines_arg = (n > 0).then(|| mine.clone());
                    // The header stays at the top while its hunk scrolls under it.
                    let head_y = top.max(viewport.top().min(bottom - HUNK_HEAD));
                    let head = Rect::from_min_size(origin + vec2(viewport.left(), head_y), vec2(viewport.width(), HUNK_HEAD));
                    // Lines.
                    let body_top = top + HUNK_HEAD;
                    let measured = measurement.as_ref().map(|tops| &tops[hi]);
                    let screen_top = viewport.top() - body_top;
                    let screen_bottom = viewport.bottom() - body_top;
                    let onscreen = if let Some(tops) = measured {
                        visible_rows(tops, screen_top, screen_bottom)
                    } else {
                        let start = ((screen_top / LINE).floor() as usize).min(h.lines.len());
                        let end = ((screen_bottom / LINE).ceil() as usize).min(h.lines.len());
                        start..end.max(start)
                    };
                    let row_top = |li: usize| measured.map_or(li as f32 * LINE, |tops| tops[li]);
                    for li in onscreen {
                        let l = &h.lines[li];
                        let y = row_top(li);
                        let row = Rect::from_min_size(origin + vec2(0.0, body_top + y), vec2(width, row_top(li + 1) - y));
                        let change = matches!(l.kind, DiffLineKind::Added | DiffLineKind::Removed);
                        let on = mine.contains(&li);
                        let bg = match l.kind {
                            DiffLineKind::Added => GOOD.gamma_multiply(0.2),
                            DiffLineKind::Removed => BAD.gamma_multiply(0.2),
                            _ => Color32::TRANSPARENT,
                        };
                        painter.rect_filled(row, CornerRadius::ZERO, bg);
                        let resp = ui.interact(row, ui.scope_id().with(("hl", hi, li)), if change && actions { Sense::click() } else { Sense::hover() });
                        if on {
                            painter.rect(row, CornerRadius::ZERO, ACCENT.gamma_multiply(0.25), Stroke::new(1.0, ACCENT), StrokeKind::Inside);
                        } else if change && actions && resp.hovered() {
                            painter.rect_filled(row, CornerRadius::ZERO, c.white(0.06));
                        }
                        if resp.clicked() {
                            pick = Some((hi, li, ui.input(|i| i.modifiers.shift)));
                        }
                        let num = |n: Option<usize>| n.map(|n| n.to_string()).unwrap_or_default();
                        painter.text(pos2(row.left() + GUTTER - 8.0, row.top() + LINE / 2.0), Align2::RIGHT_CENTER, num(l.old_no), font.clone(), c.fg3);
                        painter.text(pos2(row.left() + GUTTER * 2.0 - 8.0, row.top() + LINE / 2.0), Align2::RIGHT_CENTER, num(l.new_no), font.clone(), c.fg3);
                        painter.line_segment([pos2(row.left() + GUTTER * 2.0, row.top()), pos2(row.left() + GUTTER * 2.0, row.bottom())], Stroke::new(1.0, c.white(0.05)));
                        let sign = match l.kind {
                            DiffLineKind::Added => "+",
                            DiffLineKind::Removed => "-",
                            _ => " ",
                        };
                        painter.text(pos2(row.left() + GUTTER * 2.0 + 8.0, row.top() + LINE / 2.0), Align2::LEFT_CENTER, sign, font.clone(), c.fg3);
                        let marks: Vec<(std::ops::Range<usize>, Color32)> = pairs[hi].get(&li).and_then(|o| textdiff::changed_words(&l.text, &h.lines[*o].text)).map(|m| m.into_iter().map(|r| (r, if l.kind == DiffLineKind::Added { GOOD.gamma_multiply(0.45) } else { BAD.gamma_multiply(0.45) })).collect()).unwrap_or_default();
                        let base = if l.kind == DiffLineKind::Meta { c.fg3 } else { c.white(0.85) };
                        let mut job = highlight::job(&l.text, hl.and_then(|h| h.get(&(hi, li))), base, font.clone(), &marks);
                        layout_hunk_line(&mut job, wrap_width, compose_emoji);
                        let g = ui.painter().layout_job(job);
                        let text_top = if wrap { row.top() } else { row.center().y - g.size().y / 2.0 };
                        painter.with_clip_rect(row.intersect(ui.clip_rect())).galley(
                            pos2(row.left() + GUTTER * 2.0 + 20.0, text_top), g, c.fg,
                        );
                    }
                    // Header (drawn after the lines so it covers them).
                    painter.rect_filled(head, CornerRadius::ZERO, c.panel2);
                    painter.line_segment([head.left_top(), head.right_top()], Stroke::new(1.0, c.white(0.05)));
                    painter.line_segment([head.left_bottom(), head.right_bottom()], Stroke::new(1.0, c.white(0.05)));
                    let mut bar = ui.new_child(egui::UiBuilder::new().max_rect(head.shrink2(vec2(12.0, 0.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
                    bar.spacing_mut().item_spacing.x = 6.0;
                    let plural = |n: usize| if n == 1 { "" } else { "s" };
                    if actions {
                        match &target.source {
                            DiffSource::Unstaged => {
                                let stage = if n > 0 { format!("Stage {n} Line{}", plural(n)) } else { "Stage Hunk".into() };
                                if widgets::button(&mut bar, Kind::Success, None, &stage, 20.0, true).clicked() {
                                    run = Some((PatchOp::Stage, hi, lines_arg.clone()));
                                }
                                let discard = if n > 0 { format!("Discard {n} Line{}", plural(n)) } else { "Discard Hunk".into() };
                                if widgets::button(&mut bar, Kind::Danger, None, &discard, 20.0, true).clicked() {
                                    run = Some((PatchOp::Discard, hi, lines_arg.clone()));
                                }
                                if !target.untracked && widgets::button(&mut bar, Kind::Default, None, if n > 0 { "Stash Selected Lines" } else { "Stash Hunk" }, 20.0, true).clicked() {
                                    stash = Some((hi, lines_arg.clone()));
                                }
                            }
                            DiffSource::Staged => {
                                let unstage = if n > 0 { format!("Unstage {n} Line{}", plural(n)) } else { "Unstage Hunk".into() };
                                if widgets::button(&mut bar, Kind::Danger, None, &unstage, 20.0, true).clicked() {
                                    run = Some((PatchOp::Unstage, hi, lines_arg.clone()));
                                }
                            }
                            DiffSource::Commit(_) => {
                                let label = if n > 0 { format!("Revert {n} Line{}", plural(n)) } else { "Revert Hunk".into() };
                                if widgets::button(&mut bar, Kind::Default, None, &label, 20.0, true).tip("Apply the inverse of this hunk to your working directory").clicked() {
                                    revert = Some((hi, lines_arg.clone()));
                                }
                            }
                            _ => {}
                        }
                    }
                    let room = bar.available_width();
                    let g = widgets::fit(ui, &h.header, FontId::monospace(11.5), c.fg2, room);
                    painter.galley(pos2(head.left() + 12.0, head.center().y - g.size().y / 2.0), g, c.fg2);
                }
            })
        }).inner;
        ui.data_mut(|data| data.insert_temp(scroll_key, output.state.clamped_offset().y));
        // Line picking: a click picks or drops a line, Shift+click a range from the last one.
        if let Some((hi, li, shift)) = pick {
            let h: &Hunk = &hunks[hi];
            r.diff_ui.picked = match r.diff_ui.picked.take() {
                Some((ph, mut ls, anchor)) if ph == hi => {
                    if shift {
                        let (a, b) = (anchor.min(li), anchor.max(li));
                        Some((hi, (a..=b).filter(|k| matches!(h.lines[*k].kind, DiffLineKind::Added | DiffLineKind::Removed)).collect(), anchor))
                    } else {
                        if let Some(pos) = ls.iter().position(|x| *x == li) {
                            ls.remove(pos);
                        } else {
                            ls.push(li);
                        }
                        (!ls.is_empty()).then_some((hi, ls, li))
                    }
                }
                _ => Some((hi, vec![li], li)),
            };
        }
        let expected = lines.to_vec();
        if let Some((op, hunk, picked_lines)) = run {
            r.diff_ui.picked = None;
            let action = Action::Patch { file: target.file.clone(), untracked: target.untracked, op: op.clone(), hunk, lines: picked_lines.clone(), expected };
            if op == PatchOp::Discard {
                let title = match &picked_lines {
                    Some(l) => format!("Discard {} line{}?", l.len(), if l.len() == 1 { "" } else { "s" }),
                    None => "Discard this hunk?".into(),
                };
                later.push(Box::new(move |app| app.confirm_act(tab, &title, "These changes are removed from the working folder. You can bring them back with Undo.", "Discard", action)));
            } else {
                later.push(Box::new(move |app| app.act_quiet(tab, action, true)));
            }
        } else if let Some((hunk, picked_lines)) = stash {
            r.diff_ui.picked = None;
            let action = Action::StashHunk { file: target.file.clone(), hunk, lines: picked_lines, expected };
            later.push(Box::new(move |app| app.act(tab, action)));
        } else if let Some((hunk, picked_lines)) = revert
            && let DiffSource::Commit(sha) = &target.source
        {
            r.diff_ui.picked = None;
            let action = Action::RevertHunk { sha: sha.clone(), parent: target.parent.clone(), file: target.file.clone(), hunk, lines: picked_lines };
            later.push(Box::new(move |app| app.act(tab, action)));
        }
    }
}

/// Colours for the hunk view: old and new lines each carry their own state through a hunk.
fn hunk_highlights(file: &str, lines: &[DiffLine], dark: bool) -> HashMap<(usize, usize), Runs> {
    let mut out = HashMap::new();
    let Some(syntax) = highlight::syntax_for(file) else { return out };
    if lines.len() > highlight::LINE_LIMIT {
        return out;
    }
    let (_, hunks) = textdiff::split_hunks(lines);
    for (hi, h) in hunks.iter().enumerate() {
        let mut old = highlight::Lines::new(syntax, dark);
        let mut new = highlight::Lines::new(syntax, dark);
        for (li, l) in h.lines.iter().enumerate() {
            let runs = match l.kind {
                DiffLineKind::Added => new.line(&l.text),
                DiffLineKind::Removed => old.line(&l.text),
                DiffLineKind::Context => {
                    let _ = old.line(&l.text);
                    new.line(&l.text)
                }
                _ => continue,
            };
            out.insert((hi, li), runs);
        }
    }
    out
}

fn layout_hunk_line(job: &mut egui::text::LayoutJob, wrap_width: Option<f32>, compose_emoji: bool) {
    if let Some(width) = wrap_width {
        job.wrap.max_width = width;
        job.wrap.max_rows = usize::MAX;
        job.wrap.break_anywhere = true;
        job.first_row_min_height = LINE;
        for section in &mut job.sections {
            section.format.line_height = Some(LINE);
        }
    }
    super::fonts::compose_emoji(job, compose_emoji);
}

fn hunk_row_tops(ui: &Ui, lines: &[DiffLine], font: &FontId, width: f32, compose_emoji: bool) -> Vec<f32> {
    let mut tops = Vec::with_capacity(lines.len() + 1);
    tops.push(0.0);
    for line in lines {
        let mut job = egui::text::LayoutJob::simple_singleline(line.text.clone(), font.clone(), Color32::WHITE);
        layout_hunk_line(&mut job, Some(width), compose_emoji);
        let height = ui.fonts_mut(|fonts| fonts.layout_job(job).size().y).max(LINE);
        tops.push(tops.last().copied().unwrap_or(0.0) + height);
    }
    tops
}

#[cfg(test)]
#[test]
fn headless_wrapped_hunks_reserve_height_for_each_logical_diff_line() {
    let ctx = egui::Context::default();
    let lines = ["First line is deliberately wide.".repeat(12), "Second line".into()]
        .map(|text| DiffLine { kind: DiffLineKind::Added, old_no: None, new_no: None, text });
    let mut outcome = ctx.run_ui(egui::RawInput::default(), |ui| {
        let tops = hunk_row_tops(ui, &lines, &FontId::monospace(12.0), 120.0, false);
        assert!(tops[1] > 3.0 * LINE);
        assert_eq!(tops[2] - tops[1], LINE);
        assert_eq!(visible_rows(&tops, tops[1], tops[2]), 1..2);
    });
    outcome.textures_delta.clear();
}

/// How many screen lines `text` takes when lines break after `columns` characters (tabs count
/// as four).
fn wrapped_lines(text: &str, columns: usize) -> usize {
    let width: usize = text.chars().map(|ch| if ch == '\t' { 4 } else { 1 }).sum();
    width.div_ceil(columns.max(1)).max(1)
}

/// Where each row starts, and the total height at the end, for rows of `lines(row)` screen lines.
#[cfg(test)]
fn row_tops(rows: usize, lines: impl Fn(usize) -> usize) -> Vec<f32> {
    sized_row_tops(rows, LINE, lines)
}

fn sized_row_tops(rows: usize, height: f32, mut lines: impl FnMut(usize) -> usize) -> Vec<f32> {
    let mut tops = Vec::with_capacity(rows + 1);
    let mut y = 0.0;
    tops.push(y);
    for row in 0..rows {
        y += lines(row) as f32 * height;
        tops.push(y);
    }
    tops
}

/// The rows between `top` and `bottom`, from the row tops.
fn visible_rows(tops: &[f32], top: f32, bottom: f32) -> std::ops::Range<usize> {
    let rows = tops.len().saturating_sub(1);
    let first = tops.partition_point(|t| *t <= top).saturating_sub(1).min(rows);
    let last = (tops.partition_point(|t| *t < bottom)).min(rows);
    first..last.max(first)
}

/// The monospace character width, for wrapping.
fn char_width(ui: &Ui, font: &FontId) -> f32 {
    ui.fonts_mut(|f| f.glyph_width(font, 'M')).max(1.0)
}

/// The whole file compared, inline (one column) or split (old left, new right). With `wrap`,
/// long lines continue on the next screen lines instead of scrolling sideways.
#[allow(clippy::too_many_arguments)]
pub(super) fn compare_view(ui: &mut Ui, id: egui::Id, revision: (u64, bool), cmp: &Comparison, prefs: &EditorPrefs, split: bool, wrap: bool, hl: &(bool, Vec<Runs>, Vec<Runs>), source: ComparisonSource<'_>, next: bool, prev: bool, jump: &mut Option<f32>, search: Option<ComparisonSearch<'_>>) {
    let c = p();
    let compose_emoji = super::fonts::use_emoji_font(ui.ctx());
    let font = &prefs.font;
    let height = editor_line_height(font);
    let gutter = if prefs.numbers { GUTTER } else { 0.0 };
    let rows = if split { cmp.split.len() } else { cmp.rows.len() };
    let line_text = |old: bool, i: usize| if old { cmp.old.get(i).map(String::as_str).unwrap_or("") } else { cmp.new.get(i).map(String::as_str).unwrap_or("") };
    // Row tops, worked out again only when the width or the comparison changes.
    let cw = char_width(ui, font);
    let unicode = source.unicode;
    if unicode.is_some_and(|reports| reports.iter().any(|report| report.more)) {
        unicode_highlight::caption(ui);
    }
    let body = ui.available_rect_before_wrap();
    let (body, overview) = if split { split_view::reserve_ruler(body) } else { (body, Rect::NOTHING) };
    let full = body.width();
    let ratio = if split { split_ratio(ui, id, body) } else { 0.5 };
    let columns = |width: f32| ((width - gutter - 14.0).max(cw * 10.0) / cw).floor() as usize;
    let full_cols = columns(full);
    let left_cols = columns(full * ratio);
    let right_cols = columns(full * (1.0 - ratio));
    let tops_id = id.with("comparison_tops");
    let geometry = id.with((revision, split, wrap, full_cols, left_cols, right_cols, height.to_bits()));
    let cached = ui.data_mut(|d| d.get_temp::<(egui::Id, Arc<Vec<f32>>)>(tops_id));
    let tops = cached.filter(|(key, _)| *key == geometry).map(|(_, tops)| tops).unwrap_or_else(|| {
        let tops = if !wrap {
            sized_row_tops(rows, height, |_| 1)
        } else if split {
            sized_row_tops(rows, height, |r| {
                let s = &cmp.split[r];
                let l = s.left.map_or(1, |i| wrapped_lines(line_text(true, i), left_cols));
                let rt = s.right.map_or(1, |i| wrapped_lines(line_text(false, i), right_cols));
                l.max(rt)
            })
        } else {
            sized_row_tops(rows, height, |r| {
                let rw = &cmp.rows[r];
                let (no, old) = if rw.kind == RowKind::Removed { (rw.old, true) } else { (rw.new, false) };
                no.map_or(1, |i| wrapped_lines(line_text(old, i), full_cols))
            })
        };
        let tops = std::sync::Arc::new(tops);
        ui.data_mut(|d| d.insert_temp(tops_id, (geometry, tops.clone())));
        tops
    });
    let total = tops.last().copied().unwrap_or(0.0);
    let changes = if split { &cmp.split_changes } else { &cmp.changes };
    let scroll_key = id.with("comparison_scroll");
    let cur = ui.data_mut(|data| data.get_temp::<split_view::Scroll>(scroll_key)).unwrap_or_default().top;
    let overview_key = id.with("overview_jump");
    let overview_jump = ui.data_mut(|data| data.remove_temp::<f32>(overview_key)).filter(|_| split);
    let mut scroll = egui::ScrollArea::both().id_salt("cmp_scroll").auto_shrink([false, false]).content_margin(0);
    let view_h = body.height();
    let scroll_mark = cur + view_h / 3.0;
    let target = if next {
        changes.get(changes.partition_point(|&row| tops[row] <= scroll_mark + 4.0))
    } else if prev {
        changes.partition_point(|&row| tops[row] < scroll_mark - 8.0).checked_sub(1).and_then(|index| changes.get(index))
    } else {
        None
    }.map(|&row| tops[row]);
    let match_top = search.as_ref().filter(|search| search.reveal)
        .and_then(|search| comparison_line(cmp, split, search.original, search.hit.line))
        .and_then(|row| tops.get(row)).copied();
    if let Some(t) = overview_jump.or_else(|| match_top.or(target).or(jump.take()).map(|t| (t - view_h / 3.0).max(0.0))) {
        scroll = scroll.vertical_scroll_offset(t);
    }
    let longest = cmp.longest.map(|width| width as f32 * cw + gutter + 20.0);
    let old_hl = &hl.1;
    let new_hl = &hl.2;
    let mut comparison_ui = ui.new_child(egui::UiBuilder::new().id_salt(id).max_rect(body));
    comparison_ui.set_clip_rect(ui.clip_rect().intersect(body));
    let output = scroll.show_viewport(&mut comparison_ui, |ui, viewport| {
        let full = ui.available_width();
        let pane_widths = [viewport.width() * ratio, viewport.width() * (1.0 - ratio)];
        let extra = [0, 1].map(|side| (longest[side] - pane_widths[side]).max(0.0));
        let width = if wrap { full } else if split { full + extra[0].max(extra[1]) } else { longest[0].max(longest[1]).max(full) };
        let (_, _) = ui.allocate_exact_size(vec2(width, total.max(viewport.height())), Sense::hover());
        let origin = ui.min_rect().min;
        // Horizontal panning moves the text within each pane. Both panes and the divider
        // remain visible even when one version has a very long line.
        let split_x = origin.x + viewport.left();
        let half = viewport.width() * ratio;
        let painter = ui.painter().clone();
        let selection_revision = (revision.0, None);
        let indices = [0, 1].map(|side| cached_file_lines(ui, id.with(("comparison_index", side)), selection_revision, source.texts[side]));
        let clip = ui.clip_rect().intersect(body);
        let divider = split_x + half;
        let left_edge = (divider - 4.0).max(clip.left());
        let left = Rect::from_min_max(pos2((split_x + gutter).min(left_edge), clip.top()), pos2(left_edge, clip.bottom()));
        let right = Rect::from_min_max(pos2((divider + gutter.max(4.0)).min(clip.right()), clip.top()), clip.max);
        let whole = Rect::from_min_max(pos2((clip.left() + gutter).min(clip.right()), clip.top()), clip.max);
        let selection = [0, 1].map(|side| {
            (split || side == 1).then(|| {
                let rect = if split { [left, right][side] } else { whole };
                PreviewSelection::new(ui, id.with(("comparison_selection", split, side)), rect, selection_revision, source.texts, side)
            })
        });
        let [mut original, mut modified] = selection;
        let mut draw = |painter: &egui::Painter, rect: Rect, no: Option<usize>, old: bool, kind: RowKind, pair: Option<(usize, usize)>, same_old: Option<usize>, horizontal: f32| {
            let bg = match kind {
                RowKind::Added => GOOD.gamma_multiply(0.2),
                RowKind::Removed => BAD.gamma_multiply(0.2),
                RowKind::Same => Color32::TRANSPARENT,
            };
            painter.rect_filled(rect, CornerRadius::ZERO, bg);
            let Some(i) = no else {
                painter.rect_filled(rect, CornerRadius::ZERO, c.white(0.02));
                return;
            };
            if prefs.numbers {
                painter.text(pos2(rect.left() + gutter - 8.0, rect.top() + height / 2.0), Align2::RIGHT_CENTER, (i + 1).to_string(), font.clone(), c.fg3);
            }
            let text = line_text(old, i);
            let marks = pair
                .and_then(|(o, n)| if old { textdiff::changed_words(text, line_text(false, n)) } else { textdiff::changed_words(text, line_text(true, o)) })
                .into_iter().flatten()
                .map(|r| (r, if old { BAD.gamma_multiply(0.45) } else { GOOD.gamma_multiply(0.45) }));
            let find = search.as_ref().and_then(|search| {
                if search.original == old {
                    return search.hit.mark(i, text);
                }
                // A row shown from the modified side that matched in the original: the two differ only in
                // whitespace (with Ignore Whitespace), so the match is mapped onto the text shown.
                let line = if search.original { same_old? } else { return None };
                let original = indices[0].line(source.texts[0], line)?;
                if original == text {
                    return search.hit.mark(line, text);
                }
                let (range, color) = search.hit.mark(line, original)?;
                Some((remap_ignoring_whitespace(&range, original, text)?, color))
            });
            let marks: Vec<_> = find.into_iter().chain(marks).collect();
            let runs = if !prefs.syntax { None } else if old { old_hl.get(i) } else { new_hl.get(i) };
            let mut job = highlight::job(text, runs, c.white(0.85), font.clone(), &marks);
            if wrap {
                let available = if !split { full_cols } else if old { left_cols } else { right_cols };
                job.wrap = egui::text::TextWrapping { max_width: available as f32 * cw + 0.5, break_anywhere: true, ..Default::default() };
                job.first_row_min_height = height;
                for section in &mut job.sections {
                    section.format.line_height = Some(height);
                }
            }
            super::fonts::compose_emoji(&mut job, compose_emoji);
            let g = ui.painter().layout_job(job);
            let y = if wrap { rect.top() } else { rect.center().y - g.size().y / 2.0 };
            let text_clip = Rect::from_min_max(pos2((rect.left() + gutter).min(rect.right()), rect.top()), rect.max);
            let painter = painter.with_clip_rect(text_clip.intersect(ui.clip_rect()));
            let pos = pos2(rect.left() + gutter + 8.0 - horizontal, y);
            let side = usize::from(!old);
            let select = if split && old { &mut original } else { &mut modified };
            if let Some(select) = select {
                let (side, line) = if !split && select.prefers_original()
                    && let Some(previous) = same_old
                    && indices[0].line(source.texts[0], previous) == Some(text)
                {
                    (0, previous)
                } else {
                    (side, i)
                };
                if let Some(raw) = indices[side].line(source.texts[side], line) {
                    select.line(&painter, rect, &g, pos, raw, raw.as_ptr().addr() - source.texts[side].as_ptr().addr(), side, ACCENT);
                }
            }
            painter.galley(pos, g.clone(), c.fg);
            if let Some(reports) = unicode {
                unicode_highlight::paint(ui, id.with(("unicode", old, i)), &painter, &g, pos,
                    reports[usize::from(!old)].line(i), 0, 0..usize::MAX, prefs.syntax, unicode_highlight::ink(c.dark));
            }
        };
        for row in visible_rows(&tops, viewport.top(), viewport.bottom()) {
            let y = origin.y + tops[row];
            let h = tops[row + 1] - tops[row];
            if split {
                let s = &cmp.split[row];
                let lrect = Rect::from_min_size(pos2(split_x, y), vec2((half - 1.0).max(0.0), h));
                let rrect = Rect::from_min_size(pos2(split_x + half, y), vec2(viewport.width() - half, h));
                let pair = (s.changed && s.left.is_some() && s.right.is_some()).then(|| (s.left.unwrap_or(0), s.right.unwrap_or(0)));
                draw(&painter, lrect, s.left, true, if s.changed && s.left.is_some() { RowKind::Removed } else { RowKind::Same }, pair, None, viewport.left().min(extra[0]));
                draw(&painter, rrect, s.right, false, if s.changed && s.right.is_some() { RowKind::Added } else { RowKind::Same }, pair, None, viewport.left().min(extra[1]));
                painter.line_segment([pos2(split_x + half - 0.5, y), pos2(split_x + half - 0.5, y + h)], Stroke::new(1.0, c.line));
            } else {
                let rw = &cmp.rows[row];
                let rect = Rect::from_min_size(pos2(origin.x, y), vec2(width, h));
                let (no, old) = match rw.kind {
                    RowKind::Removed => (rw.old, true),
                    _ => (rw.new, false),
                };
                let unchanged_original = (rw.kind == RowKind::Same).then_some(rw.old).flatten();
                draw(&painter, rect, no, old, rw.kind, rw.pair, unchanged_original, 0.0);
            }
        }
        for select in [original, modified].into_iter().flatten() {
            let runs = if prefs.syntax { [old_hl.as_slice(), new_hl.as_slice()] } else { [&[][..]; 2] };
            select.finish(ui, runs, c.fg, |_| {});
        }
    });
    let current = split_view::Scroll::from_output(&output);
    ui.data_mut(|data| data.insert_temp(scroll_key, current));
    if split {
        let basis = current.total.max(1.0);
        let viewport = (current.top / basis, (current.top + current.viewport) / basis);
        if let Some(target) = split_view::ruler(ui, id, overview, &cmp.split_index, viewport, geometry.with(basis.to_bits()), |row| tops[row] / basis) {
            ui.data_mut(|data| data.insert_temp(overview_key, current.clamp(target * basis)));
            ui.ctx().request_repaint();
        }
    }
    ui.advance_cursor_after_rect(body);
}

pub(super) struct ComparisonSource<'a> {
    pub(super) texts: [&'a str; 2],
    pub(super) unicode: Option<&'a ComparisonUnicode>,
}

pub struct ComparisonSearch<'a> {
    hit: &'a find::Match,
    original: bool,
    reveal: bool,
}

fn comparison_line(cmp: &Comparison, split: bool, original: bool, number: usize) -> Option<usize> {
    if split {
        return cmp.split_index.lines[usize::from(!original)].row(number);
    }
    cmp.rows.iter().position(|r| {
        if original && r.kind == RowKind::Same {
            return r.old == Some(number);
        }
        (if original && r.kind == RowKind::Removed { r.old } else if !original && r.kind != RowKind::Removed { r.new } else { None }) == Some(number)
    })
}

#[cfg(test)]
#[test]
fn headless_comparison_search_tracks_original_and_modified_line_numbers() {
    let cmp = textdiff::compare("heading\nremoved\n", "heading\nadded\nmore\n", false);
    assert_eq!(comparison_line(&cmp, false, true, 1), Some(1));
    assert_eq!(comparison_line(&cmp, false, false, 1), Some(2));
    assert_eq!(comparison_line(&cmp, true, false, 2), Some(2));
}

#[cfg(test)]
#[test]
fn headless_split_overview_paints_both_sides_and_navigates_at_high_dpi() {
    use egui::{Event, Key, Modifiers, PointerButton, RawInput};
    let ctx = egui::Context::default();
    ctx.set_pixels_per_point(1.5);
    let id = egui::Id::unique("split_overview_test");
    let text: String = (0..900).map(|i| format!("Line {i} stays the same.\n")).collect();
    let cmp = textdiff::compare(&format!("Removed\n{text}"), &format!("{text}Added\n"), false);
    let prefs = EditorPrefs { font: FontId::monospace(12.0), tab: 4, crlf: false, syntax: false, numbers: true };
    let run = |split, events| {
        let mut out = ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(710.0, 460.0))),
                events,
                ..Default::default()
            },
            |ui| compare_view(ui, id, (1, false), &cmp, &prefs, split, true, &(true, Vec::new(), Vec::new()), ComparisonSource { texts: ["", ""], unicode: None }, false, false, &mut None, None),
        );
        out.textures_delta.clear();
        out.shapes
    };
    let shapes = run(true, vec![]);
    let ruler = ctx.read_response(id.with("overview_ruler")).expect("fixed overview ruler");
    let marks = |shape: &egui::epaint::ClippedShape, color, left| {
        matches!(&shape.shape, egui::Shape::Rect(marker)
            if marker.fill == color
                && ruler.rect.contains(marker.rect.center())
                && (marker.rect.center().x < ruler.rect.center().x) == left)
    };
    assert!(shapes.iter().any(|s| marks(s, BAD, true)));
    assert!(shapes.iter().any(|s| marks(s, GOOD, false)));

    let pos = pos2(ruler.rect.center().x, ruler.rect.top() + ruler.rect.height() * 0.7);
    let click = |pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    run(true, vec![Event::PointerMoved(pos), click(true)]);
    run(true, vec![click(false)]);
    run(true, vec![]);
    let scroll = || ctx.data_mut(|data| data.get_temp::<split_view::Scroll>(id.with("comparison_scroll"))).unwrap();
    assert!(scroll().top > scroll().total * 0.6);
    let key = |key| Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
    run(true, vec![key(Key::End)]);
    run(true, vec![]);
    assert!((scroll().top - scroll().clamp(f32::MAX)).abs() < 1.0);
    run(true, vec![key(Key::Home)]);
    run(true, vec![]);
    assert_eq!(scroll().top, 0.0);
    run(false, vec![]);
    run(false, vec![]);
    assert!(ctx.read_response(id.with("overview_ruler")).is_none(), "inline view has no overview rail");
}

#[cfg(test)]
#[test]
fn headless_unicode_decorations_cover_original_and_modified_comparisons() {
    let ctx = egui::Context::default();
    let id = egui::Id::unique("unicode_comparison");
    let old = "café 👩🏽‍💻\noriginal a\u{200b}b\n";
    let new = "café 👩🏽‍💻\nmodified payp\u{430}l\n";
    let cmp = textdiff::compare(old, new, false);
    let data = [old, new].map(|text| Arc::new(unicode_highlight::Report::scan("fixture.txt", text)));
    let prefs = EditorPrefs { font: FontId::monospace(12.0), tab: 4, crlf: false, syntax: false, numbers: true };
    let color = unicode_highlight::ink(p().dark);
    for split in [false, true] {
        let mut outcome = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 500.0))),
                ..Default::default()
            },
            |ui| compare_view(ui, id, (1, false), &cmp, &prefs, split, true, &(true, Vec::new(), Vec::new()), ComparisonSource { texts: [old, new], unicode: Some(&data) }, false, false, &mut None, None),
        );
        outcome.textures_delta.clear();
        let markers: Vec<_> = outcome.shapes.iter().filter_map(|shape| {
            if let egui::Shape::Rect(rect) = &shape.shape && rect.stroke.color == color {
                Some(rect.rect.center().x)
            } else {
                None
            }
        }).collect();
        assert_eq!(markers.len(), 2, "only the suspicious code points, split={split}");
        if split {
            assert!(markers[0] < 440.0 && markers[1] > 440.0, "{markers:?}");
        }
    }
}

#[cfg(test)]
#[test]
fn headless_read_only_split_selects_and_copies_each_source() {
    use egui::{Event, Key, Modifiers, OutputCommand, PointerButton, RawInput};
    let ctx = egui::Context::default();
    let id = egui::Id::unique("copy_comparison");
    let old = "old first\r\ncafé 👩🏽‍💻\r\nold third\r\n";
    let new = "new first\ncafé 👩🏽‍💻\nnew third\n";
    let cmp = textdiff::compare(old, new, false);
    let prefs = EditorPrefs { font: FontId::monospace(12.0), tab: 4, crlf: false, syntax: false, numbers: true };
    let run = |events| {
        let mut outcome = ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 500.0))),
                events,
                ..Default::default()
            },
            |ui| compare_view(ui, id, (1, false), &cmp, &prefs, true, false, &(true, Vec::new(), Vec::new()), ComparisonSource { texts: [old, new], unicode: None }, false, false, &mut None, None),
        );
        outcome.textures_delta.clear();
        outcome.platform_output.commands.into_iter().filter_map(|command| {
            if let OutputCommand::CopyText(text) = command { Some(text) } else { None }
        }).collect::<Vec<_>>()
    };
    run(vec![]);
    let pointer = |pos, pressed| Event::PointerButton {
        pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE,
    };
    let pane = |side: usize| ctx.read_response(id.with(("comparison_selection", true, side))).unwrap().rect;
    let left = pane(0);
    let from = pos2(left.left() + 2.0, left.top() + 8.0);
    let to = pos2(left.left() + 200.0, left.top() + 26.0);
    run(vec![Event::PointerMoved(from), pointer(from, true)]);
    run(vec![Event::PointerMoved(to)]);
    run(vec![pointer(to, false)]);
    assert_eq!(run(vec![Event::Copy]), ["old first\r\ncafé 👩🏽‍💻"]);

    let right = pane(1);
    let point = pos2(right.left() + 14.0, right.top() + 8.0);
    run(vec![Event::PointerMoved(point), pointer(point, true)]);
    run(vec![pointer(point, false)]);
    let select_all = Event::Key { key: Key::A, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND };
    assert_eq!(run(vec![select_all, Event::Copy]), [new]);
}

/// The file, read-only, with line numbers and colours.
#[derive(Default)]
struct FileLines<'a> {
    removed: &'a [bool],
    aligned: Option<&'a [Option<usize>]>,
    jump: Option<usize>,
    scroll: Option<f32>,
    row_height: Option<f32>,
    folds: Option<&'a mut folding::Folds>,
}

#[derive(Default)]
struct FileLineIndex {
    // A scrolling frame only needs the line index for rows inside the viewport.
    anchors: Vec<usize>,
    count: usize,
    longest: usize,
}

impl FileLineIndex {
    const STRIDE: usize = 64;

    fn new(text: &str) -> Self {
        let mut index = Self::default();
        for (number, line) in text.lines().enumerate() {
            if number % Self::STRIDE == 0 {
                index.anchors.push(line.as_ptr().addr() - text.as_ptr().addr());
            }
            index.count += 1;
            index.longest = index.longest.max(line.len());
        }
        index
    }

    fn line<'a>(&self, text: &'a str, row: usize) -> Option<&'a str> {
        let start = *self.anchors.get(row / Self::STRIDE)?;
        text[start..].lines().nth(row % Self::STRIDE)
    }
}

fn cached_file_lines(ui: &Ui, id: egui::Id, revision: FileRevision, text: &str) -> Arc<FileLineIndex> {
    let cached = ui.data_mut(|data| data.get_temp::<(FileRevision, Arc<FileLineIndex>)>(id));
    cached.filter(|(key, _)| *key == revision).map(|(_, index)| index).unwrap_or_else(|| {
        let index = Arc::new(FileLineIndex::new(text));
        ui.data_mut(|data| data.insert_temp(id, (revision, index.clone())));
        index
    })
}

#[allow(clippy::too_many_arguments)]
fn file_lines(ui: &mut Ui, file: &str, text: &str, wrap: bool, dark: bool, prefs: &EditorPrefs, revision: FileRevision, hit: Option<&find::Match>, reveal: bool, view: FileLines<'_>) -> split_view::Pane {
    let c = p();
    let compose_emoji = super::fonts::use_emoji_font(ui.ctx());
    let font = &prefs.font;
    let height = view.row_height.unwrap_or_else(|| editor_line_height(font));
    let gutter = if prefs.numbers { GUTTER } else if view.folds.as_deref().is_some_and(folding::Folds::any) { 18.0 } else { 0.0 };
    let id = ui.scope_id().with(("file_hl", file));
    type CachedSyntax = Arc<Mutex<ParseCache<FileHighlightKey, Vec<Runs>>>>;
    let parsed = ui.data_mut(|data| {
        data.get_temp::<CachedSyntax>(id).unwrap_or_else(|| {
            let cache = CachedSyntax::default();
            data.insert_temp(id, cache.clone());
            cache
        })
    });
    let runs = syntax_runs(
        &mut parsed.lock().expect("syntax cache lock"), ui.ctx(),
        (revision, dark, prefs.syntax), file, text,
    );
    type CachedUnicode = Arc<Mutex<ParseCache<FileRevision, unicode_highlight::Report>>>;
    let parsed_unicode = ui.data_mut(|data| {
        data.get_temp::<CachedUnicode>(id).unwrap_or_else(|| {
            let cache = CachedUnicode::default();
            data.insert_temp(id, cache.clone());
            cache
        })
    });
    let unicode = parsed_unicode.lock().expect("Unicode cache lock")
        .get(ui.ctx(), revision, file, text, unicode_highlight::Report::scan);
    if unicode.as_ref().is_some_and(|report| report.more) {
        unicode_highlight::caption(ui);
    }
    let index = cached_file_lines(ui, id.with("line_index"), revision, text);
    let cw = char_width(ui, font);
    let longest = index.longest as f32 * cw + gutter + 20.0;
    let columns = ((ui.available_width() - gutter - 8.0 - 6.0).max(cw * 10.0) / cw).floor() as usize;
    let fold_generation = view.folds.as_deref().map_or(0, folding::Folds::generation);
    let visible = view.folds.as_deref().filter(|folds| folds.has_collapsed()).map(|folds| {
        let key = (revision, fold_generation);
        let id = id.with("visible_lines");
        let cached = ui.data_mut(|data| data.get_temp::<((FileRevision, u64), Arc<folding::VisibleLines>)>(id));
        cached.filter(|(cached, _)| *cached == key).map(|(_, rows)| rows).unwrap_or_else(|| {
            let rows = Arc::new(folds.visible_lines(index.count));
            ui.data_mut(|data| data.insert_temp(id, (key, rows.clone())));
            rows
        })
    });
    let line_at = |row: usize| {
        if let Some(visible) = visible.as_ref() {
            visible.get(row)
        } else {
            view.aligned.map_or(Some(row), |alignment| alignment.get(row).copied().flatten())
        }
    };
    let rows = visible.as_ref().map_or_else(|| view.aligned.map_or(index.count, <[_]>::len), |rows| rows.len());
    let tops = wrap.then(|| {
        let tops_id = id.with("file_tops");
        let tops_key = (revision, columns, height.to_bits(), fold_generation);
        let cached = ui.data_mut(|d| d.get_temp::<(FileRowsKey, Arc<Vec<f32>>)>(tops_id));
        cached.filter(|(key, _)| *key == tops_key).map(|(_, tops)| tops).unwrap_or_else(|| {
            let mut lines = text.lines();
            let mut next = 0;
            let tops = Arc::new(sized_row_tops(rows, height, |row| {
                if let Some(line) = line_at(row) {
                    let text = lines.nth(line - next);
                    next = line + 1;
                    text.map_or(1, |line| wrapped_lines(line, columns))
                } else {
                    1
                }
            }));
            ui.data_mut(|data| data.insert_temp(tops_id, (tops_key, tops.clone())));
            tops
        })
    });
    let row_top = |row| {
        tops.as_ref().map_or(row as f32 * height, |tops| tops[row])
    };
    let total = row_top(rows);
    let mut scroll = egui::ScrollArea::both().id_salt(("file", file)).auto_shrink([false, false]).content_margin(0);
    if let Some(offset) = view.scroll {
        scroll = scroll.vertical_scroll_offset(offset);
    }
    let visual_row = |line| if let Some(visible) = &visible {
        visible.find(line)
    } else {
        view.aligned.map_or(Some(line), |aligned| aligned.iter().position(|candidate| *candidate == Some(line)))
    };
    let jump = view.jump.and_then(visual_row);
    if let Some(line) = jump.filter(|&line| line < rows) {
        scroll = scroll.vertical_scroll_offset((row_top(line) - ui.available_height() / 3.0).max(0.0));
    }
    if reveal && let Some(hit) = hit
        && let Some(row) = visual_row(hit.line).filter(|&row| row < rows)
        && let Some(line) = index.line(text, hit.line)
    {
        let marks = hit.mark(hit.line, line);
        let columns_before = marks.map_or(0, |(range, _)| line[..range.start].chars().count());
        let offset = if wrap { columns_before / columns } else { 0 };
        scroll = scroll.vertical_scroll_offset(row_top(row) + offset as f32 * height);
        if !wrap {
            let x = columns_before as f32 * cw + gutter + 8.0;
            scroll = scroll.horizontal_scroll_offset((x - ui.available_width() / 2.0).max(0.0));
        }
    }
    let mut toggle = None;
    let mut fold_all = None;
    let output = scroll.show_viewport(ui, |ui, viewport| {
        let width = if wrap { ui.available_width() } else { longest.max(ui.available_width()) };
        let (_, _) = ui.allocate_exact_size(vec2(width, total.max(viewport.height())), Sense::hover());
        let clip = ui.clip_rect();
        let select_rect = Rect::from_min_max(pos2((clip.left() + gutter).min(clip.right()), clip.top()), clip.max);
        let mut selection = PreviewSelection::new(ui, id.with("selection"), select_rect, revision, [text, text], 1);
        let origin = ui.min_rect().min;
        let onscreen = if let Some(tops) = &tops {
            visible_rows(tops, viewport.top(), viewport.bottom())
        } else {
            let start = ((viewport.top() / height).floor() as usize).min(rows);
            let end = ((viewport.bottom() / height).ceil() as usize).min(rows);
            start..end.max(start)
        };
        for row in onscreen {
            let top = row_top(row);
            let y = origin.y + top;
            let Some(i) = line_at(row) else { continue };
            let Some(line) = index.line(text, i) else { continue };
            if view.removed.get(i) == Some(&true) {
                let rect = Rect::from_min_size(pos2(origin.x, y), vec2(width, row_top(row + 1) - top));
                ui.painter().rect_filled(rect, 0, BAD.gamma_multiply(0.2));
            }
            if prefs.numbers {
                ui.painter().text(pos2(origin.x + gutter - 8.0, y + height / 2.0), Align2::RIGHT_CENTER, (i + 1).to_string(), font.clone(), c.white(0.33));
            }
            if let Some(folds) = view.folds.as_deref() && folds.foldable(i) {
                let rect = Rect::from_min_size(pos2(origin.x, y), vec2(16.0, height));
                let response = ui.interact(rect, id.with(("fold_line", i)), Sense::click());
                let collapsed = folds.collapsed(i);
                let label = if collapsed { "Unfold" } else { "Fold" };
                response.widget_info(|| egui::WidgetInfo::labeled(
                    egui::Role::Button, ui.is_enabled(), format!("{label} line {}", i + 1)
                ));
                widgets::chevron(ui, rect, !collapsed, 12.0, if response.hovered() || collapsed { c.fg } else { c.fg3 });
                if response.clicked() {
                    toggle = Some(i);
                }
            }
            let marks = hit.and_then(|hit| hit.mark(i, line));
            let mut job = highlight::job(line, runs.as_ref().and_then(|runs| runs.get(i)), c.white(0.85), font.clone(), marks.as_slice());
            if wrap {
                job.wrap = egui::text::TextWrapping { max_width: columns as f32 * cw + 0.5, break_anywhere: true, ..Default::default() };
                for section in &mut job.sections {
                    section.format.line_height = Some(height);
                }
            }
            super::fonts::compose_emoji(&mut job, compose_emoji);
            let g = ui.painter().layout_job(job);
            let top = if wrap { y } else { y + height / 2.0 - g.size().y / 2.0 };
            let pos = pos2(origin.x + gutter + 8.0, top);
            let row_rect = Rect::from_min_max(pos2(origin.x + gutter, y), pos2(origin.x + width, y + row_top(row + 1) - row_top(row)));
            let painter = ui.painter().with_clip_rect(row_rect.intersect(ui.clip_rect()));
            selection.line(&painter, row_rect, &g, pos, line, line.as_ptr().addr() - text.as_ptr().addr(), 1, ACCENT);
            ui.painter().galley(pos, g.clone(), c.fg);
            if let Some(report) = &unicode {
                let painter = ui.painter().with_clip_rect(row_rect.intersect(ui.clip_rect()));
                unicode_highlight::paint(ui, id.with(("unicode", i)), &painter, &g, pos,
                    report.line(i), 0, 0..usize::MAX, prefs.syntax, unicode_highlight::ink(dark));
            }
        }
        let syntax = runs.as_deref().map_or(&[][..], Vec::as_slice);
        selection.finish(ui, [syntax; 2], c.fg, |ui| {
            if let Some(folds) = view.folds.as_deref().filter(|folds| folds.any()) {
                ui.separator();
                if ui.button("Fold All").clicked() {
                    fold_all = Some(true);
                    ui.close();
                }
                if ui.add_enabled(folds.has_collapsed(), egui::Button::new("Unfold All")).clicked() {
                    fold_all = Some(false);
                    ui.close();
                }
            }
        });
    });
    if let Some(folds) = view.folds {
        if let Some(row) = toggle {
            folds.toggle(row);
        }
        match fold_all {
            Some(true) => folds.fold_all(),
            Some(false) => folds.unfold_all(),
            None => {}
        }
        if toggle.is_some() || fold_all.is_some() {
            ui.ctx().request_repaint();
        }
    }
    split_view::Pane {
        scroll: split_view::Scroll::from_output(&output),
        lines: tops.map_or(split_view::LineTops::Fixed(height), split_view::LineTops::Consecutive),
        inset: 0.0,
    }
}

/// Before and after pictures side by side.
fn image_diff(ui: &mut Ui, key: &str, before: Option<&[u8]>, after: Option<&[u8]>) {
    let c = p();
    let w = ui.available_width();
    let h = ui.available_height();
    ui.horizontal_top(|ui| {
        for (label, bytes, color, side) in [("Before", before, REMOVED, "old"), ("After", after, GOOD, "new")] {
            ui.allocate_ui(vec2(w / 2.0, h), |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(16.0);
                    ui.label(egui::RichText::new(label).font(super::fonts::semibold(12.0)).color(color));
                    ui.add_space(8.0);
                    let (rect, _) = ui.allocate_exact_size(vec2(w / 2.0 - 32.0, (h - 60.0).max(120.0)), Sense::hover());
                    // A checkerboard behind transparent pictures.
                    let cell = 8.0;
                    let painter = ui.painter().with_clip_rect(rect);
                    let (a, b) = (Color32::from_rgb(0x2a, 0x2d, 0x34), Color32::from_rgb(0x22, 0x25, 0x2b));
                    let mut yy = rect.top();
                    let mut row = 0;
                    while yy < rect.bottom() {
                        let mut xx = rect.left();
                        let mut col = row;
                        while xx < rect.right() {
                            painter.rect_filled(Rect::from_min_size(pos2(xx, yy), Vec2::splat(cell)), CornerRadius::ZERO, if col % 2 == 0 { a } else { b });
                            xx += cell;
                            col += 1;
                        }
                        yy += cell;
                        row += 1;
                    }
                    ui.painter().rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0, c.white(0.1)), StrokeKind::Inside);
                    match bytes {
                        Some(bytes) => {
                            // Named by its content too, so a changed picture isn't shown from egui's cache.
                            let id = {
                                use std::hash::{Hash, Hasher};
                                let mut h = std::collections::hash_map::DefaultHasher::new();
                                bytes.hash(&mut h);
                                h.finish()
                            };
                            let img = egui::Image::from_bytes(preview_uri(&key, side, id, bytes), bytes.to_vec()).max_size(rect.size()).maintain_aspect_ratio(true);
                            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::centered_and_justified(egui::Direction::TopDown)));
                            child.add(img);
                        }
                        None => {
                            ui.painter().text(rect.center(), Align2::CENTER_CENTER, "No image", FontId::proportional(12.0), c.fg3);
                        }
                    }
                });
            });
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn wrapped_rows_take_their_screen_lines() {
        use super::{row_tops, visible_rows, wrapped_lines, LINE};
        assert_eq!(wrapped_lines("", 10), 1);
        assert_eq!(wrapped_lines("0123456789", 10), 1);
        assert_eq!(wrapped_lines("0123456789a", 10), 2);
        assert_eq!(wrapped_lines("\tab", 4), 2);
        let tops = row_tops(3, |r| [1, 3, 1][r]);
        assert_eq!(tops, vec![0.0, LINE, LINE * 4.0, LINE * 5.0]);
        // The tall second row is shown while any of its lines are in view.
        assert_eq!(visible_rows(&tops, LINE * 2.0, LINE * 3.0), 1..2);
        assert_eq!(visible_rows(&tops, 0.0, LINE * 10.0), 0..3);
        assert_eq!(visible_rows(&tops, LINE * 4.5, LINE * 10.0), 2..3);
    }
}

/// `range` of `from`, moved to the same non-whitespace characters of `to` (the same text apart from whitespace).
fn remap_ignoring_whitespace(range: &std::ops::Range<usize>, from: &str, to: &str) -> Option<std::ops::Range<usize>> {
    let before = from.get(..range.start)?.chars().filter(|c| !c.is_whitespace()).count();
    let inside = from.get(range.clone())?.chars().filter(|c| !c.is_whitespace()).count();
    if inside == 0 {
        return None;
    }
    let mut seen = 0;
    let (mut start, mut end) = (None, None);
    for (at, c) in to.char_indices().filter(|(_, c)| !c.is_whitespace()) {
        if seen == before {
            start = Some(at);
        }
        seen += 1;
        if seen == before + inside {
            end = Some(at + c.len_utf8());
            break;
        }
    }
    Some(start?..end?)
}

/// The cache name of a previewed picture. egui's loaders pick a decoder from the extension, so an SVG's name ends in
/// `.svg`, after the content hash that keeps a changed picture from being shown from the cache.
fn preview_uri(key: &str, side: impl std::fmt::Display, id: u64, bytes: &[u8]) -> String {
    let svg = key.to_ascii_lowercase().ends_with(".svg") || bytes.iter().take(2048).copied().collect::<Vec<u8>>().windows(4).any(|w| w.eq_ignore_ascii_case(b"<svg"));
    format!("bytes://{key}-{side}-{id:016x}{}", if svg { ".svg" } else { "" })
}

/// Whether File View can show this file rendered as Markdown.
fn is_markdown(file: &str) -> bool {
    let lower = file.to_ascii_lowercase();
    [".md", ".markdown", ".mdown", ".mkd"].iter().any(|ext| lower.ends_with(ext))
}
