//! File History and Blame for one file: the commits that changed it on the left, and the
//! selected commit's change or who last changed each line on the right.

use super::App;
use super::ctxmenu::Later;
use super::icons::{self, Icon};
use super::repo::{DiffTarget, HistoryMode, HistoryTarget, Loaded, RepoTab, Selection};
use super::theme::{GOOD, p};
use super::widgets::{self, MODIFIED, REMOVED, lane_color};
use egui::{Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};
use gitcraft_core::files::{BlameLine, FileHistoryCursor, HistoryEntry};
use gitcraft_core::git::DiffSource;
use std::collections::HashMap;

pub struct HistoryUi {
    pub file: String,
    pub entries: Vec<HistoryEntry>,
    pub cursor: Option<FileHistoryCursor>,
    pub loading: bool,
    pub error: Option<String>,
    pub ignore_revs: bool,
    pub blame: HashMap<(String, Option<String>, bool), Loaded<Vec<BlameLine>>>,
    pub hover: Option<String>,
}

impl Default for HistoryUi {
    fn default() -> Self {
        HistoryUi { file: String::new(), entries: Vec::new(), cursor: None, loading: false, error: None, ignore_revs: true, blame: HashMap::new(), hover: None }
    }
}

/// "3d", "2w": the first unit that fits, coloured red, orange or green by age.
pub fn time_since(unix: i64, now: i64) -> (String, Color32) {
    use gitcraft_core::metrics as m;
    let red = REMOVED;
    let orange = MODIFIED;
    let secs = now - unix;
    let units: [(u32, &str, Color32); 6] = [(m::TIME_YEAR_SECS, "y", red), (m::TIME_MONTH_SECS, "M", red), (m::TIME_WEEK_SECS, "w", red), (m::TIME_DAY_SECS, "d", orange), (m::TIME_HOUR_SECS, "h", GOOD), (m::TIME_MINUTE_SECS, "m", GOOD)];
    for (len, suffix, color) in units {
        if secs >= len as i64 {
            return (format!("{}{suffix}", secs / len as i64), color);
        }
    }
    if secs > 0 { (format!("{secs}s"), GOOD) } else { ("--".into(), p().fg3) }
}

impl App {
    fn want_history_page(&mut self, tab: u64) {
        let Some(r) = self.repo_mut(tab) else { return };
        if r.history_ui.loading {
            return;
        }
        r.history_ui.loading = true;
        let (path, file, cursor) = (r.path.clone(), r.history_ui.file.clone(), r.history_ui.cursor.clone());
        self.spawn(move || {
            let result = gitcraft_core::files::file_history_page(std::path::Path::new(&path), &file, cursor, gitcraft_core::metrics::FILE_HISTORY_MAX);
            super::Msg::FileHistory { tab, file, result }
        });
    }

    pub(crate) fn history_loaded(&mut self, tab: u64, file: String, result: Result<gitcraft_core::files::FileHistoryPage, String>) {
        let Some(r) = self.repo_mut(tab) else { return };
        if r.history_ui.file != file {
            return;
        }
        r.history_ui.loading = false;
        match result {
            Ok(page) => {
                r.history_ui.entries.extend(page.entries);
                r.history_ui.cursor = page.cursor;
                r.history_ui.error = None;
            }
            Err(e) => r.history_ui.error = Some(e),
        }
    }

    fn want_blame(&mut self, tab: u64, file: String, rev: Option<String>, ignore: bool) {
        let Some(r) = self.repo_mut(tab) else { return };
        let key = (file.clone(), rev.clone(), ignore);
        if r.history_ui.blame.contains_key(&key) {
            return;
        }
        r.history_ui.blame.insert(key.clone(), Loaded::Loading);
        let path = r.path.clone();
        self.spawn(move || super::Msg::Blame { tab, key, result: gitcraft_core::files::blame(std::path::Path::new(&path), &file, rev.as_deref(), ignore) });
    }

