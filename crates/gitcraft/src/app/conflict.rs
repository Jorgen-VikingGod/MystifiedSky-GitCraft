//! The merge conflict tool: A (current) and B (incoming) side by side with a check box per line,
//! the Output below, which can also be edited by hand, and saving the result as resolved.

use super::widgets::Tip;
use super::App;
use super::ctxmenu::Later;
use super::icons::{self, Icon};
use super::repo::{Loaded, RepoTab};
use super::theme::{BAD, p};
use super::widgets::{self, Kind, REMOVED};
use egui::{Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};
use gitcraft_core::actions::Action;
use gitcraft_core::rewrite::ConflictInfo;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Segment {
    Common(Vec<String>),
    Conflict { ours: Vec<String>, base: Vec<String>, theirs: Vec<String>, raw: Vec<String> },
}

#[derive(Clone, Debug, Default)]
pub struct Pick {
    pub a: Vec<bool>,
    pub b: Vec<bool>,
    pub touched: bool,
}

#[derive(Default)]
pub struct ConflictUi {
    pub file: String,
    pub info: Option<Loaded<ConflictInfo>>,
    /// The version being edited, kept while picks or the manual edit are in progress.
    pub base: Option<ConflictInfo>,
    pub picks: HashMap<usize, Pick>,
    pub manual: Option<String>,
    pub current: usize,
    pub jump: bool,
    pub ai_busy: bool,
    /// Bumped by every edit; an AI answer for an older edit is dropped.
    pub revision: u64,
    pub saving: bool,
}

/// Complete Git conflict blocks; incomplete or ordinary marker-like text stays as it is.
pub fn parse_conflicts(text: &str) -> Vec<Segment> {
    let normalized = text.replace("\r\n", "\n");
    let mut lines: Vec<&str> = normalized.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    let mut out = Vec::new();
    let mut common: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let size = lines[i].chars().take_while(|c| *c == '<').count();
        let opening = size >= 7 && (lines[i].len() == size || lines[i][size..].starts_with(' '));
        if !opening {
            common.push(lines[i].to_owned());
            i += 1;
            continue;
        }
        let marker = |line: &str, ch: char| {
            let run: String = std::iter::repeat_n(ch, size).collect();
            line == run || line.starts_with(&format!("{run} "))
        };
        let (mut ours, mut base, mut theirs) = (Vec::new(), Vec::new(), Vec::new());
        let mut part = 0;
        let (mut separator, mut base_seen) = (false, false);
        let mut end = i + 1;
        while end < lines.len() {
            let line = lines[end];
            if marker(line, '<') || marker(line, '>') {
                break;
            }
            if !separator && !base_seen && marker(line, '|') {
                base_seen = true;
                part = 1;
            } else if !separator && line == "=".repeat(size) {
                separator = true;
                part = 2;
            } else {
                match part {
                    0 => ours.push(line.to_owned()),
                    1 => base.push(line.to_owned()),
                    _ => theirs.push(line.to_owned()),
                }
            }
            end += 1;
        }
        if end >= lines.len() || !separator || !marker(lines[end], '>') {
            common.push(lines[i].to_owned());
            i += 1;
            continue;
        }
        if !common.is_empty() {
            out.push(Segment::Common(std::mem::take(&mut common)));
        }
        out.push(Segment::Conflict { ours, base, theirs, raw: lines[i..=end].iter().map(|s| s.to_string()).collect() });
        i = end + 1;
    }
    if !common.is_empty() {
        out.push(Segment::Common(common));
    }
    out
}

