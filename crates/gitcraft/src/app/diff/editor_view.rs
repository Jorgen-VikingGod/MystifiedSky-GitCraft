//! Editable file and comparison surfaces backed by one working-folder draft.

use super::cursor_history::{CursorHistory, Direction};
use super::unicode_highlight::{self, Report};
use super::{
    ACCENT, App, BAD, DiffUi, FileHighlightKey, FileLines, FileRevision, GOOD, GUTTER, ParseCache,
    REMOVED, VIRTUAL_PREVIEW_THRESHOLD, editing, editor_id, file_lines, folding, highlight, p,
    split_view::{self, LineTops, Pane, Scroll, Sync},
    syntax_runs, widgets,
};
use crate::app::highlight::Runs;
use crate::app::textdiff::Comparison;
use egui::{Align2, FontId, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use std::borrow::Cow;
use std::cell::Cell;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::sync::{Arc, Weak};

/// Preferences > Editor, as the file editor uses them.
pub struct EditorPrefs {
    pub font: FontId,
    /// Tab inserts spaces up to the next multiple of this.
    pub tab: usize,
    /// New lines typed or pasted are CRLF.
    pub crlf: bool,
    pub syntax: bool,
    pub numbers: bool,
}

pub(super) fn editor_line_height(font: &FontId) -> f32 {
    (font.size * 1.5).round()
}

impl App {
    pub(in crate::app) fn editor_prefs(&self, ctx: &egui::Context) -> EditorPrefs {
        let size = (self.settings.num("editorFontSize", 12.0) as f32).clamp(8.0, 32.0);
        let name = self.settings.string("editorFontFamily", "");
        EditorPrefs {
            font: crate::app::fonts::preferred(ctx, &name, size),
            tab: (self.settings.num("editorTabSize", 4.0) as usize).clamp(1, 16),
            crlf: self
                .settings
                .string("editorEol", if cfg!(windows) { "crlf" } else { "lf" })
                == "crlf",
            syntax: self.settings.flag("editorSyntax", true),
            numbers: self.settings.flag("editorLineNumbers", true),
        }
    }
}

pub(super) struct FileEditor<'a> {
    pub(super) file: &'a str,
    pub(super) base: &'a str,
    pub(super) generation: u64,
    pub(super) wrap: bool,
    pub(super) dark: bool,
    pub(super) prefs: &'a EditorPrefs,
    pub(super) editable: bool,
    pub(super) changed: &'a [bool],
    pub(super) removed: &'a [folding::RemovedLine<'a>],
    pub(super) old_runs: &'a [Runs],
    pub(super) jump: Option<usize>,
    pub(super) scroll: Option<f32>,
    pub(super) spacers: bool,
}

pub(super) fn file_editor(ui: &mut Ui, d: &mut DiffUi, file: FileEditor<'_>) {
    show_file_editor(ui, d, file, false, None);
}

fn show_file_editor(
    ui: &mut Ui,
    d: &mut DiffUi,
    file: FileEditor<'_>,
    track_scroll: bool,
    old_unicode: Option<&Report>,
) -> Pane {
    let shown = d.draft.as_deref().unwrap_or(file.base);
    let revision = (file.generation, d.draft.as_ref().map(|_| d.edits));
    let mut reveal = d.find.bar(ui, shown, revision);
    if file.editable
        && let Some((updated, caret)) = d.find.replace_bar(ui, shown)
    {
        let id = editor_id(d.editor);
        let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
        let mut undo = state.undoer();
        undo.add_undo(&(
            state.cursor.char_range().unwrap_or_default(),
            shown.to_owned(),
        ));
        let cursor = egui::text::CCursorRange::one(egui::text::CCursor::new(caret));
        undo.add_undo(&(cursor, updated.clone()));
        state.set_undoer(undo);
        state.cursor.set_char_range(Some(cursor));
        state.store(ui.ctx(), id);
        d.find.replaced(&updated, caret);
        d.folds.unfold_all();
        d.editor_positions = None;
        d.cursors.clear();
        d.edits += 1;
        d.draft = Some(updated);
        reveal = false;
    }
    let shown = d.draft.as_deref().unwrap_or(file.base);
    let revision = (file.generation, d.draft.as_ref().map(|_| d.edits));
    if file.editable
        && std::path::Path::new(file.file)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
    {
        if d.json_check
            .as_ref()
            .is_none_or(|(checked, _)| *checked != revision)
        {
            d.json_check = Some((revision, json_syntax_error(shown)));
        }
        if let Some((_, Some(error))) = &d.json_check {
            ui.label(egui::RichText::new(format!("JSON syntax error. {error}")).color(REMOVED));
        }
    }
    if file.editable || shown.len() <= VIRTUAL_PREVIEW_THRESHOLD {
        editor(ui, d, file, revision, reveal, track_scroll, old_unicode)
    } else {
        d.folds.update(revision, file.file, shown, file.prefs.tab);
        if reveal && let Some(hit) = d.find.hit() {
            d.folds.reveal(hit.line);
        }
        file_lines(
            ui,
            file.file,
            shown,
            file.wrap,
            file.dark,
            file.prefs,
            revision,
            d.find.hit(),
            reveal,
            FileLines {
                jump: file.jump,
                folds: Some(&mut d.folds),
                ..Default::default()
            },
        )
    }
}

pub(super) fn split_ratio(ui: &Ui, id: egui::Id, rect: Rect) -> f32 {
    let id = id.with("split_size");
    let mut ratio: f32 = ui.data_mut(|data| data.get_temp(id)).unwrap_or(0.5);
    let x = rect.left() + rect.width() * ratio;
    let handle = Rect::from_min_max(pos2(x - 4.0, rect.top()), pos2(x + 4.0, rect.bottom()));
    let response = ui
        .interact(handle, id, Sense::click_and_drag())
        .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
    response.widget_info(|| {
        egui::WidgetInfo::slider(ui.is_enabled(), f64::from(ratio), "Resize comparison panes")
    });
    if response.clicked() {
        response.request_focus();
    }
    if response.dragged()
        && let Some(pointer) = response.interact_pointer_pos()
        && rect.width() > 0.0
    {
        ratio = (pointer.x - rect.left()) / rect.width();
    }
    if response.has_focus() {
        ratio += ui.input_mut(|input| {
            if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft) {
                -0.025
            } else if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight) {
                0.025
            } else {
                0.0
            }
        });
    }
    ratio = ratio.clamp(0.23, 0.77);
    ui.data_mut(|data| data.insert_temp(id, ratio));
    ratio
}