    pub(crate) fn history_view(&self, ui: &mut Ui, tab: u64, r: &mut RepoTab, target: &HistoryTarget, later: &mut Vec<Later>) {
        let c = p();
        if r.history_ui.file != target.file {
            let ignore = r.history_ui.ignore_revs;
            r.history_ui = HistoryUi { file: target.file.clone(), ignore_revs: ignore, ..Default::default() };
            later.push(Box::new(move |app| app.want_history_page(tab)));
        }
        let h = &mut r.history_ui;
        let selected = target.sha.as_ref().and_then(|s| h.entries.iter().find(|e| e.commit.sha == *s));
        let current = selected.or(h.entries.first()).map(|e| (e.commit.sha.clone(), e.path.clone(), e.commit.parents.first().cloned()));
        let blame_file = if target.sha.is_some() { selected.map(|e| e.path.clone()) } else { Some(target.file.clone()) };
        let w = ui.available_width();
        let (bar, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
        ui.painter().rect_filled(bar, CornerRadius::ZERO, c.toolbar.gamma_multiply(0.6));
        ui.painter().line_segment([bar.left_bottom(), bar.right_bottom()], Stroke::new(1.0, c.black(0.3)));
        icons::paint(ui, Rect::from_min_size(pos2(bar.left() + 12.0, bar.center().y - 7.0), Vec2::splat(14.0)), Icon::History, 14.0, c.link);
        let title = ui.painter().text(pos2(bar.left() + 34.0, bar.center().y), Align2::LEFT_CENTER, "File History", super::fonts::semibold(13.0), c.white(1.0));
        let mut set: Option<HistoryTarget> = None;
        let mut close = false;
        let mut right = ui.new_child(egui::UiBuilder::new().max_rect(bar.shrink2(vec2(8.0, 0.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
        right.spacing_mut().item_spacing.x = 8.0;
        if widgets::icon_button(&mut right, Icon::X, 16.0, "Close (Esc)").clicked() {
            close = true;
        }
        if target.mode == HistoryMode::Blame {
            widgets::checkbox(&mut right, &mut h.ignore_revs, "Ignore revisions", 11.5).on_hover_text("Apply Git's blame ignore lists, including .git-blame-ignore-revs when present");
        }
        // History / Blame switch.
        let (seg, _) = right.allocate_exact_size(vec2(160.0, 20.0), Sense::hover());
        for (i, (mode, icon, label)) in [(HistoryMode::History, Icon::History, "History"), (HistoryMode::Blame, Icon::ScanLine, "Blame")].into_iter().enumerate() {
            let pr = Rect::from_min_size(pos2(seg.left() + i as f32 * 80.0, seg.top()), vec2(80.0, 20.0));
            let on = target.mode == mode;
            let resp = right.interact(pr, right.scope_id().with(("hmode", i)), Sense::click());
            right.painter().rect_filled(pr, CornerRadius::ZERO, if on { c.white(0.15) } else { c.white(0.03) });
            let ink = if on || resp.hovered() { c.white(1.0) } else { c.fg2 };
            icons::paint(&right, Rect::from_min_size(pos2(pr.left() + 12.0, pr.center().y - 6.0), Vec2::splat(12.0)), icon, 12.0, ink);
            right.painter().text(pos2(pr.left() + 30.0, pr.center().y), Align2::LEFT_CENTER, label, FontId::proportional(12.0), ink);
            if resp.clicked() {
                set = Some(HistoryTarget { mode, ..target.clone() });
            }
        }
        let room = right.available_width() - (title.right() - bar.left()) - 8.0;
        let g = widgets::fit(ui, &target.file, FontId::proportional(12.5), c.fg2, room.max(20.0));
        ui.painter().galley(pos2(title.right() + 8.0, bar.center().y - g.size().y / 2.0), g, c.fg2);
        if !ui.ctx().egui_wants_keyboard_input() && !super::dialog_open(ui.ctx()) && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        // List and detail.
        let body = ui.available_rect_before_wrap();
        let list = Rect::from_min_size(body.min, vec2(250.0, body.height()));
        let detail = Rect::from_min_max(pos2(list.right() + 1.0, body.top()), body.max);
        ui.painter().rect_filled(list, CornerRadius::ZERO, c.panel);
        ui.painter().line_segment([list.right_top(), list.right_bottom()], Stroke::new(1.0, c.black(0.3)));
        let mut on_graph: Option<String> = None;
        let now = chrono::Local::now().timestamp();
        let mut lui = ui.new_child(egui::UiBuilder::new().max_rect(list).layout(egui::Layout::top_down(egui::Align::Min)));
        egui::ScrollArea::vertical().id_salt(("hist_list", tab)).auto_shrink([false, false]).show(&mut lui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            let note = |ui: &mut Ui, t: &str, color: Color32| {
                egui::Frame::new().inner_margin(egui::Margin::same(12)).show(ui, |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(t).size(12.0).color(color)).wrap());
                });
            };
            if let Some(e) = &h.error
                && h.entries.is_empty()
            {
                note(ui, &format!("Could not load file history: {e}"), REMOVED);
            }
            if h.loading && h.entries.is_empty() {
                note(ui, "Loading history…", c.fg3);
            }
            if !h.loading && h.error.is_none() && h.entries.is_empty() {
                note(ui, "No commits touch this file.", c.fg3);
            }
            if target.mode == HistoryMode::Blame {
                let (row, resp) = ui.allocate_exact_size(vec2(250.0, 34.0), Sense::click());
                let on = target.sha.is_none();
                ui.painter().rect_filled(row, CornerRadius::ZERO, if on { c.selrow } else if resp.hovered() { c.hoverrow } else { Color32::TRANSPARENT });
                ui.painter().text(pos2(row.left() + 12.0, row.center().y), Align2::LEFT_CENTER, "Working folder (current)", FontId::proportional(12.5), if on { c.white(1.0) } else { c.fg });
                ui.painter().line_segment([row.left_bottom(), row.right_bottom()], Stroke::new(1.0, c.white(0.05)));
                if resp.clicked() {
                    set = Some(HistoryTarget { sha: None, ..target.clone() });
                }
            }
            let current_sha = current.as_ref().map(|c| c.0.clone());
            for e in &h.entries {
                let renamed = e.path != target.file;
                let height = if renamed { 62.0 } else { 48.0 };
                let (row, resp) = ui.allocate_exact_size(vec2(250.0, height), Sense::click());
                let on = Some(&e.commit.sha) == current_sha.as_ref() && (target.mode == HistoryMode::History || target.sha.is_some());
                ui.painter().rect_filled(row, CornerRadius::ZERO, if on { c.selrow } else if resp.hovered() { c.hoverrow } else { Color32::TRANSPARENT });
                ui.painter().line_segment([row.left_bottom(), row.right_bottom()], Stroke::new(1.0, c.white(0.05)));
                widgets::paint_avatar(ui, pos2(row.left() + 12.0 + 12.0, row.top() + 8.0 + 12.0), 24.0, &e.commit.author, &e.commit.email);
                let x = row.left() + 44.0;
                let g = widgets::fit(ui, &e.commit.summary, FontId::proportional(12.5), c.white(1.0), row.right() - x - 8.0);
                ui.painter().galley(pos2(x, row.top() + 8.0), g, c.white(1.0));
                let (since, color) = time_since(e.commit.time, now);
                let mut job = egui::text::LayoutJob::default();
                let small = FontId::proportional(11.0);
                job.append(&format!("{} · ", e.commit.author), 0.0, egui::TextFormat::simple(small.clone(), c.fg3));
                job.append(&since, 0.0, egui::TextFormat::simple(small.clone(), color));
                job.append(" · ", 0.0, egui::TextFormat::simple(small.clone(), c.fg3));
                job.append(&e.commit.sha.chars().take(6).collect::<String>(), 0.0, egui::TextFormat::simple(FontId::monospace(11.0), c.fg3));
                job.wrap = egui::text::TextWrapping::truncate_at_width(row.right() - x - 8.0);
                let g = ui.painter().layout_job(job);
                ui.painter().galley(pos2(x, row.top() + 26.0), g, c.fg3);
                if renamed {
                    let g = widgets::fit(ui, &format!("was {}", e.path), FontId::proportional(10.5), c.fg3, row.right() - x - 8.0);
                    ui.painter().galley(pos2(x, row.top() + 42.0), g, c.fg3);
                }
                let resp = resp.on_hover_text("Double-click to view on the graph");
                if resp.double_clicked() {
                    on_graph = Some(e.commit.sha.clone());
                } else if resp.clicked() {
                    set = Some(HistoryTarget { sha: Some(e.commit.sha.clone()), ..target.clone() });
                }
            }
            if h.cursor.is_some() {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    let label = if h.loading { "Loading older history…" } else if h.error.is_some() { "Retry older history" } else { "Load older history" };
                    if widgets::link(ui, label, 12.0).clicked() && !h.loading {
                        later.push(Box::new(move |app| app.want_history_page(tab)));
                    }
                });
            }
        });
        let mut dui = ui.new_child(egui::UiBuilder::new().max_rect(detail).layout(egui::Layout::top_down(egui::Align::Min)));
        dui.set_clip_rect(detail);
        match target.mode {
            HistoryMode::History => {
                if let Some((sha, file, parent)) = current {
                    let (bar, _) = dui.allocate_exact_size(vec2(detail.width(), 26.0), Sense::hover());
                    dui.painter().line_segment([bar.left_bottom(), bar.right_bottom()], Stroke::new(1.0, c.white(0.05)));
                    let link_r = dui.painter().text(pos2(bar.right() - 12.0, bar.center().y), Align2::RIGHT_CENTER, "View on graph", FontId::proportional(11.5), c.link);
                    if dui.interact(link_r, dui.scope_id().with("on_graph"), Sense::click()).clicked() {
                        on_graph = Some(sha.clone());
                    }
                    let g = widgets::fit(&dui, &file, FontId::proportional(11.5), c.fg2, link_r.left() - bar.left() - 24.0);
                    dui.painter().galley(pos2(bar.left() + 12.0, bar.center().y - g.size().y / 2.0), g, c.fg2);
                    let t = DiffTarget { source: DiffSource::Commit(sha.clone()), file: file.clone(), untracked: false, parent: parent.clone() };
                    self.history_diff(&mut dui, tab, r, &t, later);
                }
            }
            HistoryMode::Blame => match blame_file {
                Some(file) => {
                    let ignore = h.ignore_revs;
                    let key = (file.clone(), target.sha.clone(), ignore);
                    match h.blame.get(&key) {
                        None => {
                            let (f, s) = (file.clone(), target.sha.clone());
                            later.push(Box::new(move |app| app.want_blame(tab, f, s, ignore)));
                        }
                        Some(Loaded::Loading) => {
                            dui.label(egui::RichText::new("Loading blame…").size(13.0).color(c.fg3));
                        }
                        Some(Loaded::Failed(e)) => {
                            dui.label(egui::RichText::new(e).size(13.0).color(REMOVED));
                        }
                        Some(Loaded::Ready(lines)) => {
                            let lines = lines.clone();
                            if let Some(sha) = blame_view(&mut dui, &lines, &mut h.hover, self.settings.flag("wordWrap", false), later) {
                                on_graph = Some(sha);
                            }
                        }
                    }
                }
                None => {
                    let t = if h.loading { "Loading file history..." } else { "Select a revision from this file's loaded history to view its blame." };
                    dui.label(egui::RichText::new(t).size(13.0).color(c.fg3));
                }
            },
        }
        if let Some(sha) = on_graph {
            if r.index.contains_key(&sha) {
                r.ui.history = None;
                r.ui.select(Selection::Commit(sha));
            } else {
                later.push(Box::new(|app| app.toast(false, "This commit is not visible on the graph")));
            }
            return;
        }
        if close {
            r.ui.history = None;
        } else if let Some(t) = set {
            r.ui.history = Some(t);
        }
    }

    /// One commit's change to the file, compared inline or split.
    fn history_diff(&self, ui: &mut Ui, tab: u64, r: &mut RepoTab, target: &DiffTarget, later: &mut Vec<Later>) {
        let c = p();
        let key = super::diff::diff_key(target, false);
        let cache = self.diffs.get(&(tab, key.clone()));
        let Some(cache) = cache else {
            let t = target.clone();
            later.push(Box::new(move |app| app.want_diff(tab, t, false)));
            return;
        };
        let v = match &cache.versions {
            Some(Loaded::Ready(v)) => v,
            Some(Loaded::Failed(e)) => {
                ui.label(egui::RichText::new(e).size(13.0).color(c.fg3));
                return;
            }
            _ => {
                ui.label(egui::RichText::new("Loading…").size(13.0).color(c.fg3));
                return;
            }
        };
        if v.too_large {
            ui.label(egui::RichText::new("This file exceeds the preview limit. Open it in an external editor to inspect it.").size(13.0).color(c.fg3));
            return;
        }
        if v.binary {
            ui.label(egui::RichText::new("Binary file").size(13.0).color(c.fg3));
            return;
        }
        let ignore_ws = self.settings.flag("ignoreWhitespace", false);
        let dark = c.dark;
        match &cache.comparison {
            Some((ws, cmp)) if *ws == ignore_ws => {
                let empty = (dark, Vec::new(), Vec::new());
                let hl = cache.hl.as_ref().filter(|h| h.0 == dark).unwrap_or(&empty);
                let split = self.settings.string("diffMode", "hunk") == "split";
                super::diff::compare_view(ui, ui.scope_id().with(("hist_cmp", &key)), cmp, &target.file, split, hl, false, false, &mut r.diff_ui.jump);
            }
            _ => {
                let (o, n, f) = (v.old.clone().unwrap_or_default(), v.new.clone().unwrap_or_default(), target.file.clone());
                later.push(Box::new(move |app| {
                    let cmp = super::textdiff::compare(&o, &n, ignore_ws);
                    let (ho, hn) = (super::highlight::whole(&f, &o, dark), super::highlight::whole(&f, &n, dark));
                    if let Some(e) = app.diffs.get_mut(&(tab, key)) {
                        e.comparison = Some((ignore_ws, cmp));
                        e.hl = Some((dark, ho, hn));
                    }
                }));
            }
        }
    }
}