/// Untouched chunks keep their markers; picked lines replace touched ones.
pub fn conflict_output(segments: &[Segment], picks: &HashMap<usize, Pick>, working: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for (i, seg) in segments.iter().enumerate() {
        match seg {
            Segment::Common(lines) => out.extend(lines.iter().cloned()),
            Segment::Conflict { ours, theirs, raw, .. } => match picks.get(&i) {
                Some(p) if p.touched => {
                    out.extend(ours.iter().zip(&p.a).filter(|(_, on)| **on).map(|(l, _)| l.clone()));
                    out.extend(theirs.iter().zip(&p.b).filter(|(_, on)| **on).map(|(l, _)| l.clone()));
                }
                _ => out.extend(raw.iter().cloned()),
            },
        }
    }
    let mut text = out.join("\n");
    if !out.is_empty() && working.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// Even an incomplete marker block must not be staged as resolved.
pub fn has_conflict_markers(text: &str) -> bool {
    text.lines().any(|l| {
        let l = l.trim_end_matches('\r');
        for ch in ['<', '|', '>'] {
            let n = l.chars().take_while(|c| *c == ch).count();
            if n >= 7 && (l.len() == n || l[n..].starts_with(' ')) {
                return true;
            }
        }
        let n = l.chars().take_while(|c| *c == '=').count();
        n >= 7 && l.len() == n
    })
}

fn hex(c: &str) -> Color32 {
    let v = u32::from_str_radix(c.trim_start_matches('#'), 16).unwrap_or(0);
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

impl App {
    fn want_conflict(&mut self, tab: u64, file: &str) {
        let Some(r) = self.repo_mut(tab) else { return };
        r.conflict_ui.info = Some(Loaded::Loading);
        let (path, file) = (r.path.clone(), file.to_owned());
        self.spawn(move || Msg::ConflictInfo { tab, info: Box::new(gitcraft_core::rewrite::conflict_info(std::path::Path::new(&path), &file)) });
    }

    pub(crate) fn conflict_loaded(&mut self, tab: u64, info: ConflictInfo) {
        if let Some(r) = self.repo_mut(tab) {
            r.conflict_ui.info = Some(Loaded::Ready(info));
        }
    }

    /// Save the output: resolved when no markers remain, otherwise just written.
    fn save_conflict(&mut self, tab: u64, utf8: bool) {
        let Some(r) = self.repo_mut(tab) else { return };
        let cu = &mut r.conflict_ui;
        let Some(info) = cu.base.clone().or_else(|| match &cu.info {
            Some(Loaded::Ready(i)) => Some(i.clone()),
            _ => None,
        }) else {
            return;
        };
        if cu.saving {
            return;
        }
        if info.decoding_loss && !utf8 {
            self.dialog = Some(super::dialogs::Dialog::confirm_with(
                "Review this encoding before saving",
                "Some original bytes could not be decoded. Choose the correct repository encoding to preserve them. Converting this output to UTF-8 saves the text shown and can replace undecodable bytes.",
                "Convert output to UTF-8",
                false,
                move |app, _| app.save_conflict(tab, true),
            ));
            return;
        }
        let working = info.working.clone().unwrap_or_default();
        let segments = parse_conflicts(&working);
        let output = cu.manual.clone().unwrap_or_else(|| conflict_output(&segments, &cu.picks, &working));
        let crlf = working.contains("\r\n");
        let normalized = output.replace("\r\n", "\n");
        let text = if crlf { normalized.replace('\n', "\r\n") } else { normalized };
        let file = cu.file.clone();
        if !has_conflict_markers(&output) {
            cu.base = None;
            cu.picks.clear();
            cu.manual = None;
            r.ui.conflict = None;
            self.act(tab, Action::SaveResolved { file, content: text, expected: Some(info.working_token.clone()), utf8: Some(utf8) });
        } else {
            cu.saving = true;
            let path = r.path.clone();
            let token = info.working_token.clone();
            self.spawn(move || match gitcraft_core::files::save_checked_mode(std::path::Path::new(&path), &file, &text, Some(&token), utf8) {
                Ok(()) => Msg::ConflictSaved { tab, result: Ok(format!("Saved {file}; conflict markers remain to resolve")) },
                Err(e) => Msg::ConflictSaved { tab, result: Err(e) },
            });
        }
    }

    pub(crate) fn conflict_saved(&mut self, tab: u64, result: Result<String, String>) {
        if let Some(r) = self.repo_mut(tab) {
            r.conflict_ui.saving = false;
        }
        match result {
            Ok(text) => {
                if let Some(r) = self.repo_mut(tab) {
                    r.conflict_ui.base = None;
                    r.conflict_ui.picks.clear();
                    r.conflict_ui.manual = None;
                    r.conflict_ui.info = None;
                }
                self.toast(true, text);
                self.load_repo(tab);
            }
            Err(e) if e.contains("ENCODING_UNREPRESENTABLE") || e.contains("ENCODING_DECODE_LOSS") => {
                self.dialog = Some(super::dialogs::Dialog::confirm_with(
                    "Save this file as UTF-8?",
                    "The current encoding cannot preserve all characters. The original file is unchanged. Convert it to UTF-8 with a byte-order mark?",
                    "Save as UTF-8",
                    false,
                    move |app, _| app.save_conflict(tab, true),
                ));
            }
            Err(e) => self.toast(false, e),
        }
    }

    pub(crate) fn conflict_view(&self, ui: &mut Ui, tab: u64, r: &mut RepoTab, file: &str, later: &mut Vec<Later>) {
        let c = p();
        if r.conflict_ui.file != file {
            r.conflict_ui = ConflictUi { file: file.to_owned(), ..Default::default() };
        }
        if r.conflict_ui.info.is_none() {
            let f = file.to_owned();
            later.push(Box::new(move |app| app.want_conflict(tab, &f)));
            r.conflict_ui.info = Some(Loaded::Loading);
        }
        let cu = &mut r.conflict_ui;
        let info = match (&cu.base, &cu.info) {
            (Some(b), _) => b.clone(),
            (None, Some(Loaded::Ready(i))) => i.clone(),
            (None, Some(Loaded::Failed(e))) => {
                ui.label(egui::RichText::new(e).size(13.0).color(REMOVED));
                return;
            }
            _ => {
                ui.add_space(16.0);
                ui.label(egui::RichText::new("Loading conflict…").size(13.0).color(c.fg3));
                return;
            }
        };
        let working = info.working.clone().unwrap_or_default();
        let segments = parse_conflicts(&working);
        let chunks: Vec<usize> = segments.iter().enumerate().filter(|(_, s)| matches!(s, Segment::Conflict { .. })).map(|(i, _)| i).collect();
        let (ours_label, theirs_label) = (info.ours_label.clone(), info.theirs_label.clone());
        let mut close = false;
        // Header.
        let w = ui.available_width();
        let (bar, _) = ui.allocate_exact_size(vec2(w, 32.0), Sense::hover());
        ui.painter().rect_filled(bar, CornerRadius::ZERO, c.toolbar.gamma_multiply(0.6));
        ui.painter().line_segment([bar.left_bottom(), bar.right_bottom()], Stroke::new(1.0, c.black(0.3)));
        widgets::status_mark(ui, Rect::from_min_size(pos2(bar.left() + 12.0, bar.center().y - 7.0), Vec2::splat(14.0)), 'U');
        let mut hr = ui.new_child(egui::UiBuilder::new().max_rect(bar.shrink2(vec2(8.0, 0.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
        hr.spacing_mut().item_spacing.x = 6.0;
        if widgets::icon_button(&mut hr, Icon::X, 16.0, "Close (Esc)").clicked() {
            close = true;
        }
        if widgets::button(&mut hr, Kind::Default, None, &format!("Take incoming ({theirs_label})"), 22.0, true).clicked() {
            let f = file.to_owned();
            later.push(Box::new(move |app| {
                app.act(tab, Action::TakeSide { paths: vec![f], side: "theirs".into() });
                if let Some(r) = app.repo_mut(tab) {
                    r.ui.conflict = None;
                }
            }));
        }
        if widgets::button(&mut hr, Kind::Default, None, &format!("Take current ({ours_label})"), 22.0, true).clicked() {
            let f = file.to_owned();
            later.push(Box::new(move |app| {
                app.act(tab, Action::TakeSide { paths: vec![f], side: "ours".into() });
                if let Some(r) = app.repo_mut(tab) {
                    r.ui.conflict = None;
                }
            }));
        }
        if !chunks.is_empty() {
            let n = chunks.len();
            if widgets::icon_button(&mut hr, Icon::ArrowDown, 14.0, "Next conflict").clicked() {
                cu.current = (cu.current + 1) % n;
                cu.jump = true;
            }
            if widgets::icon_button(&mut hr, Icon::ArrowUp, 14.0, "Previous conflict").clicked() {
                cu.current = (cu.current + n - 1) % n;
                cu.jump = true;
            }
            hr.label(egui::RichText::new(format!("conflict {} of {n}", (cu.current + 1).min(n))).size(12.0).color(c.fg2));
        }
        let room = hr.available_width() - 40.0;
        let g = widgets::fit(ui, file, FontId::proportional(13.0), c.white(1.0), room);
        ui.painter().galley(pos2(bar.left() + 34.0, bar.center().y - g.size().y / 2.0), g, c.white(1.0));
        if !ui.ctx().egui_wants_keyboard_input() && !super::dialog_open(ui.ctx()) && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        if close {
            r.ui.conflict = None;
            return;
        }
        if info.binary || chunks.is_empty() {
            let text = if info.binary {
                "This is a binary file, so its lines can't be merged here. Choose which version to keep with Take current or Take incoming."
            } else {
                "This file has no conflict markers (for example one side deleted it). Choose which version to keep, or mark it resolved from the file list."
            };
            egui::Frame::new().inner_margin(egui::Margin::same(20)).show(ui, |ui| {
                ui.add(egui::Label::new(egui::RichText::new(text).size(13.0).color(c.fg2)).wrap());
            });
            return;
        }
        let pick_of = |picks: &HashMap<usize, Pick>, i: usize| -> Pick {
            picks.get(&i).cloned().unwrap_or_else(|| match &segments[i] {
                Segment::Conflict { ours, theirs, .. } => Pick { a: vec![false; ours.len()], b: vec![false; theirs.len()], touched: false },
                _ => Pick::default(),
            })
        };
        let body = ui.available_rect_before_wrap();
        let out_h = (body.height() * 0.45).round();
        let panes = Rect::from_min_max(body.min, pos2(body.right(), body.bottom() - out_h));
        let half = panes.width() / 2.0;
        let mut changed: Option<(usize, Pick)> = None;
        let mut take_all: Option<(bool, bool)> = None;
        let font = FontId::monospace(12.0);
        let jump_to = cu.jump.then(|| chunks.get(cu.current).copied()).flatten();
        for (side, label, color) in [(false, ours_label.clone(), hex(&gitcraft_core::metrics::metrics().conflict_a)), (true, theirs_label.clone(), hex(&gitcraft_core::metrics::metrics().conflict_b))] {
            let rect = Rect::from_min_size(pos2(panes.left() + if side { half } else { 0.0 }, panes.top()), vec2(half, panes.height()));
            let mut pui = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)));
            pui.set_clip_rect(rect);
            pui.spacing_mut().item_spacing = Vec2::ZERO;
            if !side {
                ui.painter().line_segment([rect.right_top(), rect.right_bottom()], Stroke::new(1.0, c.black(0.3)));
            }
            let (head, _) = pui.allocate_exact_size(vec2(half, 26.0), Sense::hover());
            pui.painter().rect_filled(Rect::from_min_max(pos2(head.left(), head.bottom() - 2.0), head.max), CornerRadius::ZERO, color);
            let t = pui.painter().text(pos2(head.left() + 8.0, head.center().y), Align2::LEFT_CENTER, if side { "B" } else { "A" }, super::fonts::bold(12.0), color);
            let g = widgets::fit(&pui, &label, FontId::proportional(12.0), c.fg, half - 120.0);
            pui.painter().galley(pos2(t.right() + 8.0, head.center().y - g.size().y / 2.0), g, c.fg);
            let all_id = pui.scope_id().with(("take_all", side));
            let mut all: bool = pui.data(|d| d.get_temp(all_id)).unwrap_or(false);
            let mut child = pui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(pos2(head.right() - 90.0, head.top()), head.max)).layout(egui::Layout::left_to_right(egui::Align::Center)));
            if widgets::checkbox(&mut child, &mut all, "Take all", 12.0).clicked() {
                take_all = Some((side, all));
            }
            pui.data_mut(|d| d.insert_temp(all_id, all));
            egui::ScrollArea::both().id_salt(("conflict_pane", side)).auto_shrink([false, false]).show(&mut pui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                for (i, seg) in segments.iter().enumerate() {
                    match seg {
                        Segment::Common(lines) => {
                            for l in lines {
                                let (row, _) = ui.allocate_exact_size(vec2(half.max(l.len() as f32 * 7.3 + 40.0), 19.0), Sense::hover());
                                ui.painter().text(pos2(row.left() + 28.0, row.center().y), Align2::LEFT_CENTER, l, font.clone(), c.white(0.55));
                            }
                        }
                        Segment::Conflict { ours, theirs, .. } => {
                            let lines = if side { theirs } else { ours };
                            let p = pick_of(&cu.picks, i);
                            ui.add_space(2.0);
                            let start = ui.cursor().top();
                            if lines.is_empty() {
                                let (row, _) = ui.allocate_exact_size(vec2(half, 19.0), Sense::hover());
                                ui.painter().rect_filled(row, CornerRadius::ZERO, color.gamma_multiply(0.25));
                                ui.painter().text(pos2(row.left() + 28.0, row.center().y), Align2::LEFT_CENTER, "(no lines on this side)", FontId::proportional(11.0), c.fg3);
                            }
                            for (k, l) in lines.iter().enumerate() {
                                let (row, resp) = ui.allocate_exact_size(vec2(half.max(l.len() as f32 * 7.3 + 40.0), 19.0), Sense::click());
                                ui.painter().rect_filled(row, CornerRadius::ZERO, color.gamma_multiply(0.25));
                                if resp.hovered() {
                                    ui.painter().rect_filled(row, CornerRadius::ZERO, c.white(0.05));
                                }
                                let on = if side { p.b.get(k).copied().unwrap_or(false) } else { p.a.get(k).copied().unwrap_or(false) };
                                let bx = Rect::from_min_size(pos2(row.left() + 8.0, row.center().y - 6.0), Vec2::splat(12.0));
                                if on {
                                    ui.painter().rect_filled(bx, CornerRadius::same(2), Color32::from_rgb(0x80, 0xb1, 0xea));
                                    icons::paint(ui, bx, Icon::Check, 10.0, Color32::from_rgb(0x1c, 0x1e, 0x23));
                                } else {
                                    ui.painter().rect_stroke(bx, CornerRadius::same(2), Stroke::new(1.0, c.white(0.4)), egui::StrokeKind::Inside);
                                }
                                ui.painter().text(pos2(row.left() + 28.0, row.center().y), Align2::LEFT_CENTER, l, font.clone(), c.white(0.9));
                                if resp.tip("Take this line").clicked() {
                                    let mut np = p.clone();
                                    let list = if side { &mut np.b } else { &mut np.a };
                                    if let Some(x) = list.get_mut(k) {
                                        *x = !*x;
                                    }
                                    changed = Some((i, np));
                                }
                            }
                            let end = ui.cursor().top();
                            ui.painter().rect_filled(Rect::from_min_max(pos2(ui.min_rect().left(), start), pos2(ui.min_rect().left() + 3.0, end)), CornerRadius::ZERO, color);
                            if jump_to == Some(i) {
                                ui.scroll_to_rect(Rect::from_min_max(pos2(ui.min_rect().left(), start), pos2(ui.min_rect().left() + 10.0, end)), Some(egui::Align::Center));
                            }
                            ui.add_space(2.0);
                        }
                    }
                }
            });
        }
        if jump_to.is_some() {
            cu.jump = false;
        }
        let saving = cu.saving;
        if !saving {
            if let Some((i, mut p)) = changed {
                if cu.base.is_none() {
                    cu.base = Some(info.clone());
                }
                p.touched = true;
                cu.picks.insert(i, p);
                cu.revision += 1;
            }
            if let Some((side, on)) = take_all {
                if cu.base.is_none() {
                    cu.base = Some(info.clone());
                }
                for &i in &chunks {
                    let mut p = pick_of(&cu.picks, i);
                    let list = if side { &mut p.b } else { &mut p.a };
                    list.iter_mut().for_each(|x| *x = on);
                    p.touched = true;
                    cu.picks.insert(i, p);
                }
                cu.revision += 1;
            }
        }
        // Output.
        let built = conflict_output(&segments, &cu.picks, &working);
        let output = cu.manual.clone().unwrap_or_else(|| built.clone());
        let unresolved = if cu.manual.is_none() { chunks.iter().filter(|i| !cu.picks.get(i).is_some_and(|p| p.touched)).count() } else { parse_conflicts(&output).iter().filter(|s| matches!(s, Segment::Conflict { .. })).count() };
        let all_done = !has_conflict_markers(&output);
        let out_color = hex(&gitcraft_core::metrics::metrics().conflict_output);
        let orect = Rect::from_min_max(pos2(body.left(), panes.bottom()), body.max);
        let mut oui = ui.new_child(egui::UiBuilder::new().max_rect(orect).layout(egui::Layout::top_down(egui::Align::Min)));
        oui.set_clip_rect(orect);
        oui.spacing_mut().item_spacing = Vec2::ZERO;
        oui.painter().rect_filled(Rect::from_min_size(orect.min, vec2(orect.width(), 2.0)), CornerRadius::ZERO, out_color);
        oui.add_space(2.0);
        let (ohead, _) = oui.allocate_exact_size(vec2(orect.width(), 30.0), Sense::hover());
        let t = oui.painter().text(pos2(ohead.left() + 8.0, ohead.center().y), Align2::LEFT_CENTER, "Output", super::fonts::bold(12.0), out_color);
        let state = if !all_done {
            if unresolved > 0 { format!("{unresolved} conflict{} left", if unresolved == 1 { "" } else { "s" }) } else { "conflict markers remain".into() }
        } else if cu.manual.is_some() {
            "edited by hand".into()
        } else {
            "all conflicts decided".into()
        };
        oui.painter().text(pos2(t.right() + 8.0, ohead.center().y), Align2::LEFT_CENTER, state, FontId::proportional(12.0), c.fg3);
        let mut orow = oui.new_child(egui::UiBuilder::new().max_rect(ohead.shrink2(vec2(8.0, 0.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
        orow.spacing_mut().item_spacing.x = 6.0;
        if widgets::button(&mut orow, Kind::Success, None, if all_done { "Save Output" } else { "Create Conflict Chunks" }, 22.0, !saving).clicked() {
            later.push(Box::new(move |app| app.save_conflict(tab, false)));
        }
        if widgets::button(&mut orow, Kind::Default, None, "Reset", 22.0, !saving).clicked() {
            cu.picks.clear();
            cu.manual = None;
            cu.base = None;
            cu.current = 0;
            cu.revision += 1;
        }
        if cu.manual.is_none() && widgets::button(&mut orow, Kind::Default, Some(Icon::Pencil), "Edit", 22.0, !saving).tip("Type in the output directly").clicked() {
            if cu.base.is_none() {
                cu.base = Some(info.clone());
            }
            cu.manual = Some(built.clone());
            cu.revision += 1;
        }
        let ai_label = if cu.ai_busy { "Resolving…" } else { "Auto-resolve with AI" };
        if widgets::button(&mut orow, Kind::Ai, Some(Icon::Sparkles), ai_label, 22.0, !cu.ai_busy).tip("Ask the AI (Preferences > AI) to merge both sides; check the result before saving").clicked() {
            cu.ai_busy = true;
            let (base, ours, theirs, f, revision) = (info.base.clone().unwrap_or_default(), info.ours.clone().unwrap_or_default(), info.theirs.clone().unwrap_or_default(), file.to_owned(), cu.revision);
            later.push(Box::new(move |app| app.ai_resolve_conflict(tab, base, ours, theirs, f, revision)));
        }
        match cu.manual.as_mut() {
            Some(text) => {
                egui::ScrollArea::vertical().id_salt("conflict_manual").auto_shrink([false, false]).show(&mut oui, |ui| {
                    if ui.add(egui::TextEdit::multiline(text).code_editor().frame(egui::Frame::NONE.inner_margin(egui::Margin::same(8))).desired_width(f32::INFINITY)).changed() {
                        cu.revision += 1;
                    }
                });
            }
            None => {
                egui::ScrollArea::both().id_salt("conflict_output").auto_shrink([false, false]).show(&mut oui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    ui.add_space(4.0);
                    for l in output.split('\n') {
                        let marker = ["<<<<<<<", "=======", ">>>>>>>"].iter().any(|m| l.starts_with(m));
                        let (row, _) = ui.allocate_exact_size(vec2((l.len() as f32 * 7.3 + 30.0).max(orect.width()), 19.0), Sense::hover());
                        if marker {
                            ui.painter().rect_filled(row, CornerRadius::ZERO, BAD.gamma_multiply(0.1));
                        }
                        ui.painter().text(pos2(row.left() + 12.0, row.center().y), Align2::LEFT_CENTER, l, font.clone(), if marker { REMOVED } else { c.white(0.85) });
                    }
                });
            }
        }
    }

    /// The AI's merge of both sides, put in the Output for review.
    pub(crate) fn ai_merged(&mut self, tab: u64, revision: u64, result: Result<String, String>) {
        let Some(r) = self.repo_mut(tab) else { return };
        r.conflict_ui.ai_busy = false;
        match result {
            Ok(_) if r.conflict_ui.revision != revision => self.toast(false, "The conflict or your output changed. The outdated AI suggestion was not applied."),
            Ok(text) => {
                let cu = &mut r.conflict_ui;
                if cu.base.is_none()
                    && let Some(Loaded::Ready(i)) = &cu.info
                {
                    cu.base = Some(i.clone());
                }
                cu.manual = Some(text.replace("\r\n", "\n"));
                cu.revision += 1;
                self.toast(true, "AI suggestion is in the Output - review it, then Save Output");
            }
            Err(e) => self.toast(false, e),
        }
    }
}

use super::Msg;

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "a\n<<<<<<< HEAD\nmine\n=======\ntheirs\n>>>>>>> branch\nz\n";

    #[test]
    fn conflicts_parse_and_rebuild() {
        let segs = parse_conflicts(TEXT);
        assert_eq!(segs.len(), 3);
        assert!(matches!(&segs[1], Segment::Conflict { ours, theirs, .. } if ours == &["mine"] && theirs == &["theirs"]));
        assert_eq!(conflict_output(&segs, &HashMap::new(), TEXT), TEXT);
        let mut picks = HashMap::new();
        picks.insert(1, Pick { a: vec![true], b: vec![true], touched: true });
        let out = conflict_output(&segs, &picks, TEXT);
        assert_eq!(out, "a\nmine\ntheirs\nz\n");
        assert!(!has_conflict_markers(&out));
        assert!(has_conflict_markers(TEXT));
    }

    #[test]
    fn incomplete_blocks_stay_as_text() {
        let segs = parse_conflicts("<<<<<<< HEAD\nno end\n");
        assert_eq!(segs, vec![Segment::Common(vec!["<<<<<<< HEAD".into(), "no end".into()])]);
    }
}