#[allow(clippy::too_many_arguments)]
pub(super) fn edit_comparison(
    ui: &mut Ui,
    id: egui::Id,
    mode: &str,
    original: &str,
    comparison: Option<&Comparison>,
    previous: Option<&Comparison>,
    hl: &(bool, Vec<Runs>, Vec<Runs>),
    old_unicode: Option<&Report>,
    mut file: FileEditor<'_>,
    d: &mut DiffUi,
    nav: (bool, bool),
) {
    let mut old_jump = None;
    let [removed, added] = comparison.map_or([&[][..]; 2], |cmp| {
        [&cmp.split_index.changed[0], &cmp.split_index.changed[1]]
    });
    if let Some(cmp) = comparison
        && (nav.0 || nav.1)
        && !cmp.split_changes.is_empty()
    {
        let last = cmp.split_changes.len() - 1;
        let at = d
            .comparison_change
            .map(|i| {
                if nav.0 {
                    (i + 1) % (last + 1)
                } else if i > 0 {
                    (i - 1).min(last)
                } else {
                    last
                }
            })
            .unwrap_or(if nav.0 { 0 } else { last });
        d.comparison_change = Some(at);
        let row = cmp.split_changes[at];
        old_jump = cmp.split[row..]
            .iter()
            .find_map(|r| r.left)
            .or_else(|| cmp.old.len().checked_sub(1));
        file.jump = cmp.split[row..]
            .iter()
            .find_map(|r| r.right)
            .or_else(|| cmp.new.len().checked_sub(1));
    }
    if mode != "split" {
        let mut old_lines = Vec::new();
        if let Some(cmp) = comparison {
            let mut before = None;
            for row in cmp.rows.iter().rev() {
                if let Some(new) = row.new {
                    before = Some(new);
                } else if let Some(old) = row.old {
                    old_lines.push(folding::RemovedLine {
                        before,
                        number: old,
                        text: &cmp.old[old],
                    });
                }
            }
            old_lines.reverse();
        }
        show_file_editor(
            ui,
            d,
            FileEditor {
                changed: added,
                removed: &old_lines,
                old_runs: &hl.1,
                ..file
            },
            false,
            old_unicode,
        );
        return;
    }

    // Keep the last alignment while the background comparison catches up with typing.
    let scroll_comparison = comparison.or(previous);
    let full = ui.available_rect_before_wrap();
    let (rect, overview) = split_view::reserve_ruler(full);
    let x = rect.left() + rect.width() * split_ratio(ui, id, rect);
    let half_gap = (rect.width() * 0.04).clamp(0.0, 3.0);
    let left = Rect::from_min_max(rect.min, pos2(x - half_gap, rect.bottom()));
    let right = Rect::from_min_max(pos2(x + half_gap, rect.top()), rect.max);
    let mut old = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("split_original")
            .max_rect(left),
    );
    let mut modified = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("split_modified")
            .max_rect(right),
    );
    old.set_clip_rect(ui.clip_rect().intersect(left));
    modified.set_clip_rect(ui.clip_rect().intersect(right));
    ui.painter()
        .vline(x, rect.y_range(), Stroke::new(1.0, p().line));
    old.label("Original");
    modified.label("Modified");

    // Retain the exactly aligned rows used for small unwrapped edits. Large or wrapped
    // documents stay in their own layouts and scroll together by corresponding logical line.
    let aligned = !file.wrap
        && !d.folds.has_collapsed()
        && d.draft
            .as_deref()
            .unwrap_or(file.base)
            .len()
            .max(original.len())
            <= VIRTUAL_PREVIEW_THRESHOLD
        && scroll_comparison.is_some();
    let mut rows = Vec::new();
    let mut gaps = Vec::new();
    if aligned && let Some(cmp) = scroll_comparison {
        let mut before = None;
        for row in cmp.split.iter().rev() {
            if let Some(new) = row.right {
                before = Some(new);
            } else if let Some(old) = row.left {
                gaps.push(folding::RemovedLine {
                    before,
                    number: old,
                    text: "",
                });
            }
            rows.push(row.left);
        }
        gaps.reverse();
        rows.reverse();
    }
    let sync_key = id.with(("edit_split_scroll", d.editor));
    let mut sync: Sync = ui
        .data_mut(|data| data.get_temp(sync_key))
        .unwrap_or_default();
    let [old_pending, new_pending] = sync.input();
    let old_pane = file_lines(
        &mut old,
        file.file,
        original,
        file.wrap,
        file.dark,
        file.prefs,
        (file.generation, None),
        None,
        false,
        FileLines {
            removed,
            aligned: aligned.then_some(rows.as_slice()),
            jump: old_jump,
            scroll: old_pending,
            row_height: Some(editor_line_height(&file.prefs.font)),
            folds: None,
        },
    );
    let new_pane = show_file_editor(
        &mut modified,
        d,
        FileEditor {
            changed: added,
            removed: &gaps,
            scroll: new_pending,
            spacers: true,
            ..file
        },
        !aligned,
        None,
    );
    let panes = [&old_pane, &new_pane];
    let layout = id.with((
        (
            file.generation,
            d.edits,
            d.folds.generation(),
            aligned,
            file.wrap,
        ),
        scroll_comparison.map(|cmp| cmp.split.as_ptr().addr()),
        panes.map(|p| [p.scroll.viewport.to_bits(), p.scroll.total.to_bits()]),
        [
            left.width().to_bits(),
            right.width().to_bits(),
            ui.pixels_per_point().to_bits(),
        ],
    ));
    let preferred = usize::from(old_jump.is_some() || !ui.rect_contains_pointer(left));
    let map = |from: usize, to: usize, y: f32| {
        if aligned || y <= 0.0 {
            return y;
        }
        let anchor = panes[from].anchor(y);
        scroll_comparison.map_or_else(
            || panes[to].offset(anchor.0, anchor.1),
            |cmp| {
                // Before the comparison catches up with an edit, the other pane stays where it is.
                match split_view::mapped_position(&cmp.split_index, from, anchor, cmp.split.len()) {
                    Some(diff) => split_view::pane_offset(&cmp.split_index, to, diff, panes[to]),
                    None => panes[to].scroll.top,
                }
            },
        )
    };
    if sync.update(panes, layout, preferred, map) {
        ui.ctx().request_repaint();
    }
    if let Some(cmp) = comparison {
        let leading = sync.leader(panes);
        let leader = panes[leading];
        let rows = cmp.split.len().max(1) as f32;
        let at = |y| {
            if aligned {
                y / editor_line_height(&file.prefs.font)
            } else {
                split_view::diff_position(
                    &cmp.split_index,
                    leading,
                    leader.anchor(y),
                    cmp.split.len(),
                )
            }
        };
        let viewport = (
            at(leader.scroll.top) / rows,
            at(leader.scroll.top + leader.scroll.viewport) / rows,
        );
        if let Some(fraction) = split_view::ruler(
            ui,
            id,
            overview,
            &cmp.split_index,
            viewport,
            layout,
            |row| row as f32 / rows,
        ) {
            let targets = [0, 1].map(|side| {
                let y = if aligned {
                    fraction * rows * editor_line_height(&file.prefs.font)
                } else {
                    split_view::pane_offset(&cmp.split_index, side, fraction * rows, panes[side])
                };
                panes[side].scroll.clamp(y)
            });
            sync.jump(targets, leading);
            ui.ctx().request_repaint();
        }
    }
    ui.data_mut(|data| data.insert_temp(sync_key, sync));
}

