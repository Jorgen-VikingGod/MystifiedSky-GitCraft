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

const LINE: f32 = 19.0;
const HUNK_HEAD: f32 = 28.0;
const GUTTER: f32 = 44.0;

/// A diff read for a tab.
#[derive(Default)]
pub struct DiffCache {
    pub versions: Option<Loaded<FileVersions>>,
    pub lines: Option<Loaded<Vec<DiffLine>>>,
    pub comparison: Option<(bool, Comparison)>,
    pub hl: Option<(bool, Vec<Runs>, Vec<Runs>)>,
    pub hunk_hl: Option<(bool, HashMap<(usize, usize), Runs>)>,
    pub images: Option<(Option<Vec<u8>>, Option<Vec<u8>>)>,
}

/// What the open diff shows: the view, the edit in progress, picked lines.
#[derive(Default)]
pub struct DiffUi {
    pub key: String,
    pub file_view: bool,
    pub editing: bool,
    pub draft: Option<String>,
    pub token: Option<String>,
    /// Picked lines: hunk, line indexes, the line a Shift+click range starts from.
    pub picked: Option<(usize, Vec<usize>, usize)>,
    pub jump: Option<f32>,
    pub allow_large: bool,
    pub saving: bool,
    pub draft_hl: Option<(u64, bool, Vec<Runs>)>,
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
    /// Read the file's versions and diff once per target.
    pub(crate) fn want_diff(&mut self, tab: u64, target: DiffTarget, allow_large: bool) {
        let key = diff_key(&target, allow_large);
        if self.diffs.contains_key(&(tab, key.clone())) {
            return;
        }
        let Some(path) = self.repo(tab).map(|r| r.path.clone()) else { return };
        self.diffs.insert((tab, key.clone()), DiffCache { versions: Some(Loaded::Loading), ..Default::default() });
        self.spawn(move || {
            let dir = std::path::Path::new(&path);
            let versions = gitcraft_core::files::versions_with_limit(dir, &target.source, &target.file, target.parent.as_deref(), target.untracked, allow_large);
            let lines = (!versions.too_large && !versions.binary).then(|| gitcraft_core::git::file_diff_opts(dir, &target.source, &target.file, target.untracked, target.parent.as_deref(), false));
            super::Msg::Diff { tab, key, versions: Box::new(versions), lines }
        });
    }

    pub(crate) fn diff_loaded(&mut self, tab: u64, key: String, versions: FileVersions, lines: Option<Vec<DiffLine>>) {
        let entry = self.diffs.entry((tab, key)).or_default();
        let images = (versions.old_image.is_some() || versions.new_image.is_some()).then(|| (versions.old_image.as_deref().and_then(data_url_bytes), versions.new_image.as_deref().and_then(data_url_bytes)));
        entry.images = images;
        entry.versions = Some(Loaded::Ready(versions));
        entry.lines = lines.map(Loaded::Ready);
    }

    /// Save the editor's text to the working-folder copy.
    pub(crate) fn save_draft(&mut self, tab: u64, utf8: bool) {
        let Some(r) = self.repo_mut(tab) else { return };
        let (Some(target), Some(draft)) = (r.ui.diff.clone(), r.diff_ui.draft.clone()) else { return };
        if r.diff_ui.saving {
            return;
        }
        r.diff_ui.saving = true;
        let (path, token) = (r.path.clone(), r.diff_ui.token.clone());
        self.spawn(move || {
            let result = gitcraft_core::files::save_checked_mode(std::path::Path::new(&path), &target.file, &draft, token.as_deref(), utf8);
            super::Msg::Saved { tab, file: target.file, result }
        });
    }