/// Blame: each run of lines from one commit has a coloured bar with its author, age and sha.
/// Returns a commit to show on the graph.
fn blame_view(ui: &mut Ui, lines: &[BlameLine], hover: &mut Option<String>, _wrap: bool, later: &mut Vec<Later>) -> Option<String> {
    let c = p();
    let font = FontId::monospace(12.0);
    let mut colors: HashMap<&str, Color32> = HashMap::new();
    for l in lines {
        let n = colors.len();
        colors.entry(l.sha.as_str()).or_insert_with(|| lane_color(n));
    }
    let uncommitted = |l: &BlameLine| !l.sha.is_empty() && l.sha.bytes().all(|b| b == b'0');
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    let text = format!("{} lines · ", lines.len());
    let g = ui.painter().layout_no_wrap(text, FontId::proportional(11.0), c.fg3);
    let gw = g.size().x;
    ui.painter().galley(pos2(head.left() + 12.0, head.center().y - g.size().y / 2.0), g, c.fg3);
    let cr = ui.painter().text(pos2(head.left() + 12.0 + gw, head.center().y), Align2::LEFT_CENTER, "Copy file text", FontId::proportional(11.0), c.link);
    if ui.interact(cr, ui.scope_id().with("copy_blame"), Sense::click()).clicked() {
        let all = lines.iter().map(|l| l.text.clone()).collect::<Vec<_>>().join("\n");
        later.push(Box::new(move |app| app.copy(&all)));
    }
    let mut pick = None;
    let now = chrono::Local::now().timestamp();
    let longest = lines.iter().map(|l| l.text.len()).max().unwrap_or(0) as f32 * 7.3 + 300.0;
    let mut new_hover = None;
    egui::ScrollArea::both().id_salt("blame").auto_shrink([false, false]).content_margin(0).show_viewport(ui, |ui, viewport| {
        let width = longest.max(ui.available_width());
        let (_, _) = ui.allocate_exact_size(vec2(width, lines.len() as f32 * 19.0), Sense::hover());
        let origin = ui.min_rect().min;
        let first = (viewport.top() / 19.0).floor().max(0.0) as usize;
        let last = ((viewport.bottom() / 19.0).ceil() as usize + 1).min(lines.len());
        for i in first..last {
            let l = &lines[i];
            let row = Rect::from_min_size(origin + vec2(0.0, i as f32 * 19.0), vec2(width, 19.0));
            let resp = ui.interact(row, ui.scope_id().with(("blame_row", i)), Sense::hover());
            if resp.hovered() {
                new_hover = Some(l.sha.clone());
            }
            if hover.as_deref() == Some(l.sha.as_str()) {
                ui.painter().rect_filled(row, CornerRadius::ZERO, c.white(0.04));
            }
            let starts = i == 0 || lines[i - 1].sha != l.sha;
            if starts && i > 0 {
                ui.painter().line_segment([row.left_top(), row.right_top()], Stroke::new(1.0, c.white(0.05)));
            }
            let color = if uncommitted(l) { Color32::from_gray(0x88) } else { colors[l.sha.as_str()] };
            ui.painter().rect_filled(Rect::from_min_size(row.min, vec2(3.0, 19.0)), CornerRadius::ZERO, color);
            if starts || i == first {
                let small = FontId::proportional(11.5);
                let who = if uncommitted(l) { "You".to_owned() } else { l.author.clone() };
                let g = widgets::fit(ui, &who, small.clone(), c.white(0.85), 90.0);
                let gw = g.size().x;
                ui.painter().galley(pos2(row.left() + 8.0, row.center().y - g.size().y / 2.0), g, c.white(0.85));
                let (since, sc) = if uncommitted(l) { ("uncommitted".to_owned(), c.fg3) } else { time_since(l.time, now) };
                ui.painter().text(pos2(row.left() + 14.0 + gw, row.center().y), Align2::LEFT_CENTER, since, small, sc);
                if !uncommitted(l) {
                    ui.painter().text(pos2(row.left() + 222.0, row.center().y), Align2::RIGHT_CENTER, l.sha.chars().take(6).collect::<String>(), FontId::monospace(11.5), c.link);
                }
                let meta = Rect::from_min_size(row.min, vec2(230.0, 19.0));
                let mresp = ui.interact(meta, ui.scope_id().with(("blame_meta", i)), Sense::click());
                let tip = if uncommitted(l) { "Not committed yet".to_owned() } else { format!("{}\n{}\nClick to view on the graph", l.summary, widgets::format_date(l.time, "MM/DD/YYYY @ h:mm A")) };
                if mresp.on_hover_text(tip).clicked() && !uncommitted(l) {
                    pick = Some(l.sha.clone());
                }
            }
            ui.painter().text(pos2(row.left() + 230.0 + 44.0 - 8.0, row.center().y), Align2::RIGHT_CENTER, l.line.to_string(), font.clone(), c.fg3);
            ui.painter().line_segment([pos2(row.left() + 274.0, row.top()), pos2(row.left() + 274.0, row.bottom())], Stroke::new(1.0, c.white(0.05)));
            ui.painter().text(pos2(row.left() + 286.0, row.center().y), Align2::LEFT_CENTER, &l.text, font.clone(), c.white(0.85));
        }
    });
    *hover = new_hover;
    pick
}

#[cfg(test)]
mod tests {
    #[test]
    fn ages_pick_the_largest_unit() {
        let now = 1_000_000_000;
        assert_eq!(super::time_since(now - 3 * 86400, now).0, "3d");
        assert_eq!(super::time_since(now - 90, now).0, "1m");
        assert_eq!(super::time_since(now, now).0, "--");
    }
}