/// The byte where character `at` starts.
fn byte_at(text: &str, at: usize) -> usize {
    text.char_indices().nth(at).map_or(text.len(), |(b, _)| b)
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct PresentationKey {
    revision: FileRevision,
    folds: u64,
    removed: u64,
}

struct EditorPresentation {
    key: PresentationKey,
    positions: Arc<folding::Positions>,
    text: Option<String>,
}

fn fingerprint(value: &(impl Hash + ?Sized)) -> u64 {
    let mut hash = DefaultHasher::new();
    value.hash(&mut hash);
    hash.finish()
}

fn presentation(
    cached: &mut Option<Arc<EditorPresentation>>,
    revision: FileRevision,
    text: &str,
    folds: &folding::Folds,
    removed: &[folding::RemovedLine<'_>],
) -> Arc<EditorPresentation> {
    let key = PresentationKey {
        revision,
        folds: folds.generation(),
        removed: fingerprint(removed),
    };
    if let Some(cached) = cached.as_ref().filter(|cached| cached.key == key) {
        return cached.clone();
    }
    let visible = folding::Projection::with_removed(text, folds, removed);
    let next = Arc::new(EditorPresentation {
        key,
        positions: Arc::new(visible.positions),
        text: match visible.text {
            Cow::Borrowed(_) => None,
            Cow::Owned(text) => Some(text),
        },
    });
    *cached = Some(next.clone());
    next
}

struct SplitLineCache {
    galley: Weak<egui::Galley>,
    projection: Arc<EditorPresentation>,
    tops: Arc<Vec<(usize, f32)>>,
}

impl SplitLineCache {
    fn measure(galley: &Arc<egui::Galley>, projection: &Arc<EditorPresentation>) -> Self {
        let mut row = 0;
        let mut chars = 0;
        let mut tops = Vec::with_capacity(projection.positions.lines.len());
        for line in &projection.positions.lines {
            while let Some(visual) = galley.rows.get(row) {
                let next = chars + visual.char_count_including_newline().0;
                if next > line.start || row + 1 == galley.rows.len() {
                    if line.removed.is_none() {
                        tops.push((line.original, visual.min_y()));
                    }
                    break;
                }
                chars = next;
                row += 1;
            }
        }
        Self {
            galley: Arc::downgrade(galley),
            projection: projection.clone(),
            tops: Arc::new(tops),
        }
    }
}

#[derive(Default)]
pub(in crate::app) struct EditorCache {
    presentation: Option<Arc<EditorPresentation>>,
    layout_job: Option<(LayoutKey, Arc<egui::text::LayoutJob>)>,
    syntax: ParseCache<FileHighlightKey, Vec<Runs>>,
    unicode: ParseCache<FileRevision, Report>,
    brackets: ParseCache<FileRevision, std::collections::BTreeMap<usize, usize>>,
    wrapped_rows: Option<(Arc<egui::Galley>, Vec<usize>)>,
    split_lines: Option<SplitLineCache>,
    cursor_history: CursorHistory,
}

fn visible_editor_rows(
    out: &egui::text_edit::TextEditOutput,
    cached: &mut Option<(Arc<egui::Galley>, Vec<usize>)>,
    positions: &folding::Positions,
    wrap: bool,
    clip: Rect,
) -> (Range<usize>, Range<usize>) {
    let rows = &out.galley.rows;
    let start = rows.partition_point(|row| row.max_y() + out.galley_pos.y < clip.top());
    let end = rows.partition_point(|row| row.min_y() + out.galley_pos.y <= clip.bottom());
    let starts = if !wrap && rows.len() == positions.lines.len() {
        None
    } else {
        if cached
            .as_ref()
            .is_none_or(|(galley, _)| !Arc::ptr_eq(galley, &out.galley))
        {
            let mut chars = 0;
            let offsets = rows
                .iter()
                .map(|row| {
                    let first = chars;
                    chars += row.char_count_including_newline().0;
                    first
                })
                .collect();
            *cached = Some((out.galley.clone(), offsets));
        }
        cached.as_ref().map(|(_, offsets)| offsets)
    };
    let char_at = |row| {
        if let Some(starts) = starts {
            starts.get(row).copied()
        } else {
            positions
                .lines
                .get(row)
                .map(|line: &folding::Line| line.start)
        }
        .unwrap_or(usize::MAX)
    };
    (start..end, char_at(start)..char_at(end))
}

#[derive(PartialEq)]
struct LayoutKey {
    presentation: PresentationKey,
    font: FontId,
    dark: bool,
    syntax: bool,
    compose_emoji: bool,
    spacers: bool,
    added: u64,
    hit: Option<(usize, usize)>,
    ink: egui::Color32,
    base: egui::Color32,
}

fn plain_job(source: &str, font: &FontId, ink: egui::Color32) -> egui::text::LayoutJob {
    let height = editor_line_height(font);
    let mut job = egui::text::LayoutJob {
        first_row_min_height: height,
        ..Default::default()
    };
    job.append(
        source,
        0.0,
        egui::TextFormat {
            line_height: Some(height),
            ..egui::TextFormat::simple(font.clone(), ink)
        },
    );
    job
}

#[allow(clippy::too_many_arguments)]
fn editor_job(
    source: &str,
    positions: &folding::Positions,
    runs: &[Runs],
    old_runs: &[Runs],
    added: &[bool],
    spacers: bool,
    hit: Option<&super::find::Match>,
    font: &FontId,
    base: egui::Color32,
    ink: egui::Color32,
) -> egui::text::LayoutJob {
    if runs.is_empty()
        && old_runs.is_empty()
        && added.is_empty()
        && hit.is_none()
        && !positions.has_removed()
    {
        return plain_job(source, font, base);
    }
    let height = editor_line_height(font);
    let mut job = egui::text::LayoutJob {
        first_row_min_height: height,
        ..Default::default()
    };
    let mut lines = source.split('\n').peekable();
    let mut row = 0;
    while let Some(line) = lines.next() {
        let position = positions.lines.get(row);
        let original = position.map_or(row, |line| line.original);
        let old = position.and_then(|line| line.removed);
        let mark = old
            .is_none()
            .then(|| hit.and_then(|hit| hit.mark(original, line)))
            .flatten();
        let background = if old.is_some() && !spacers {
            Some((0..line.len(), BAD.gamma_multiply(0.23)))
        } else {
            (added.get(original) == Some(&true))
                .then_some((0..line.len(), GOOD.gamma_multiply(0.2)))
        };
        let marks: Vec<_> = mark.into_iter().chain(background).collect();
        let runs = if let Some(old) = old {
            old_runs.get(old)
        } else {
            runs.get(original)
        };
        let part = highlight::job(line, runs, base, font.clone(), &marks);
        for section in part.sections {
            let (a, b) = (section.byte_range.start.0, section.byte_range.end.0);
            job.append(
                &part.text[a..b],
                0.0,
                egui::TextFormat {
                    line_height: Some(height),
                    ..section.format
                },
            );
        }
        if lines.peek().is_some() {
            job.append(
                "\n",
                0.0,
                egui::TextFormat {
                    line_height: Some(height),
                    ..egui::TextFormat::simple(font.clone(), ink)
                },
            );
        }
        row += 1;
    }
    job
}

/// The editor: the working-folder copy with colours; Ctrl+S saves.
#[allow(clippy::too_many_arguments)]
fn editor(
    ui: &mut Ui,
    d: &mut DiffUi,
    view: FileEditor<'_>,
    revision: FileRevision,
    reveal: bool,
    track_scroll: bool,
    old_unicode: Option<&Report>,
) -> Pane {
    let FileEditor {
        file,
        base,
        wrap,
        dark,
        prefs,
        editable,
        changed: added,
        jump,
        mut removed,
        old_runs,
        scroll,
        spacers,
        ..
    } = view;
    let c = p();
    let had_draft = d.draft.is_some();
    let mut text = d.draft.take().map_or(Cow::Borrowed(base), Cow::Owned);
    let id = editor_id(d.editor);
    let folded_id = id.with("folded");
    let hit = d.find.hit();
    let mut changed = false;
    let mut reveal_caret = false;
    let mut restore_folds = None;
    let mut restore_scroll = None;
    let scope = ui.scope_id();
    d.editor_cache
        .cursor_history
        .update_revision(scope, revision);
    {
        d.folds.update(revision, file, &text, prefs.tab);
        let folded_focus =
            d.editor_positions.is_some() && ui.memory(|memory| memory.has_focus(folded_id));
        let expand_to_edit = folded_focus
            && !crate::app::dialog_open(ui.ctx())
            && ui.input(|input| {
                input.events.iter().any(|event| match event {
                    egui::Event::Key {
                        key,
                        modifiers,
                        pressed: true,
                        ..
                    } => {
                        editing::shortcut(*key, *modifiers)
                            || d.cursors.multiple() && editing::navigation(*key)
                            || (editable
                                && matches!(
                                    key,
                                    egui::Key::Enter
                                        | egui::Key::Backspace
                                        | egui::Key::Delete
                                        | egui::Key::Tab
                                        | egui::Key::Z
                                        | egui::Key::Y
                                ))
                    }
                    egui::Event::Text(_)
                    | egui::Event::Paste(_)
                    | egui::Event::Cut
                    | egui::Event::Ime(_) => editable,
                    _ => false,
                })
            });
        if expand_to_edit {
            if d.folds.has_collapsed()
                && !ui.input(|input| {
                    input
                        .events
                        .iter()
                        .any(|event| matches!(event, egui::Event::Ime(_)))
                })
            {
                restore_folds = Some((text.to_string(), d.folds.snapshot(&text)));
            }
            d.folds.unfold_all();
            removed = &[];
        } else if reveal && let Some(hit) = hit {
            d.folds.reveal(hit.line);
        } else if let Some(line) = jump {
            d.folds.reveal(line);
        }
        let projection = presentation(
            &mut d.editor_cache.presentation,
            revision,
            &text,
            &d.folds,
            removed,
        );
        let target = if projection.text.is_some() {
            folded_id
        } else {
            id
        };
        if let Some(previous) = d.editor_positions.take() {
            if (projection.text.is_none() || !Arc::ptr_eq(&previous, &projection.positions))
                && let Some(cursor) = egui::TextEdit::load_state(ui.ctx(), folded_id)
                    .and_then(|state| state.cursor.char_range())
            {
                let canonical = previous.in_document(cursor);
                set_editor_selection(
                    ui,
                    target,
                    projection.positions.in_view(canonical),
                    folded_focus,
                );
            }
        } else if projection.text.is_some()
            && let Some(cursor) =
                egui::TextEdit::load_state(ui.ctx(), id).and_then(|state| state.cursor.char_range())
        {
            let focus = ui.memory(|memory| memory.has_focus(id));
            set_editor_selection(ui, target, projection.positions.in_view(cursor), focus);
        }
        let history = &mut d.editor_cache.cursor_history;
        let undo_key = ui.memory(|memory| memory.has_focus(target))
            && !crate::app::dialog_open(ui.ctx())
            && ui.input_mut(|input| {
                let mut undo = false;
                input.events.retain(|event| {
                    let egui::Event::Key {
                        key: egui::Key::U,
                        pressed: true,
                        modifiers,
                        ..
                    } = event
                    else {
                        return true;
                    };
                    if modifiers.matches_exact(egui::Modifiers::COMMAND) {
                        undo = true;
                        return false;
                    }
                    // Windows AltGr can report Ctrl+Alt with a separate Text event.
                    cfg!(target_os = "macos") || !modifiers.ctrl
                });
                undo
            });
        let action = history
            .request
            .take()
            .or(undo_key.then_some(Direction::Undo));
        if let Some(selection) = action.and_then(|action| history.restore(action)) {
            set_editor_selection(
                ui,
                target,
                projection.positions.in_view(selection.primary),
                true,
            );
            d.cursors.restore_secondary(selection.secondary);
            restore_scroll = Some(selection.scroll);
        }
        if projection.text.is_some()
            && ui.memory(|memory| memory.has_focus(folded_id))
            && ui.input_mut(|input| {
                let before = input.events.len();
                input
                    .events
                    .retain(|event| !matches!(event, egui::Event::Copy));
                input.events.len() != before
            })
            && let Some(cursor) = egui::TextEdit::load_state(ui.ctx(), folded_id)
                .and_then(|state| state.cursor.char_range())
        {
            if removed.is_empty() || spacers || d.cursors.multiple() {
                editing::copy(
                    ui,
                    file,
                    &text,
                    projection.positions.in_document(cursor),
                    &d.cursors,
                    dark,
                    prefs.syntax,
                );
            } else {
                editing::copy(
                    ui,
                    file,
                    projection.text.as_deref().unwrap_or(&text),
                    cursor,
                    &d.cursors,
                    dark,
                    prefs.syntax,
                );
            }
        }
        if projection.text.is_none() {
            let effect = editing::input(
                ui,
                id,
                file,
                &mut text,
                &mut d.cursors,
                prefs,
                dark,
                editable,
            );
            changed = effect.changed;
            reveal_caret = effect.reveal_caret;
        }
    }
    let revision = if changed {
        (revision.0, Some(d.edits + 1))
    } else {
        revision
    };
    let syntax = syntax_runs(
        &mut d.editor_cache.syntax,
        ui.ctx(),
        (revision, dark, prefs.syntax),
        file,
        &text,
    );
    let unicode = d
        .editor_cache
        .unicode
        .get(ui.ctx(), revision, file, &text, Report::scan);
    if unicode.as_ref().is_some_and(|report| report.more)
        || old_unicode.is_some_and(|report| report.more)
    {
        unicode_highlight::caption(ui);
    }
    let runs = syntax.as_deref().map_or(&[][..], Vec::as_slice);
    let font = prefs.font.clone();
    let projection = presentation(
        &mut d.editor_cache.presentation,
        revision,
        &text,
        &d.folds,
        removed,
    );
    d.editor_positions = projection
        .text
        .as_ref()
        .map(|_| projection.positions.clone());
    let find_cursor = hit
        .filter(|_| reveal)
        .and_then(|hit| text.get(..hit.start()))
        .map(|before| {
            let point =
                egui::text::CCursorRange::one(egui::text::CCursor::new(before.chars().count()));
            projection.positions.in_view(point).primary
        })
        .or_else(|| {
            jump.and_then(|line| {
                projection
                    .positions
                    .lines
                    .iter()
                    .find(|row| row.original == line && row.removed.is_none())
                    .map(|row| egui::text::CCursor::new(row.start))
            })
        });
    let mutations = Cell::new(0);
    let compose_emoji = crate::app::fonts::use_emoji_font(ui.ctx());
    let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, width: f32| {
        let source = buf.as_str();
        let mut job = if mutations.get() == 0 {
            let key = LayoutKey {
                presentation: projection.key,
                font: font.clone(),
                dark,
                syntax: !runs.is_empty(),
                compose_emoji,
                spacers,
                added: fingerprint(added),
                hit: hit.map(|hit| (hit.start(), hit.len())),
                ink: c.fg,
                base: c.white(0.85),
            };
            if d.editor_cache
                .layout_job
                .as_ref()
                .is_none_or(|(old, _)| *old != key)
            {
                let mut job = editor_job(
                    source,
                    &projection.positions,
                    runs,
                    old_runs,
                    added,
                    spacers,
                    hit,
                    &font,
                    c.white(0.85),
                    c.fg,
                );
                crate::app::fonts::compose_emoji(&mut job, compose_emoji);
                d.editor_cache.layout_job = Some((key, Arc::new(job)));
            }
            (*d.editor_cache.layout_job.as_ref().expect("set above").1).clone()
        } else {
            // The widget just applied input. The next frame colors the new document revision.
            let mut job = plain_job(source, &font, c.white(0.85));
            crate::app::fonts::compose_emoji(&mut job, compose_emoji);
            job
        };
        if wrap {
            job.wrap.max_width = width;
        }
        ui.fonts_mut(|fonts| fonts.layout_job(job))
    };
    let lines = projection.positions.lines.len().max(1);
    let folded = projection.text.is_some();
    let active_id = if folded { folded_id } else { id };
    // The unmodified document stays borrowed. Only the editable buffer can make it owned.
    let mut display = projection.text.as_deref().unwrap_or_default();
    let mut area = egui::ScrollArea::both()
        .id_salt(("editor", d.editor))
        .auto_shrink([false, false]);
    if let Some(offset) = scroll {
        area = area.vertical_scroll_offset(offset);
    }
    if let Some(offset) = restore_scroll {
        area = area.scroll_offset(offset);
    }
    let mut observed_cursor = None;
    let mut lines_in_pane = LineTops::Fixed(editor_line_height(&font));
    let mut inset = 0.0;
    let output = area.show(ui, |ui| {
        let origin = ui.cursor().top();
        ui.horizontal_top(|ui| {
            let gutter_width = if prefs.numbers {
                GUTTER
            } else if d.folds.any() {
                18.0
            } else {
                0.0
            };
            let (gutter, _) = ui.allocate_exact_size(vec2(gutter_width, 0.0), Sense::hover());
            let cursor_before = egui::TextEdit::load_state(ui.ctx(), active_id)
                .and_then(|state| state.cursor.char_range());
            let mut working;
            let mut unmodified;
            let buffer: &mut dyn egui::TextBuffer = if folded {
                &mut display
            } else if editable {
                working = editing::DocumentBuffer {
                    text: &mut text,
                    crlf: prefs.crlf,
                    mutations: &mutations,
                };
                &mut working
            } else {
                unmodified = text.as_ref();
                &mut unmodified
            };
            let edit = egui::TextEdit::multiline(buffer)
                .id(active_id)
                .code_editor()
                .font(font.clone())
                .frame(egui::Frame::NONE)
                .desired_width(f32::INFINITY)
                .desired_rows(lines)
                .layouter(&mut layouter);
            let mut out = edit.show(ui);
            if track_scroll {
                inset = (out.galley_pos.y - origin).max(0.0);
                if wrap || folded {
                    let galley = Arc::downgrade(&out.galley);
                    if d.editor_cache.split_lines.as_ref().is_none_or(|old| {
                        !old.galley.ptr_eq(&galley) || !Arc::ptr_eq(&old.projection, &projection)
                    }) {
                        d.editor_cache.split_lines =
                            Some(SplitLineCache::measure(&out.galley, &projection));
                    }
                    lines_in_pane = LineTops::Editor(
                        d.editor_cache
                            .split_lines
                            .as_ref()
                            .expect("measured above")
                            .tops
                            .clone(),
                    );
                }
            }
            if let Some(cursor) = find_cursor {
                let position = out
                    .galley
                    .pos_from_cursor(cursor)
                    .translate(out.galley_pos.to_vec2());
                ui.scroll_to_rect(position, Some(egui::Align::Center));
            } else if reveal_caret && let Some(cursor) = out.cursor_range {
                let position = out
                    .galley
                    .pos_from_cursor(cursor.primary)
                    .translate(out.galley_pos.to_vec2());
                ui.scroll_to_rect(position, None);
            }
            let clip = ui.clip_rect();
            let (visible_rows, visible_chars) = visible_editor_rows(
                &out,
                &mut d.editor_cache.wrapped_rows,
                &projection.positions,
                wrap,
                clip,
            );
            if !folded {
                d.cursors.pointer(ui, &mut out, cursor_before);
            }
            d.cursors.paint(
                ui,
                &out,
                ACCENT,
                folded.then_some(projection.positions.as_ref()),
                visible_rows.clone(),
                visible_chars.clone(),
            );
            if !changed
                && !out.response.changed()
                && out.response.has_focus()
                && let Some(cursor) = out.cursor_range.filter(|cursor| cursor.is_empty())
            {
                paint_matching_brackets(
                    ui,
                    &out,
                    file,
                    &text,
                    revision,
                    &projection.positions,
                    cursor,
                    &mut d.editor_cache.brackets,
                );
            }
            let mut toggle = None;
            let painter = ui.painter().with_clip_rect(clip);
            let visible_start = projection
                .positions
                .lines
                .partition_point(|line| line.start < visible_chars.start);
            if mutations.get() == 0 && (unicode.is_some() || old_unicode.is_some()) {
                // The first visible wrapped row may be a continuation of a logical line.
                let visible_end = projection
                    .positions
                    .lines
                    .partition_point(|line| line.start < visible_chars.end);
                for line in
                    &projection.positions.lines[visible_start.saturating_sub(1)..visible_end]
                {
                    let (report, row) = if let Some(old) = line.removed {
                        (if spacers { None } else { old_unicode }, old)
                    } else {
                        (unicode.as_deref(), line.original)
                    };
                    if let Some(report) = report {
                        unicode_highlight::paint(
                            ui,
                            active_id.with("unicode"),
                            &painter,
                            &out.galley,
                            out.galley_pos,
                            report.line(row),
                            line.start,
                            visible_chars.clone(),
                            prefs.syntax,
                            unicode_highlight::ink(dark),
                        );
                    }
                }
            }
            for (i, line) in projection.positions.lines[visible_start..]
                .iter()
                .take_while(|line| line.start < visible_chars.end)
                .enumerate()
            {
                let rect = if !wrap && out.galley.rows.len() == lines {
                    let row = &out.galley.rows[visible_start + i];
                    Rect::from_min_max(pos2(0.0, row.min_y()), pos2(0.0, row.max_y()))
                } else {
                    let point = egui::text::CCursor::new(line.start);
                    let row = &out.galley.rows[out.galley.layout_from_cursor(point).row];
                    Rect::from_min_max(pos2(0.0, row.min_y()), pos2(0.0, row.max_y()))
                }
                .translate(out.galley_pos.to_vec2());
                if prefs.numbers && (!spacers || line.removed.is_none()) {
                    let number = line.removed.unwrap_or(line.original) + 1;
                    let ink = if line.removed.is_some() {
                        REMOVED
                    } else {
                        c.white(0.33)
                    };
                    painter.text(
                        pos2(gutter.right() - 2.0, rect.center().y),
                        Align2::RIGHT_CENTER,
                        number.to_string(),
                        font.clone(),
                        ink,
                    );
                }
                if line.removed.is_none() && d.folds.foldable(line.original) {
                    let area = Rect::from_center_size(
                        pos2(gutter.left() + 8.0, rect.center().y),
                        vec2(16.0, rect.height()),
                    );
                    let button = ui.interact(
                        area,
                        active_id.with(("fold", line.original)),
                        Sense::click(),
                    );
                    let collapsed = d.folds.collapsed(line.original);
                    let label = if collapsed { "Unfold" } else { "Fold" };
                    button.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::Button,
                            ui.is_enabled(),
                            format!("{label} line {}", line.original + 1),
                        )
                    });
                    widgets::chevron(
                        ui,
                        area,
                        !collapsed,
                        12.0,
                        if button.hovered() || collapsed {
                            c.fg
                        } else {
                            c.fg3
                        },
                    );
                    if button.clicked() {
                        toggle = Some(line.original);
                    }
                }
            }
            observed_cursor = out.state.cursor.char_range();
            let mut fold_all = None;
            let active_selection = out.cursor_range.or(cursor_before);
            {
                out.response.context_menu(|ui| {
                    let history = &mut d.editor_cache.cursor_history;
                    for (label, action) in [
                        ("Cursor Undo", Direction::Undo),
                        ("Cursor Redo", Direction::Redo),
                    ] {
                        if ui
                            .add_enabled(history.enabled(action), egui::Button::new(label))
                            .clicked()
                        {
                            history.request = Some(action);
                            ui.ctx().request_repaint();
                            ui.close();
                        }
                    }
                    if let Some(cursor) = active_selection
                        && ui.button("Copy With Syntax Highlighting").clicked()
                    {
                        if removed.is_empty() || spacers || d.cursors.multiple() {
                            editing::copy(
                                ui,
                                file,
                                &text,
                                projection.positions.in_document(cursor),
                                &d.cursors,
                                dark,
                                prefs.syntax,
                            );
                        } else {
                            editing::copy(
                                ui,
                                file,
                                projection.text.as_deref().unwrap_or(&text),
                                cursor,
                                &d.cursors,
                                dark,
                                prefs.syntax,
                            );
                        }
                        ui.close();
                    }
                    if d.folds.any() {
                        if ui.button("Fold All").clicked() {
                            fold_all = Some(true);
                            ui.close();
                        }
                        if ui
                            .add_enabled(d.folds.has_collapsed(), egui::Button::new("Unfold All"))
                            .clicked()
                        {
                            fold_all = Some(false);
                            ui.close();
                        }
                    }
                });
            }
            if let Some(line) = toggle {
                d.folds.toggle(line);
            }
            match fold_all {
                Some(true) => d.folds.fold_all(),
                Some(false) => d.folds.unfold_all(),
                None => {}
            }
            if toggle.is_some() || fold_all.is_some() {
                d.cursors.clear();
                ui.memory_mut(|memory| memory.request_focus(active_id));
                ui.ctx().request_repaint();
            }
            if editable && !folded && out.response.changed() {
                if let Some(cursor) = out.cursor_range {
                    out.state
                        .cursor
                        .set_char_range(Some(editing::document_cursor(&text, cursor)));
                    observed_cursor = out.state.cursor.char_range();
                    out.state.store(ui.ctx(), id);
                }
                changed = true;
            }
        });
    });
    let final_revision = if changed {
        (revision.0, Some(d.edits + 1))
    } else {
        revision
    };
    d.editor_cache
        .cursor_history
        .update_revision(scope, final_revision);
    if let Some(cursor) = observed_cursor {
        let primary = if folded {
            projection.positions.in_document(cursor)
        } else {
            cursor
        };
        d.editor_cache.cursor_history.observe(
            primary,
            d.cursors.secondary(),
            output.state.clamped_offset(),
            restore_scroll.is_some(),
        );
    }
    if changed {
        let working = text.into_owned();
        if let Some((before, snapshot)) = restore_folds {
            d.folds
                .update((revision.0, Some(d.edits + 1)), file, &working, prefs.tab);
            d.folds.restore_after_edit(&before, &working, snapshot);
            if d.folds.has_collapsed()
                && let Some(cursor) = egui::TextEdit::load_state(ui.ctx(), id)
                    .and_then(|state| state.cursor.char_range())
            {
                let byte = byte_at(&working, cursor.primary.index.0);
                let row = working[..byte].bytes().filter(|&b| b == b'\n').count();
                d.folds.reveal(row);
            }
        }
        d.draft = Some(working);
        d.edits += 1;
    } else {
        // A command that only moved the caret (such as going to the matching bracket) leaves the text alone: the
        // folds opened for it come back, except the one hiding the caret.
        if let Some((before, snapshot)) = restore_folds {
            d.folds.restore_after_edit(&before, &text, snapshot);
            if d.folds.has_collapsed()
                && let Some(cursor) = egui::TextEdit::load_state(ui.ctx(), id).and_then(|state| state.cursor.char_range())
            {
                let byte = byte_at(&text, cursor.primary.index.0);
                let row = text[..byte].bytes().filter(|&b| b == b'\n').count();
                d.folds.reveal(row);
            }
        }
        if had_draft {
            d.draft = Some(text.into_owned());
        }
    }
    Pane {
        scroll: Scroll::from_output(&output),
        lines: lines_in_pane,
        inset,
    }
}