    pub(crate) fn saved(&mut self, tab: u64, file: String, result: Result<(), String>) {
        if let Some(r) = self.repo_mut(tab) {
            r.diff_ui.saving = false;
        }
        match result {
            Ok(()) => {
                if let Some(r) = self.repo_mut(tab) {
                    r.diff_ui.draft = None;
                    r.diff_ui.editing = false;
                }
                let name = file.rsplit('/').next().unwrap_or(&file).to_owned();
                self.toast(true, format!("Saved {name}"));
                self.diff_changed(tab);
                self.load_repo(tab);
            }
            Err(e) if e.contains("ENCODING_UNREPRESENTABLE") || e.contains("ENCODING_DECODE_LOSS") => {
                self.dialog = Some(super::dialogs::Dialog::confirm_with(
                    "Save this file as UTF-8?",
                    "The current encoding cannot represent every character. The original file is unchanged. Convert this file to UTF-8 with a byte-order mark to preserve your text?",
                    "Save as UTF-8",
                    false,
                    move |app, _| app.save_draft(tab, true),
                ));
            }
            Err(e) => self.toast(false, e),
        }
    }

    pub(crate) fn diff_view(&self, ui: &mut Ui, tab: u64, r: &mut RepoTab, target: &DiffTarget, later: &mut Vec<Later>) {
        let c = p();
        let key0 = diff_key(target, false);
        if r.diff_ui.key != key0 {
            r.diff_ui = DiffUi { key: key0, ..Default::default() };
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
        let mode = self.settings.string("diffMode", "hunk");
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
                r.diff_ui.editing = true;
                r.diff_ui.file_view = true;
            }
        }
        if r.diff_ui.editing {
            if widgets::button(&mut left_ui, Kind::Success, Some(Icon::Save), "Save Changes", 22.0, r.diff_ui.draft.is_some()).tip("Save (Ctrl+S)").clicked() {
                later.push(Box::new(move |app| app.save_draft(tab, false)));
            }
            if widgets::button(&mut left_ui, Kind::Default, None, "Cancel", 22.0, true).clicked() {
                r.diff_ui.draft = None;
                r.diff_ui.editing = false;
            }
        }
        // The source and File / Diff switch, centred.
        let src = source_label(&target.source);
        let sw: f32 = [src.as_str(), "File View", "Diff View"].iter().map(|t| ui.painter().layout_no_wrap(t.to_string(), FontId::proportional(12.0), c.fg).size().x + 20.0).sum();
        // Centred, but clear of the buttons on both sides.
        let left_end = left_ui.min_rect().right() + 12.0;
        let right_start = tools.right() - 8.0 - (24.0 * 7.0 + 2.0 * 6.0 + 16.0) - 12.0;
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
                r.diff_ui.file_view = i == 1;
            }
            x += tw;
        }
        let mut tools_right = ui.new_child(egui::UiBuilder::new().max_rect(tools.shrink2(vec2(8.0, 0.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
        tools_right.spacing_mut().item_spacing.x = 2.0;
        let toggle = |ui: &mut Ui, icon: Icon, tip: &str, on: bool| -> bool {
            let (rect, resp) = ui.allocate_exact_size(vec2(24.0, 22.0), Sense::click());
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
        if toggle(&mut tools_right, Icon::Pilcrow, "Ignore Leading/Trailing Whitespace", ignore_ws) {
            later.push(Box::new(move |app| app.settings.set("ignoreWhitespace", !ignore_ws)));
        }
        tools_right.add_space(8.0);
        for (m, icon, tip) in [("split", Icon::Columns2, "Split View"), ("inline", Icon::TextQuote, "Inline View"), ("hunk", Icon::Rows3, "Hunk View")] {
            if toggle(&mut tools_right, icon, tip, !r.diff_ui.file_view && mode == m) {
                r.diff_ui.file_view = false;
                later.push(Box::new(move |app| app.settings.set("diffMode", m)));
            }
        }
        tools_right.add_space(8.0);
        let next = toggle(&mut tools_right, Icon::ArrowDown, "Next change", false);
        let prev = toggle(&mut tools_right, Icon::ArrowUp, "Previous change", false);
        // Keys: Esc closes, S / U stage and unstage, Ctrl+S saves.
        let typing = ui.ctx().egui_wants_keyboard_input();
        if !super::dialog_open(ui.ctx()) {
            let (esc, s_key, u_key, save) = ui.input(|i| (i.key_pressed(egui::Key::Escape), i.key_pressed(egui::Key::S) && i.modifiers.is_none(), i.key_pressed(egui::Key::U) && i.modifiers.is_none(), i.key_pressed(egui::Key::S) && i.modifiers.command));
            if save && r.diff_ui.editing && r.diff_ui.draft.is_some() {
                later.push(Box::new(move |app| app.save_draft(tab, false)));
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
        if r.diff_ui.file_view {
            let shown = r.diff_ui.draft.clone().unwrap_or_else(|| v.new.clone().or_else(|| v.old.clone()).unwrap_or_default());
            if r.diff_ui.editing && can_edit_locked {
                return editor(ui, &target.file, &mut r.diff_ui, shown, wrap, dark, self.settings.flag("editorLineNumbers", true));
            }
            return file_lines(ui, &target.file, &shown, wrap, dark);
        }
        if mode != "hunk" {
            let old = v.old.clone().unwrap_or_default();
            let new = r.diff_ui.draft.clone().unwrap_or_else(|| v.new.clone().unwrap_or_default());
            let Some(cache) = cache else { return };
            let comparison = match &cache.comparison {
                Some((ws, cmp)) if *ws == ignore_ws => cmp,
                _ => {
                    let (o, n, t, f) = (old, new, key.clone(), target.file.clone());
                    later.push(Box::new(move |app| {
                        let cmp = textdiff::compare(&o, &n, ignore_ws);
                        let hl_old = highlight::whole(&f, &o, dark);
                        let hl_new = highlight::whole(&f, &n, dark);
                        if let Some(e) = app.diffs.get_mut(&(tab, t.clone())) {
                            e.comparison = Some((ignore_ws, cmp));
                            e.hl = Some((dark, hl_old, hl_new));
                        }
                    }));
                    return note(ui, "Comparing…", false);
                }
            };
            let empty = (dark, Vec::new(), Vec::new());
            let hl = cache.hl.as_ref().filter(|h| h.0 == dark).unwrap_or(&empty);
            return compare_view(ui, ui.scope_id().with(("cmp", &key)), comparison, &target.file, mode == "split", wrap, hl, next, prev, &mut r.diff_ui.jump);
        }
        let Some(lines) = lines else { return note(ui, "Loading diff…", false) };
        if lines.is_empty() {
            return note(ui, "No changes to show (the file may be empty, or only its mode changed).", false);
        }
        let actions = (can_stage || can_unstage || matches!(target.source, DiffSource::Commit(_))) && !ignore_ws;
        let hunk_hl = cache.and_then(|c| c.hunk_hl.as_ref()).filter(|h| h.0 == dark).map(|h| &h.1);
        if hunk_hl.is_none() && highlight::syntax_for(&target.file).is_some() {
            let (l, f, k) = (lines.clone(), target.file.clone(), key.clone());
            later.push(Box::new(move |app| {
                let hl = hunk_highlights(&f, &l, dark);
                if let Some(e) = app.diffs.get_mut(&(tab, k)) {
                    e.hunk_hl = Some((dark, hl));
                }
            }));
        }
        self.hunk_view(ui, tab, r, target, lines, hunk_hl, actions, wrap, next, prev, later);
    }

    #[allow(clippy::too_many_arguments)]
    fn hunk_view(&self, ui: &mut Ui, tab: u64, r: &mut RepoTab, target: &DiffTarget, lines: &[DiffLine], hl: Option<&HashMap<(usize, usize), Runs>>, actions: bool, wrap: bool, next: bool, prev: bool, later: &mut Vec<Later>) {
        let c = p();
        let (pre, hunks) = textdiff::split_hunks(lines);
        let pairs: Vec<HashMap<usize, usize>> = hunks.iter().map(|h| textdiff::replacement_lines(&h.lines)).collect();
        // Row positions: pre lines, then per hunk a header and its lines, with a gap after.
        let mut tops: Vec<f32> = Vec::with_capacity(hunks.len());
        let mut y = pre.len() as f32 * 26.0;
        for h in &hunks {
            tops.push(y);
            y += HUNK_HEAD + h.lines.len() as f32 * LINE + 8.0;
        }
        let total = y;
        let id = ui.scope_id().with(("hunks", &r.diff_ui.key));
        let state = egui::scroll_area::State::load(ui.ctx(), id.with(egui::Id::NULL.with("hunk_scroll")));
        let cur = state.map_or(0.0, |s| s.clamped_offset().y);
        let mut scroll = egui::ScrollArea::both().id_salt("hunk_scroll").auto_shrink([false, false]).content_margin(0);
        if next {
            if let Some(t) = tops.iter().find(|t| **t > cur + 4.0) {
                scroll = scroll.vertical_scroll_offset(*t);
            }
        } else if prev && let Some(t) = tops.iter().rev().find(|t| **t < cur - 8.0) {
            scroll = scroll.vertical_scroll_offset(*t);
        }
        let font = FontId::monospace(12.0);
        let mut run: Option<(PatchOp, usize, Option<Vec<usize>>)> = None;
        let mut stash: Option<(usize, Option<Vec<usize>>)> = None;
        let mut revert: Option<(usize, Option<Vec<usize>>)> = None;
        let mut pick: Option<(usize, usize, bool)> = None;
        let picked = r.diff_ui.picked.clone();
        let longest = lines.iter().map(|l| l.text.len()).max().unwrap_or(0) as f32 * 7.3 + 140.0;
        ui.push_id(id, |ui| {
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
                    let bottom = top + HUNK_HEAD + h.lines.len() as f32 * LINE;
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
                    let first = (((viewport.top() - top - HUNK_HEAD) / LINE).floor().max(0.0)) as usize;
                    let last = ((((viewport.bottom() - top - HUNK_HEAD) / LINE).ceil()) as usize + 1).min(h.lines.len());
                    for (li, l) in h.lines.iter().enumerate().take(last).skip(first) {
                        let row = Rect::from_min_size(origin + vec2(0.0, top + HUNK_HEAD + li as f32 * LINE), vec2(width, LINE));
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
                        painter.text(pos2(row.left() + GUTTER - 8.0, row.center().y), Align2::RIGHT_CENTER, num(l.old_no), font.clone(), c.fg3);
                        painter.text(pos2(row.left() + GUTTER * 2.0 - 8.0, row.center().y), Align2::RIGHT_CENTER, num(l.new_no), font.clone(), c.fg3);
                        painter.line_segment([pos2(row.left() + GUTTER * 2.0, row.top()), pos2(row.left() + GUTTER * 2.0, row.bottom())], Stroke::new(1.0, c.white(0.05)));
                        let sign = match l.kind {
                            DiffLineKind::Added => "+",
                            DiffLineKind::Removed => "-",
                            _ => " ",
                        };
                        painter.text(pos2(row.left() + GUTTER * 2.0 + 8.0, row.center().y), Align2::LEFT_CENTER, sign, font.clone(), c.fg3);
                        let marks: Vec<(std::ops::Range<usize>, Color32)> = pairs[hi].get(&li).and_then(|o| textdiff::changed_words(&l.text, &h.lines[*o].text)).map(|m| m.into_iter().map(|r| (r, if l.kind == DiffLineKind::Added { GOOD.gamma_multiply(0.45) } else { BAD.gamma_multiply(0.45) })).collect()).unwrap_or_default();
                        let base = if l.kind == DiffLineKind::Meta { c.fg3 } else { c.white(0.85) };
                        let mut job = highlight::job(&l.text, hl.and_then(|h| h.get(&(hi, li))), base, font.clone(), &marks);
                        if wrap {
                            job.wrap.max_width = width - GUTTER * 2.0 - 30.0;
                            job.wrap.max_rows = 1;
                        }
                        let g = ui.painter().layout_job(job);
                        painter.galley(pos2(row.left() + GUTTER * 2.0 + 20.0, row.center().y - g.size().y / 2.0), g, c.fg);
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
            });
        });
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

/// How many screen lines `text` takes when lines break after `columns` characters (tabs count
/// as four).
fn wrapped_lines(text: &str, columns: usize) -> usize {
    let width: usize = text.chars().map(|ch| if ch == '\t' { 4 } else { 1 }).sum();
    width.div_ceil(columns.max(1)).max(1)
}

/// Where each row starts, and the total height at the end, for rows of `lines(row)` screen lines.
fn row_tops(rows: usize, lines: impl Fn(usize) -> usize) -> Vec<f32> {
    let mut tops = Vec::with_capacity(rows + 1);
    let mut y = 0.0;
    tops.push(y);
    for row in 0..rows {
        y += lines(row) as f32 * LINE;
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
pub fn compare_view(ui: &mut Ui, id: egui::Id, cmp: &Comparison, _file: &str, split: bool, wrap: bool, hl: &(bool, Vec<Runs>, Vec<Runs>), next: bool, prev: bool, jump: &mut Option<f32>) {
    let c = p();
    let font = FontId::monospace(12.0);
    let rows = if split { cmp.split.len() } else { cmp.rows.len() };
    let line_text = |old: bool, i: usize| if old { cmp.old.get(i).map(String::as_str).unwrap_or("") } else { cmp.new.get(i).map(String::as_str).unwrap_or("") };
    // Row tops, worked out again only when the width or the comparison changes.
    let cw = char_width(ui, &font);
    let full = ui.available_width();
    let text_w = |w: f32| (w - GUTTER - 8.0 - 6.0).max(cw * 10.0);
    let columns = (text_w(if split { full / 2.0 } else { full }) / cw).floor() as usize;
    let size: usize = cmp.old.iter().chain(cmp.new.iter()).map(String::len).sum();
    let tops_id = ui.scope_id().with(id).with(("cmp_tops", split, wrap, columns, rows, size));
    let tops: std::sync::Arc<Vec<f32>> = ui.data_mut(|d| d.get_temp(tops_id)).unwrap_or_else(|| {
        let tops = if !wrap {
            row_tops(rows, |_| 1)
        } else if split {
            row_tops(rows, |r| {
                let s = &cmp.split[r];
                let l = s.left.map_or(1, |i| wrapped_lines(line_text(true, i), columns));
                let rt = s.right.map_or(1, |i| wrapped_lines(line_text(false, i), columns));
                l.max(rt)
            })
        } else {
            row_tops(rows, |r| {
                let rw = &cmp.rows[r];
                let (no, old) = if rw.kind == RowKind::Removed { (rw.old, true) } else { (rw.new, false) };
                no.map_or(1, |i| wrapped_lines(line_text(old, i), columns))
            })
        };
        let tops = std::sync::Arc::new(tops);
        ui.data_mut(|d| d.insert_temp(tops_id, tops.clone()));
        tops
    });
    let total = tops.last().copied().unwrap_or(0.0);
    let changes: Vec<f32> = if split { &cmp.split_changes } else { &cmp.changes }.iter().map(|r| tops.get(*r).copied().unwrap_or(0.0)).collect();
    let state = egui::scroll_area::State::load(ui.ctx(), ui.scope_id().with(id).with(egui::Id::NULL.with("cmp_scroll")));
    let cur = state.map_or(0.0, |s| s.clamped_offset().y);
    let mut scroll = egui::ScrollArea::both().id_salt("cmp_scroll").auto_shrink([false, false]).content_margin(0);
    let view_h = ui.available_height();
    let target = if next { changes.iter().find(|t| **t > cur + view_h / 3.0 + 4.0).copied() } else if prev { changes.iter().rev().find(|t| **t < cur + view_h / 3.0 - 8.0).copied() } else { None };
    if let Some(t) = target.or(jump.take()) {
        scroll = scroll.vertical_scroll_offset((t - view_h / 3.0).max(0.0));
    }
    let longest = cmp.old.iter().chain(cmp.new.iter()).map(|l| l.len()).max().unwrap_or(0) as f32 * 7.3 + 80.0;
    let old_hl = &hl.1;
    let new_hl = &hl.2;
    ui.push_id(id, |ui| {
        scroll.show_viewport(ui, |ui, viewport| {
            let full = ui.available_width();
            let width = if split || wrap { full } else { longest.max(full) };
            let (_, _) = ui.allocate_exact_size(vec2(width, total.max(viewport.height())), Sense::hover());
            let origin = ui.min_rect().min;
            let painter = ui.painter().clone();
            let draw = |painter: &egui::Painter, rect: Rect, no: Option<usize>, old: bool, kind: RowKind, pair: Option<(usize, usize)>| {
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
                painter.text(pos2(rect.left() + GUTTER - 8.0, rect.top() + LINE / 2.0), Align2::RIGHT_CENTER, (i + 1).to_string(), font.clone(), c.fg3);
                let text = line_text(old, i);
                let marks: Vec<(std::ops::Range<usize>, Color32)> = pair
                    .and_then(|(o, n)| if old { textdiff::changed_words(text, line_text(false, n)) } else { textdiff::changed_words(text, line_text(true, o)) })
                    .map(|m| m.into_iter().map(|r| (r, if old { BAD.gamma_multiply(0.45) } else { GOOD.gamma_multiply(0.45) })).collect())
                    .unwrap_or_default();
                let runs = if old { old_hl.get(i) } else { new_hl.get(i) };
                let mut job = highlight::job(text, runs, c.white(0.85), font.clone(), &marks);
                if wrap {
                    job.wrap = egui::text::TextWrapping { max_width: columns as f32 * cw + 0.5, break_anywhere: true, ..Default::default() };
                    job.first_row_min_height = LINE;
                    for section in &mut job.sections {
                        section.format.line_height = Some(LINE);
                    }
                }
                let g = ui.painter().layout_job(job);
                let y = if wrap { rect.top() } else { rect.center().y - g.size().y / 2.0 };
                painter.with_clip_rect(rect.intersect(ui.clip_rect())).galley(pos2(rect.left() + GUTTER + 8.0, y), g, c.fg);
            };
            for row in visible_rows(&tops, viewport.top(), viewport.bottom()) {
                let y = origin.y + tops[row];
                let h = tops[row + 1] - tops[row];
                if split {
                    let s = &cmp.split[row];
                    let half = width / 2.0;
                    let lrect = Rect::from_min_size(pos2(origin.x, y), vec2(half - 1.0, h));
                    let rrect = Rect::from_min_size(pos2(origin.x + half, y), vec2(half, h));
                    let pair = (s.changed && s.left.is_some() && s.right.is_some()).then(|| (s.left.unwrap_or(0), s.right.unwrap_or(0)));
                    draw(&painter, lrect, s.left, true, if s.changed && s.left.is_some() { RowKind::Removed } else { RowKind::Same }, pair);
                    draw(&painter, rrect, s.right, false, if s.changed && s.right.is_some() { RowKind::Added } else { RowKind::Same }, pair);
                    painter.line_segment([pos2(origin.x + half - 0.5, y), pos2(origin.x + half - 0.5, y + h)], Stroke::new(1.0, c.line));
                } else {
                    let rw = &cmp.rows[row];
                    let rect = Rect::from_min_size(pos2(origin.x, y), vec2(width, h));
                    let (no, old) = match rw.kind {
                        RowKind::Removed => (rw.old, true),
                        _ => (rw.new, false),
                    };
                    draw(&painter, rect, no, old, rw.kind, rw.pair);
                }
            }
        });
    });
}

/// The file, read-only, with line numbers and colours.
fn file_lines(ui: &mut Ui, file: &str, text: &str, wrap: bool, dark: bool) {
    let c = p();
    let font = FontId::monospace(12.0);
    let id = ui.scope_id().with(("file_hl", file, text.len(), dark));
    let runs: std::sync::Arc<Vec<Runs>> = ui.data_mut(|d| d.get_temp(id)).unwrap_or_else(|| {
        let r = std::sync::Arc::new(highlight::whole(file, text, dark));
        ui.data_mut(|d| d.insert_temp(id, r.clone()));
        r
    });
    let lines: Vec<&str> = text.lines().collect();
    let longest = lines.iter().map(|l| l.len()).max().unwrap_or(0) as f32 * 7.3 + 80.0;
    let cw = char_width(ui, &font);
    let columns = ((ui.available_width() - GUTTER - 8.0 - 6.0).max(cw * 10.0) / cw).floor() as usize;
    let tops_id = id.with(("file_tops", wrap, columns));
    let tops: std::sync::Arc<Vec<f32>> = ui.data_mut(|d| d.get_temp(tops_id)).unwrap_or_else(|| {
        let tops = std::sync::Arc::new(row_tops(lines.len(), |i| if wrap { wrapped_lines(lines[i], columns) } else { 1 }));
        ui.data_mut(|d| d.insert_temp(tops_id, tops.clone()));
        tops
    });
    let total = tops.last().copied().unwrap_or(0.0);
    egui::ScrollArea::both().id_salt(("file", file)).auto_shrink([false, false]).content_margin(0).show_viewport(ui, |ui, viewport| {
        let width = if wrap { ui.available_width() } else { longest.max(ui.available_width()) };
        let (_, _) = ui.allocate_exact_size(vec2(width, total.max(viewport.height())), Sense::hover());
        let origin = ui.min_rect().min;
        for i in visible_rows(&tops, viewport.top(), viewport.bottom()) {
            let y = origin.y + tops[i];
            ui.painter().text(pos2(origin.x + GUTTER - 8.0, y + LINE / 2.0), Align2::RIGHT_CENTER, (i + 1).to_string(), font.clone(), c.white(0.33));
            let mut job = highlight::job(lines[i], runs.get(i), c.white(0.85), font.clone(), &[]);
            if wrap {
                job.wrap = egui::text::TextWrapping { max_width: columns as f32 * cw + 0.5, break_anywhere: true, ..Default::default() };
                for section in &mut job.sections {
                    section.format.line_height = Some(LINE);
                }
                let g = ui.painter().layout_job(job);
                ui.painter().galley(pos2(origin.x + GUTTER + 8.0, y), g, c.fg);
            } else {
                let g = ui.painter().layout_job(job);
                ui.painter().galley(pos2(origin.x + GUTTER + 8.0, y + LINE / 2.0 - g.size().y / 2.0), g, c.fg);
            }
        }
    });
}

/// The editor: the working-folder copy with colours; Ctrl+S saves.
fn editor(ui: &mut Ui, file: &str, d: &mut DiffUi, shown: String, wrap: bool, dark: bool, numbers: bool) {
    let c = p();
    let mut text = shown;
    let hash = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        h.finish()
    };
    if d.draft_hl.as_ref().is_none_or(|(h, dk, _)| *h != hash || *dk != dark) {
        d.draft_hl = Some((hash, dark, highlight::whole(file, &text, dark)));
    }
    let runs = d.draft_hl.as_ref().map(|h| h.2.clone()).unwrap_or_default();
    let font = FontId::monospace(12.0);
    let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, width: f32| {
        let s = buf.as_str();
        let mut job = egui::text::LayoutJob::default();
        for (i, line) in s.split('\n').enumerate() {
            let part = highlight::job(line, runs.get(i), c.white(0.85), font.clone(), &[]);
            for section in part.sections {
                let (a, b) = (section.byte_range.start.0, section.byte_range.end.0);
                job.append(&part.text[a..b], 0.0, section.format);
            }
            if i + 1 < s.split('\n').count() {
                job.append("\n", 0.0, egui::TextFormat::simple(font.clone(), c.fg));
            }
        }
        if wrap {
            job.wrap.max_width = width;
        }
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let lines = text.lines().count().max(1);
    egui::ScrollArea::both().id_salt(("editor", file)).auto_shrink([false, false]).show(ui, |ui| {
        ui.horizontal_top(|ui| {
            if numbers {
                let gutter = (1..=lines).map(|n| n.to_string()).collect::<Vec<_>>().join("\n");
                ui.add(egui::Label::new(egui::RichText::new(gutter).font(font.clone()).color(c.white(0.33))).selectable(false));
            }
            let edit = egui::TextEdit::multiline(&mut text).code_editor().frame(egui::Frame::NONE).desired_width(f32::INFINITY).desired_rows(lines).layouter(&mut layouter);
            if ui.add(edit).changed() {
                d.draft = Some(text.clone());
            }
        });
    });
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
                            let img = egui::Image::from_bytes(format!("bytes://{key}-{side}"), bytes.to_vec()).max_size(rect.size()).maintain_aspect_ratio(true);
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