fn set_editor_selection(ui: &Ui, id: egui::Id, cursor: egui::text::CCursorRange, focus: bool) {
    let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
    state.cursor.set_char_range(Some(cursor));
    state.store(ui.ctx(), id);
    if focus {
        ui.memory_mut(|memory| memory.request_focus(id));
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_matching_brackets(
    ui: &Ui,
    out: &egui::text_edit::TextEditOutput,
    file: &str,
    text: &str,
    revision: FileRevision,
    projection: &folding::Positions,
    cursor: egui::text::CCursorRange,
    cache: &mut ParseCache<FileRevision, std::collections::BTreeMap<usize, usize>>,
) {
    let byte = byte_at(text, projection.in_document(cursor).primary.index.0);
    let is_bracket = |byte| {
        text.as_bytes()
            .get(byte)
            .is_some_and(|b| b"()[]{}".contains(b))
    };
    if !is_bracket(byte) && !byte.checked_sub(1).is_some_and(is_bracket) {
        return;
    }
    let Some(pairs) = cache.get(ui.ctx(), revision, file, text, highlight::brackets) else {
        return;
    };
    let Some((a, b)) = highlight::bracket_at(&pairs, byte) else {
        return;
    };
    let painter = ui.painter().with_clip_rect(out.text_clip_rect);
    for at in [a, b] {
        let start = text[..at].chars().count();
        let source = egui::text::CCursorRange::one(egui::text::CCursor::new(start));
        let visible = projection.in_view(source);
        if projection.in_document(visible) == source {
            let left = out.galley.pos_from_cursor(visible.primary);
            let right = out
                .galley
                .pos_from_cursor(egui::text::CCursor::new(visible.primary.index.0 + 1));
            let rect = left.union(right).translate(out.galley_pos.to_vec2());
            painter.rect_stroke(rect, 2, Stroke::new(1.0, ACCENT), StrokeKind::Inside);
        }
    }
}

fn json_syntax_error(text: &str) -> Option<String> {
    serde_json::from_str::<serde::de::IgnoredAny>(text)
        .err()
        .map(|error| error.to_string())
}

#[cfg(test)]
#[test]
fn headless_json_diagnostics_report_syntax_errors() {
    assert!(json_syntax_error("{\"value\":[1, true, null]}").is_none());
    assert!(
        json_syntax_error("{\n\"value\": [1,]\n}")
            .unwrap()
            .contains("line 2")
    );
    assert!(json_syntax_error("{} {}").is_some());
}

#[cfg(test)]
mod editor_tests {
    use super::{DiffUi, EditorPrefs, FileEditor, editor_id, file_editor};

    #[test]
    fn headless_editor_multicursor_change_undoes_as_a_single_edit() {
        check_multicursor_undo("let test = test;\r\n");
    }

    #[test]
    fn headless_large_editor_multicursor_edit_and_undo() {
        let filler = "// caf\u{e9} abcdefghijklmnopqrstuvwxyz\r\n".repeat(16_000);
        let text = format!("let test = test;\r\n{filler}let test = test;\r\n");
        assert!(text.len() > super::VIRTUAL_PREVIEW_THRESHOLD);
        check_multicursor_undo(&text);
    }

    fn check_multicursor_undo(base: &str) {
        use egui::{Event, Key, Modifiers};
        let ctx = egui::Context::default();
        let mut draft = DiffUi {
            editor: 19,
            ..Default::default()
        };
        let id = editor_id(draft.editor);
        let prefs = EditorPrefs {
            font: egui::FontId::monospace(12.0),
            tab: 4,
            crlf: true,
            syntax: false,
            numbers: true,
        };
        let send = |draft: &mut DiffUi, events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    file_editor(
                        ui,
                        draft,
                        FileEditor {
                            file: "test.rs",
                            base,
                            generation: 1,
                            wrap: false,
                            dark: true,
                            prefs: &prefs,
                            editable: true,
                            changed: &[],
                            removed: &[],
                            old_runs: &[],
                            jump: None,
                            scroll: None,
                            spacers: false,
                        },
                    );
                },
            );
            output.textures_delta.clear();
        };
        send(&mut draft, vec![]);
        let mut state = egui::TextEdit::load_state(&ctx, id).unwrap();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(5),
            )));
        state.store(&ctx, id);
        ctx.memory_mut(|memory| memory.request_focus(id));
        let key = |key, modifiers| Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        send(
            &mut draft,
            vec![
                key(Key::L, Modifiers::COMMAND | Modifiers::SHIFT),
                key(Key::A, Modifiers::NONE),
                Event::Text("π".into()),
            ],
        );
        let expected = base.replace("test", "π");
        assert!(
            draft.draft.as_deref() == Some(expected.as_str()),
            "all occurrences edited"
        );
        send(&mut draft, vec![key(Key::Z, Modifiers::COMMAND)]);
        assert!(
            draft.draft.as_deref() == Some(base),
            "original bytes restored"
        );
        send(
            &mut draft,
            vec![key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT)],
        );
        assert!(
            draft.draft.as_deref() == Some(expected.as_str()),
            "all edits reapplied"
        );
    }

    #[test]
    fn headless_large_read_only_editor_folds_from_the_gutter() {
        check_large_read_only_fold(false);
        check_large_read_only_fold(true);
    }

    fn check_large_read_only_fold(wrap: bool) {
        use super::{pos2, vec2};
        use egui::{Event, Modifiers, PointerButton, RawInput};
        let ctx = egui::Context::default();
        let filler = "// padding abcdefghijklmnopqrstuvwxyz\n".repeat(20_050);
        // Hit a section past both the byte cutoff and the separate syntax-color line limit.
        let row = filler.lines().count();
        let file = "fixture.rs";
        let text = format!(
            "{filler}fn late() {{\n    // {}\n    again();\n}}\n",
            "wrapped ".repeat(30)
        );
        assert!(text.len() > super::VIRTUAL_PREVIEW_THRESHOLD);
        let mut draft = DiffUi::default();
        let prefs = EditorPrefs {
            font: egui::FontId::monospace(12.0),
            tab: 4,
            crlf: false,
            syntax: false,
            numbers: true,
        };
        let run = |draft: &mut DiffUi, jump, events| {
            let mut gutter_id = egui::Id::NULL;
            let mut output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        pos2(0.0, 0.0),
                        vec2(930.0, 669.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    gutter_id = ui
                        .scope_id()
                        .with(("file_hl", file))
                        .with(("fold_line", row));
                    file_editor(
                        ui,
                        draft,
                        FileEditor {
                            file,
                            base: &text,
                            generation: 1,
                            wrap,
                            dark: true,
                            prefs: &prefs,
                            editable: false,
                            changed: &[],
                            removed: &[],
                            old_runs: &[],
                            jump,
                            scroll: None,
                            spacers: false,
                        },
                    );
                },
            );
            output.textures_delta.clear();
            ctx.read_response(gutter_id)
        };
        let click_gutter = |draft: &mut DiffUi, jump| {
            let button = run(draft, jump, vec![]).expect("visible fold control");
            let pos = button.rect.center();
            let click = |pressed| Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            };
            run(draft, None, vec![Event::PointerMoved(pos), click(true)]);
            run(draft, None, vec![click(false)]);
        };
        click_gutter(&mut draft, Some(row));
        assert!(draft.folds.collapsed(row));
        let visible = draft.folds.visible_lines(row + 4);
        assert_eq!(visible.len(), row + 2);
        assert_eq!(visible.get(row), Some(row));
        assert_eq!(visible.get(row + 1), Some(row + 3));
        assert_eq!(visible.find(row + 1), None);
        assert_eq!(visible.find(row + 3), Some(row + 1));

        click_gutter(&mut draft, None);
        assert!(!draft.folds.collapsed(row));
    }

    #[test]
    fn headless_large_split_scrolls_both_ways_with_wrapping_and_ruler_navigation() {
        for wrap in [false, true] {
            check_large_split_scroll(wrap);
        }
    }

    fn check_large_split_scroll(wrap: bool) {
        use super::{Sync, edit_comparison, pos2, vec2};
        use egui::{Event, Key, Modifiers, PointerButton, RawInput};
        let ctx = egui::Context::default();
        let common =
            "// abcdefghijklmnopqrstuvwxyz abcdefghijklmnopqrstuvwxyz abcdefghijklmnopqrstuvwxyz\n"
                .repeat(7_000);
        let heading = "// common heading\n".repeat(8);
        let old = format!("{heading}{}{common}", "// removed line\n".repeat(48));
        let base = format!("{heading}{}{common}", "// added line\n".repeat(5));
        assert!(base.len() > super::VIRTUAL_PREVIEW_THRESHOLD);
        let comparison = crate::app::textdiff::compare(&old, &base, false);
        let mut draft = DiffUi {
            editor: 82,
            ..Default::default()
        };
        let id = egui::Id::unique("large_split");
        let prefs = EditorPrefs {
            font: egui::FontId::monospace(12.0),
            tab: 4,
            crlf: false,
            syntax: false,
            numbers: true,
        };
        ctx.data_mut(|data| data.insert_temp(id.with("split_size"), 0.29f32));
        let ready = std::cell::Cell::new(true);
        let mut run = |events| {
            let mut output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        pos2(0.0, 0.0),
                        vec2(930.0, 669.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    edit_comparison(
                        ui,
                        id,
                        "split",
                        &old,
                        ready.get().then_some(&comparison),
                        Some(&comparison),
                        &(true, Vec::new(), Vec::new()),
                        None,
                        FileEditor {
                            file: "fixture.txt",
                            base: &base,
                            generation: 1,
                            wrap,
                            dark: true,
                            prefs: &prefs,
                            editable: true,
                            changed: &[],
                            removed: &[],
                            old_runs: &[],
                            jump: None,
                            scroll: None,
                            spacers: false,
                        },
                        &mut draft,
                        (false, false),
                    );
                },
            );
            output.textures_delta.clear();
            ctx.data_mut(|data| data.get_temp::<Sync>(id.with(("edit_split_scroll", 82u64))))
                .expect("split scroll state")
                .positions()
        };
        let settle = |run: &mut dyn FnMut(Vec<Event>) -> [f32; 2]| {
            let mut positions = [0.0; 2];
            for _ in 0..24 {
                positions = run(vec![]);
            }
            positions
        };
        assert_eq!(run(vec![]), [0.0; 2]);
        let wheel = |x, delta| {
            vec![
                Event::PointerMoved(pos2(x, 180.0)),
                Event::MouseWheel {
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Unknown,
                    unit: egui::MouseWheelUnit::Point,
                    delta: vec2(0.0, delta),
                    modifiers: Modifiers::NONE,
                },
            ]
        };
        run(wheel(115.0, -2300.0));
        let after_old = settle(&mut run);
        assert!(
            after_old[0] > 900.0 && after_old[1] > 400.0,
            "old to new: {after_old:?}"
        );
        if !wrap {
            let expected = 43.0 * super::editor_line_height(&prefs.font);
            assert!(
                (after_old[0] - after_old[1] - expected).abs() < 5.0,
                "{after_old:?}"
            );
        }
        run(wheel(570.0, -1100.0));
        let after_new = settle(&mut run);
        assert!(
            after_new[0] > after_old[0] + 300.0 && after_new[1] > after_old[1] + 300.0,
            "new to old: {after_old:?} -> {after_new:?}"
        );
        assert_eq!(
            run(vec![]),
            after_new,
            "a settled scroll does not bounce back"
        );

        ready.set(false);
        assert_eq!(
            run(vec![]),
            after_new,
            "a pending diff preserves the established alignment"
        );
        ready.set(true);
        assert_eq!(run(vec![]), after_new);

        let ruler = ctx
            .read_response(id.with("overview_ruler"))
            .expect("ruler response");
        let pos = ruler.rect.center();
        let click = |pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        run(vec![Event::PointerMoved(pos), click(true)]);
        run(vec![click(false)]);
        let jumped = run(vec![]);
        assert!(
            jumped[0] > 30_000.0 && jumped[1] > 30_000.0,
            "ruler jump: {jumped:?}"
        );
        assert_eq!(run(vec![]), jumped);
        run(vec![Event::Key {
            key: Key::Home,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }]);
        assert_eq!(run(vec![]), [0.0; 2]);
        assert!(
            draft.draft.is_none(),
            "navigation never edits the working text"
        );
    }

    #[test]
    fn headless_windows_cursor_undo_restores_selections_without_editing_the_buffer() {
        use super::Direction;
        use egui::text::{CCursor, CCursorRange};
        use egui::{Event, Key, Modifiers, RawInput};
        let ctx = egui::Context::default();
        let mut draft = DiffUi {
            editor: 83,
            ..Default::default()
        };
        let id = editor_id(draft.editor);
        let base = "let test = test;\r\n";
        let prefs = EditorPrefs {
            font: egui::FontId::monospace(12.0),
            tab: 4,
            crlf: true,
            syntax: false,
            numbers: true,
        };
        let send = |draft: &mut DiffUi, events| {
            let mut output = ctx.run_ui(
                RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    file_editor(
                        ui,
                        draft,
                        FileEditor {
                            file: "fixture.rs",
                            base,
                            generation: 1,
                            wrap: false,
                            dark: true,
                            prefs: &prefs,
                            editable: true,
                            changed: &[],
                            removed: &[],
                            old_runs: &[],
                            jump: None,
                            scroll: None,
                            spacers: false,
                        },
                    );
                },
            );
            output.textures_delta.clear();
        };
        let key = |key, modifiers| Event::Key {
            key,
            modifiers,
            physical_key: None,
            pressed: true,
            repeat: false,
        };
        let cursor = || {
            egui::TextEdit::load_state(&ctx, id)
                .and_then(|state| state.cursor.char_range())
                .unwrap()
        };
        let cmd = Modifiers {
            ctrl: true,
            command: true,
            ..Modifiers::NONE
        };

        send(&mut draft, vec![]);
        let mut edit = egui::TextEdit::load_state(&ctx, id).unwrap();
        edit.cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(5))));
        edit.store(&ctx, id);
        ctx.memory_mut(|memory| memory.request_focus(id));
        send(&mut draft, vec![]);
        let start = cursor();
        send(&mut draft, vec![key(Key::L, cmd | Modifiers::SHIFT)]);
        let multi = cursor();
        assert!(draft.cursors.multiple());
        send(&mut draft, vec![key(Key::U, cmd)]);
        assert_eq!(cursor(), start);
        assert!(!draft.cursors.multiple());
        assert!(
            draft.draft.is_none(),
            "Windows Ctrl+U must not delete the text before the caret"
        );
        draft.editor_cache.cursor_history.request = Some(Direction::Redo);
        send(&mut draft, vec![]);
        assert_eq!(cursor(), multi);
        assert!(
            draft.cursors.multiple(),
            "cursor redo restores secondary selections too"
        );

        send(&mut draft, vec![Event::Text("replacement".into())]);
        assert_eq!(
            draft.draft.as_deref(),
            Some("let replacement = replacement;\r\n")
        );
        send(&mut draft, vec![key(Key::U, cmd)]);
        assert_eq!(
            draft.draft.as_deref(),
            Some("let replacement = replacement;\r\n")
        );
        send(&mut draft, vec![key(Key::Z, cmd)]);
        assert_eq!(
            draft.draft.as_deref(),
            Some(base),
            "document undo remains available"
        );
        if cfg!(windows) {
            let mut edit = egui::TextEdit::load_state(&ctx, id).unwrap();
            edit.cursor
                .set_char_range(Some(CCursorRange::one(CCursor::new(5))));
            edit.store(&ctx, id);
            send(
                &mut draft,
                vec![
                    key(Key::U, cmd | Modifiers::ALT),
                    Event::Text("\u{fa}".into()),
                ],
            );
            assert_eq!(draft.draft.as_deref(), Some("let t\u{fa}est = test;\r\n"));
        }
    }

    #[test]
    fn headless_unicode_decorations_work_in_edit_mode_and_virtual_previews() {
        for (editable, large) in [(true, false), (true, true), (false, true)] {
            check_unicode_view(editable, large);
        }
    }

    fn check_unicode_view(editable: bool, large: bool) {
        use super::{pos2, unicode_highlight, vec2};
        let ctx = egui::Context::default();
        let filler = if large {
            "A plain line in the large preview.\n".repeat(20_050)
        } else {
            String::new()
        };
        let line = filler.lines().count();
        let text = format!(
            "{filler}café résumé Cafe\u{301} español 👩🏽‍💻 1️⃣ plain😀text\npayp\u{430}l a\u{200b}b x\u{202e}y\n"
        );
        let mut draft = DiffUi {
            editor: 84,
            ..Default::default()
        };
        let prefs = EditorPrefs {
            font: egui::FontId::monospace(12.0),
            tab: 4,
            crlf: false,
            syntax: false,
            numbers: true,
        };
        let run = |draft: &mut DiffUi| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        pos2(0.0, 0.0),
                        vec2(930.0, 669.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    file_editor(
                        ui,
                        draft,
                        FileEditor {
                            file: "example.txt",
                            base: &text,
                            generation: 1,
                            wrap: false,
                            dark: true,
                            prefs: &prefs,
                            editable,
                            changed: &[],
                            removed: &[],
                            old_runs: &[],
                            jump: Some(line),
                            scroll: None,
                            spacers: false,
                        },
                    );
                },
            );
            output.textures_delta.clear();
            let color = unicode_highlight::ink(true);
            output.shapes.iter().filter(|shape| {
                matches!(&shape.shape, egui::Shape::Rect(rect) if rect.stroke.color == color && rect.rect.width() >= 3.0)
            }).count()
        };
        let start = std::time::Instant::now();
        let count = loop {
            let count = run(&mut draft);
            if count > 0 || start.elapsed() > std::time::Duration::from_secs(4) {
                break count;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(
            count, 3,
            "one marker for each suspicious code point; editable={editable}, large={large}"
        );
        assert!(
            draft.draft.is_none(),
            "painting markers cannot create a draft"
        );
    }

    #[test]
    fn headless_unicode_decorations_follow_edits_and_undo() {
        use super::unicode_highlight;
        use egui::text::{CCursor, CCursorRange};
        use egui::{Event, Key, Modifiers};
        let ctx = egui::Context::default();
        let mut draft = DiffUi {
            editor: 85,
            ..Default::default()
        };
        let id = editor_id(draft.editor);
        let base = "café a\u{200b}b 👩🏽‍💻\r\n";
        let prefs = EditorPrefs {
            font: egui::FontId::monospace(12.0),
            tab: 4,
            crlf: true,
            syntax: false,
            numbers: true,
        };
        let run = |draft: &mut DiffUi, events| {
            let mut result = ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    file_editor(
                        ui,
                        draft,
                        FileEditor {
                            file: "example.txt",
                            base,
                            generation: 1,
                            wrap: true,
                            dark: true,
                            prefs: &prefs,
                            editable: true,
                            changed: &[],
                            removed: &[],
                            old_runs: &[],
                            jump: None,
                            scroll: None,
                            spacers: false,
                        },
                    );
                },
            );
            result.textures_delta.clear();
            result.shapes.iter().filter(|shape| {
                matches!(&shape.shape, egui::Shape::Rect(rect) if rect.stroke.color == unicode_highlight::ink(true))
            }).count()
        };
        assert_eq!(run(&mut draft, vec![]), 1);
        let mut state = egui::TextEdit::load_state(&ctx, id).unwrap();
        state
            .cursor
            .set_char_range(Some(CCursorRange::two(CCursor::new(6), CCursor::new(7))));
        state.store(&ctx, id);
        ctx.memory_mut(|memory| memory.request_focus(id));
        let key = |key, modifiers| Event::Key {
            key,
            modifiers,
            physical_key: None,
            pressed: true,
            repeat: false,
        };
        assert_eq!(run(&mut draft, vec![key(Key::Delete, Modifiers::NONE)]), 0);
        assert_eq!(draft.draft.as_deref(), Some("café ab 👩🏽‍💻\r\n"));
        assert_eq!(run(&mut draft, vec![]), 0);
        run(&mut draft, vec![key(Key::Z, Modifiers::COMMAND)]);
        assert_eq!(draft.draft.as_deref(), Some(base));
        assert_eq!(run(&mut draft, vec![]), 1);
    }

    #[test]
    fn headless_virtual_preview_can_copy_an_entire_large_file() {
        use super::{pos2, vec2};
        use egui::{Event, Key, Modifiers, OutputCommand, PointerButton, RawInput};
        let ctx = egui::Context::default();
        let file = "large.txt";
        let text = "café 👩🏽‍💻 in a large read-only file.\r\n".repeat(14_000);
        assert!(text.len() > super::VIRTUAL_PREVIEW_THRESHOLD);
        let prefs = EditorPrefs {
            font: egui::FontId::monospace(12.0),
            tab: 4,
            crlf: true,
            syntax: false,
            numbers: true,
        };
        let mut state = DiffUi::default();
        let mut selection = egui::Id::NULL;
        let mut run = |events| {
            let mut outcome = ctx.run_ui(
                RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        pos2(0.0, 0.0),
                        vec2(800.0, 480.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    selection = ui.scope_id().with(("file_hl", file)).with("selection");
                    file_editor(
                        ui,
                        &mut state,
                        FileEditor {
                            file,
                            base: &text,
                            generation: 1,
                            wrap: false,
                            dark: true,
                            prefs: &prefs,
                            editable: false,
                            changed: &[],
                            removed: &[],
                            old_runs: &[],
                            jump: None,
                            scroll: None,
                            spacers: false,
                        },
                    );
                },
            );
            outcome.textures_delta.clear();
            (selection, outcome.platform_output.commands)
        };
        let (id, _) = run(vec![]);
        let preview = ctx.read_response(id).unwrap();
        let pos = preview.rect.center();
        let click = |pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        run(vec![Event::PointerMoved(pos), click(true)]);
        run(vec![click(false)]);
        let select_all = Event::Key {
            key: Key::A,
            modifiers: Modifiers::COMMAND,
            physical_key: None,
            pressed: true,
            repeat: false,
        };
        let (_, commands) = run(vec![select_all, Event::Copy]);
        assert!(
            commands.into_iter().any(
                |command| matches!(command, OutputCommand::CopyText(copied) if copied == text)
            )
        );
        assert!(state.draft.is_none());
    }

    #[test]
    fn headless_inline_edit_changes_only_the_modified_source() {
        use egui::{Event, RawInput};
        let ctx = egui::Context::default();
        let base = "new\r\n";
        let mut draft = DiffUi {
            editor: 44,
            ..Default::default()
        };
        let id = editor_id(draft.editor).with("folded");
        let prefs = EditorPrefs {
            font: egui::FontId::monospace(12.0),
            tab: 4,
            crlf: true,
            syntax: true,
            numbers: true,
        };
        let removed = [super::folding::RemovedLine {
            before: Some(0),
            number: 0,
            text: "old",
        }];
        let send = |draft: &mut DiffUi, events| {
            let mut output = ctx.run_ui(
                RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    file_editor(
                        ui,
                        draft,
                        FileEditor {
                            file: "test.rs",
                            base,
                            generation: 1,
                            wrap: false,
                            dark: true,
                            prefs: &prefs,
                            editable: true,
                            changed: &[true],
                            removed: &removed,
                            old_runs: &[],
                            jump: None,
                            scroll: None,
                            spacers: false,
                        },
                    );
                },
            );
            output.textures_delta.clear();
        };
        send(&mut draft, vec![]);
        let mut state = egui::TextEdit::load_state(&ctx, id).expect("inline editor state");
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new("old\nnew\r".chars().count()),
            )));
        state.store(&ctx, id);
        ctx.memory_mut(|memory| memory.request_focus(id));
        send(&mut draft, vec![Event::Text("π".into())]);
        assert_eq!(draft.draft.as_deref(), Some("newπ\r\n"));
    }
}
