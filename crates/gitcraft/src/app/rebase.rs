//! Planning history edits: interactive rebase and cherry pick, and the one-click versions the
//! right-click menus offer (reword, drop, squash, move).

use super::widgets::Tip;
use super::App;
use super::dialogs::{Body, Dialog, Field, FieldKind};
use super::widgets::Kind;
use gitcraft_core::actions::Action;
use gitcraft_core::git::Commit;
use gitcraft_core::rewrite::{PlanState, Step, TodoItem};
use gitcraft_core::session::RepoView;

#[derive(Clone, Debug, PartialEq)]
pub struct RebaseItem {
    pub action: Step,
    pub command: String,
    pub sha: String,
    pub author: String,
    pub email: String,
    pub summary: String,
    /// The full message (summary and body); edited for reword.
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanMode {
    Rebase,
    Cherry,
}

#[derive(Clone, Debug)]
pub struct RebasePlan {
    pub expected: Option<PlanState>,
    pub mode: PlanMode,
    /// The commit (or `--root`) the rebase replays onto; unused for cherry pick.
    pub base: String,
    pub onto: String,
    /// Newest first, as shown.
    pub items: Vec<RebaseItem>,
    pub original: Vec<RebaseItem>,
    /// The row with keyboard focus.
    pub focus: usize,
    pub editing: Option<usize>,
    pub skip_hooks: bool,
}

pub fn to_item(c: &Commit) -> RebaseItem {
    RebaseItem {
        action: Step::Pick,
        command: String::new(),
        sha: c.sha.clone(),
        author: c.author.clone(),
        email: c.email.clone(),
        summary: c.summary.clone(),
        message: if c.body.is_empty() { c.summary.clone() } else { format!("{}\n\n{}", c.summary, c.body) },
    }
}

/// Commits on the checked-out branch's first-parent line, newest first, up to the first merge.
pub fn head_line(repo: &RepoView) -> Vec<String> {
    let by: std::collections::HashMap<&str, &Commit> = repo.data.commits.iter().map(|c| (c.sha.as_str(), c)).collect();
    let mut out = Vec::new();
    let mut c = by.get(repo.data.head_sha.as_str()).copied();
    while let Some(commit) = c {
        out.push(commit.sha.clone());
        if commit.parents.len() != 1 {
            break;
        }
        c = by.get(commit.parents[0].as_str()).copied();
    }
    out
}

/// What a rebase that rewrites `sha` and everything after it replays onto.
pub fn base_for(repo: &RepoView, sha: &str) -> String {
    repo.data.commits.iter().find(|c| c.sha == sha).and_then(|c| c.parents.first().cloned()).unwrap_or_else(|| "--root".into())
}

/// The shown plan (newest first) in Git's order (oldest first).
pub fn todo(items: &[RebaseItem]) -> Vec<TodoItem> {
    items
        .iter()
        .rev()
        .flat_map(|item| {
            let mut out = vec![TodoItem { action: item.action.clone(), sha: item.sha.clone(), message: (item.action == Step::Reword).then(|| item.message.clone()) }];
            if item.action != Step::Drop && !item.command.trim().is_empty() {
                out.push(TodoItem { action: Step::Exec, sha: item.sha.clone(), message: Some(item.command.trim().to_owned()) });
            }
            out
        })
        .collect()
}

/// Move `fixup!` and `squash!` commits under the commit they name.
pub fn autosquash(items: &[RebaseItem]) -> Vec<RebaseItem> {
    let mut ordered: Vec<RebaseItem> = Vec::new();
    let mut targets: std::collections::HashMap<String, String> = Default::default();
    for item in items.iter().rev() {
        // A commit marked Drop stays where it is, whatever its subject says.
        if item.action == Step::Drop {
            ordered.push(item.clone());
            continue;
        }
        let marker = ["fixup! ", "squash! "].iter().find_map(|p| item.summary.strip_prefix(p).map(|rest| (*p, rest)));
        let Some((kind, mut subject)) = marker else {
            ordered.push(item.clone());
            continue;
        };
        while let Some(rest) = subject.strip_prefix("fixup! ").or_else(|| subject.strip_prefix("squash! ")) {
            subject = rest;
        }
        let hexish = (4..=64).contains(&subject.len()) && subject.bytes().all(|b| b.is_ascii_hexdigit());
        let eligible: Vec<usize> = ordered.iter().enumerate().filter(|(_, c)| c.action != Step::Drop && !targets.contains_key(&c.sha)).map(|(i, _)| i).collect();
        let target = eligible
            .iter()
            .copied()
            .find(|i| ordered[*i].summary == subject || (hexish && ordered[*i].sha.starts_with(&subject.to_lowercase())))
            .or_else(|| eligible.iter().copied().find(|i| ordered[*i].summary.starts_with(subject)));
        let Some(t) = target else {
            ordered.push(item.clone());
            continue;
        };
        let target_sha = ordered[t].sha.clone();
        let mut position = t + 1;
        while position < ordered.len() && targets.get(&ordered[position].sha) == Some(&target_sha) {
            position += 1;
        }
        targets.insert(item.sha.clone(), target_sha);
        let mut moved = item.clone();
        moved.action = if kind == "fixup! " { Step::Fixup } else { Step::Squash };
        ordered.insert(position, moved);
    }
    ordered.reverse();
    ordered
}

/// A quick edit: what to do with the plan's items (None = nothing).
type Edit = Box<dyn FnOnce(Vec<RebaseItem>) -> Option<Vec<RebaseItem>> + Send>;

impl App {
    /// Open the Interactive Rebase panel for the commits after `base` (newest first).
    pub(crate) fn open_rebase(&mut self, tab: u64, base: &str, onto: &str, autosquash_items: bool) {
        let Some(path) = self.repo(tab).map(|r| r.path.clone()) else { return };
        let (base, onto) = (base.to_owned(), onto.to_owned());
        self.spawn(move || {
            let result = gitcraft_core::session::rebase_plan(&path, &base).map(|(expected, commits)| {
                let original: Vec<RebaseItem> = commits.iter().map(to_item).rev().collect();
                let items = if autosquash_items { autosquash(&original) } else { original.clone() };
                RebasePlan { expected: Some(expected), mode: PlanMode::Rebase, base, onto, items, original, focus: 0, editing: None, skip_hooks: false }
            });
            super::Msg::Plan { tab, result }
        });
    }

    pub(crate) fn plan_ready(&mut self, tab: u64, result: Result<RebasePlan, String>) {
        match result {
            Ok(plan) if plan.items.is_empty() => self.toast(false, "There are no commits to rebase."),
            Ok(plan) => {
                if let Some(r) = self.repo_mut(tab) {
                    r.ui.rebase = Some(plan);
                    r.ui.diff = None;
                    r.ui.history = None;
                    r.ui.conflict = None;
                }
            }
            Err(e) => self.toast(false, e),
        }
    }

    /// The Interactive Cherry Pick panel for the picked commits.
    pub(crate) fn open_cherry_pick(&mut self, tab: u64, shas: &[String]) {
        let Some(r) = self.repo_mut(tab) else { return };
        let Some(v) = &r.view else { return };
        let items: Vec<RebaseItem> = v.data.commits.iter().filter(|c| shas.contains(&c.sha)).map(to_item).collect();
        let onto = v.data.head_branch.clone().unwrap_or_else(|| v.data.head_sha.chars().take(6).collect());
        r.ui.rebase = Some(RebasePlan { expected: None, mode: PlanMode::Cherry, base: String::new(), onto, original: items.clone(), items, focus: 0, editing: None, skip_hooks: false });
        r.ui.diff = None;
        r.ui.history = None;
        r.ui.conflict = None;
    }

    pub(crate) fn run_plan(&mut self, tab: u64) {
        let Some(r) = self.repo_mut(tab) else { return };
        let Some(plan) = r.ui.rebase.take() else { return };
        r.ui.multi.clear();
        let items = todo(&plan.items);
        let action = match plan.mode {
            PlanMode::Rebase => match plan.expected {
                Some(expected) => Action::InteractiveRebase { base: plan.base, items, expected, skip_hooks: plan.skip_hooks },
                None => return,
            },
            PlanMode::Cherry => Action::InteractiveCherryPick { items: items.into_iter().filter(|i| i.action != Step::Drop).collect(), skip_hooks: plan.skip_hooks },
        };
        self.act(tab, action);
    }

    /// A one-click edit: read the plan, change it, and run the rebase straight away.
    fn quick(&mut self, tab: u64, base: String, edit: Edit) {
        let Some(path) = self.repo(tab).map(|r| r.path.clone()) else { return };
        self.spawn(move || {
            let result = gitcraft_core::session::rebase_plan(&path, &base).and_then(|(expected, commits)| {
                let items: Vec<RebaseItem> = commits.iter().map(to_item).rev().collect();
                Ok(edit(items).map(|items| Action::InteractiveRebase { base: base.clone(), items: todo(&items), expected, skip_hooks: false }))
            });
            super::Msg::QuickEdit { tab, result }
        });
    }

    pub(crate) fn quick_ready(&mut self, tab: u64, result: Result<Option<Action>, String>) {
        match result {
            Ok(Some(action)) => self.act(tab, action),
            Ok(None) => {}
            Err(e) => self.toast(false, e),
        }
    }

    fn oldest(repo: &RepoView, shas: &[String]) -> Option<String> {
        let line = head_line(repo);
        shas.iter().max_by_key(|s| line.iter().position(|l| l == *s).unwrap_or(0)).cloned()
    }

    pub(crate) fn reword_commit(&mut self, tab: u64, sha: &str) {
        let Some(v) = self.repo(tab).and_then(|r| r.view.as_ref()) else { return };
        let Some(c) = v.data.commits.iter().find(|c| c.sha == sha) else { return };
        let line = head_line(v);
        let n = line.iter().position(|l| l == sha).map_or(1, |p| p + 1);
        let message = if c.body.is_empty() { c.summary.clone() } else { format!("{}\n\n{}", c.summary, c.body) };
        let base = base_for(v, sha);
        let help = format!(
            "Renaming changes this commit's ID{}. Review the message before saving. You can undo this with Undo.",
            if n > 1 { format!(" and the IDs of {} newer commit{}", n - 1, if n == 2 { "" } else { "s" }) } else { String::new() }
        );
        let sha = sha.to_owned();
        let d = Dialog::new("Rename commit", Body::Form { intro: String::new(), fields: vec![Field::new("value", "Commit message (first line is the summary)", FieldKind::Multiline(8)).help(&help)], grid: false })
            .width(560.0)
            .text_value("value", &message)
            .checked(|v| v.text("value").trim().is_empty().then(String::new))
            .choice("Rename commit", Kind::Primary, move |app, values| {
                let msg = values.text("value");
                app.quick(tab, base, Box::new(move |items| Some(items.into_iter().map(|mut i| {
                    if i.sha == sha {
                        i.action = Step::Reword;
                        i.message = msg.clone();
                    }
                    i
                }).collect())));
            });
        self.dialog = Some(d);
    }

    pub(crate) fn drop_commits(&mut self, tab: u64, shas: Vec<String>) {
        let Some(v) = self.repo(tab).and_then(|r| r.view.as_ref()) else { return };
        let Some(old) = Self::oldest(v, &shas) else { return };
        let base = base_for(v, &old);
        let n = shas.len();
        let title = if n == 1 { "Drop commit".to_owned() } else { format!("Drop {n} commits") };
        let text = if n == 1 { "Are you sure you want to drop 1 commit?".to_owned() } else { format!("Are you sure you want to drop {n} commits?") };
        self.dialog = Some(Dialog::confirm_with(&title, &text, &title, true, move |app, _| {
            app.quick(tab, base, Box::new(move |items| Some(items.into_iter().map(|mut i| {
                if shas.contains(&i.sha) {
                    i.action = Step::Drop;
                }
                i
            }).collect())));
        }));
    }

    /// Squash the picked commits into the commit just below the oldest of them.
    pub(crate) fn squash_commits(&mut self, tab: u64, shas: Vec<String>) {
        let Some(v) = self.repo(tab).and_then(|r| r.view.as_ref()) else { return };
        let line = head_line(v);
        let Some(old) = Self::oldest(v, &shas) else { return };
        // Squashing commits apart from each other would reorder the ones between them.
        let mut at: Vec<usize> = shas.iter().filter_map(|s| line.iter().position(|l| l == s)).collect();
        at.sort_unstable();
        if at.len() != shas.len() || at.windows(2).any(|w| w[1] != w[0] + 1) {
            return self.toast(false, "Pick commits next to each other to squash them.");
        }
        let Some(target) = line.iter().position(|l| *l == old).and_then(|p| line.get(p + 1)).cloned() else {
            return self.toast(false, "There is no older commit to squash into.");
        };
        let base = base_for(v, &target);
        self.quick(tab, base, Box::new(move |items| Some(items.into_iter().map(|mut i| {
            if shas.contains(&i.sha) {
                i.action = Step::Squash;
            }
            i
        }).collect())));
    }

    /// Move a commit one place newer (`up`) or older.
    pub(crate) fn move_commit(&mut self, tab: u64, sha: &str, up: bool) {
        let Some(v) = self.repo(tab).and_then(|r| r.view.as_ref()) else { return };
        let line = head_line(v);
        let Some(i) = line.iter().position(|l| l == sha) else { return };
        let other = if up { i.checked_sub(1).and_then(|j| line.get(j)) } else { line.get(i + 1) };
        let Some(other) = other.cloned() else { return };
        let lower = if up { sha.to_owned() } else { other.clone() };
        let base = base_for(v, &lower);
        let sha = sha.to_owned();
        self.quick(tab, base, Box::new(move |mut items| {
            let a = items.iter().position(|x| x.sha == sha)?;
            let b = items.iter().position(|x| x.sha == other)?;
            items.swap(a, b);
            Some(items)
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(sha: &str, summary: &str) -> RebaseItem {
        RebaseItem { action: Step::Pick, command: String::new(), sha: sha.into(), author: String::new(), email: String::new(), summary: summary.into(), message: summary.into() }
    }

    #[test]
    fn fixups_move_under_their_commit() {
        // Newest first.
        let items = vec![item("c", "fixup! Add login"), item("b", "Other"), item("a", "Add login")];
        let out = autosquash(&items);
        let order: Vec<&str> = out.iter().map(|i| i.sha.as_str()).collect();
        assert_eq!(order, ["b", "c", "a"]);
        assert_eq!(out[1].action, Step::Fixup);
    }

    #[test]
    fn a_dropped_fixup_stays_where_it_is() {
        let mut dropped = item("c", "fixup! Add login");
        dropped.action = Step::Drop;
        let out = autosquash(&[dropped, item("b", "Other"), item("a", "Add login")]);
        assert_eq!(out.iter().map(|i| i.sha.as_str()).collect::<Vec<_>>(), ["c", "b", "a"]);
        assert_eq!(out[0].action, Step::Drop);
    }

    #[test]
    fn todo_is_oldest_first_with_exec_lines() {
        let mut a = item("a", "A");
        a.command = "make test".into();
        let b = item("b", "B");
        let t = todo(&[b, a]);
        assert_eq!(t.iter().map(|i| i.sha.as_str()).collect::<Vec<_>>(), ["a", "a", "b"]);
        assert_eq!(t[1].action, Step::Exec);
    }
}

const STEPS: [(Step, &str, &str, egui::Color32); 6] = [
    (Step::Pick, "Pick", "P", egui::Color32::from_rgb(0x5c, 0xb8, 0x5c)),
    (Step::Reword, "Reword", "R", egui::Color32::from_rgb(0x4d, 0x88, 0xff)),
    (Step::Squash, "Squash", "S", egui::Color32::from_rgb(0xde, 0x9b, 0x43)),
    (Step::Fixup, "Fixup", "F", egui::Color32::from_rgb(0xb1, 0x7b, 0xcb)),
    (Step::Edit, "Edit", "E", egui::Color32::from_rgb(0x17, 0x8b, 0x8f)),
    (Step::Drop, "Drop", "D", egui::Color32::from_rgb(0xd9, 0x41, 0x3d)),
];

impl App {
    /// Interactive Rebase or Cherry Pick in the centre, newest commit at the top.
    pub(crate) fn rebase_view(&self, ui: &mut egui::Ui, tab: u64, r: &mut super::repo::RepoTab, later: &mut Vec<super::ctxmenu::Later>) {
        use super::icons::{self, Icon};
        use super::theme::p;
        use super::widgets;
        use egui::{Align2, CornerRadius, FontId, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};
        let c = p();
        let Some(plan) = r.ui.rebase.as_mut() else { return };
        let cherry = plan.mode == PlanMode::Cherry;
        let steps: Vec<(Step, &str, &str, egui::Color32)> = STEPS.iter().filter(|s| !(cherry && s.0 == Step::Edit)).cloned().collect();
        let n = plan.items.len();
        let mut close = false;
        let mut set_action: Option<(usize, Step)> = None;
        let mut moves: Option<(usize, isize)> = None;
        let mut drop_to: Option<(usize, usize)> = None;
        // Keys, unless a text field has them.
        let typing = ui.ctx().egui_wants_keyboard_input();
        if !typing && !super::dialog_open(ui.ctx()) && plan.editing.is_none() {
            ui.input(|i| {
                for (step, _, key, _) in &steps {
                    let k = egui::Key::from_name(key).expect("letters are keys");
                    if i.key_pressed(k) && i.modifiers.is_none() {
                        set_action = Some((plan.focus, step.clone()));
                    }
                }
                let dir = if i.key_pressed(egui::Key::ArrowUp) { -1 } else if i.key_pressed(egui::Key::ArrowDown) { 1 } else { 0 };
                if dir != 0 {
                    if i.modifiers.command || i.modifiers.alt {
                        moves = Some((plan.focus, dir));
                    } else {
                        plan.focus = (plan.focus as isize + dir).clamp(0, n.saturating_sub(1) as isize) as usize;
                    }
                }
                if i.key_pressed(egui::Key::Escape) {
                    close = true;
                }
            });
        }
        let w = ui.available_width();
        let (bar, _) = ui.allocate_exact_size(vec2(w, 34.0), Sense::hover());
        ui.painter().rect_filled(bar, CornerRadius::ZERO, c.toolbar.gamma_multiply(0.6));
        ui.painter().line_segment([bar.left_bottom(), bar.right_bottom()], Stroke::new(1.0, c.black(0.3)));
        let title = ui.painter().text(pos2(bar.left() + 12.0, bar.center().y), Align2::LEFT_CENTER, if cherry { "Interactive Cherry Pick" } else { "Interactive Rebase" }, super::fonts::semibold(13.5), c.white(1.0));
        let detail = if cherry { format!("Cherry picking {n} commits onto {}", plan.onto) } else { format!("Rebasing {n} commits onto {}", if plan.onto == "--root" { "the start of history" } else { &plan.onto }) };
        let g = widgets::fit(ui, &detail, FontId::proportional(12.5), c.fg2, bar.right() - title.right() - 50.0);
        ui.painter().galley(pos2(title.right() + 8.0, bar.center().y - g.size().y / 2.0), g, c.fg2);
        let xr = Rect::from_min_size(pos2(bar.right() - 32.0, bar.center().y - 12.0), Vec2::splat(24.0));
        let xresp = ui.interact(xr, ui.scope_id().with("rebase_close"), Sense::click()).tip("Cancel (Esc)");
        icons::paint(ui, xr, Icon::X, 16.0, if xresp.hovered() { c.white(1.0) } else { c.fg2 });
        if xresp.clicked() {
            close = true;
        }
        egui::Frame::new().inner_margin(egui::Margin::symmetric(12, 6)).show(ui, |ui| {
            ui.set_width(w - 24.0);
            ui.add(egui::Label::new(egui::RichText::new("Newest at the top. Drag rows or use Ctrl+↑ ↓ to reorder. Squash combines messages, Fixup keeps the earlier message, and Edit pauses for changes.").size(11.5).color(c.fg3)).wrap());
            if plan.items.iter().any(|i| i.action == Step::Reword) {
                widgets::checkbox(ui, &mut plan.skip_hooks, "Skip checks that Git allows rewording to bypass", 11.5);
            }
        });
        let (line, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
        ui.painter().rect_filled(line, CornerRadius::ZERO, c.white(0.05));
        let footer_h = 44.0;
        let list_h = (ui.available_height() - footer_h).max(60.0);
        let drag_id = ui.scope_id().with("rebase_drag");
        let dragging: Option<usize> = ui.data(|d| d.get_temp(drag_id));
        let mut new_drag = dragging;
        egui::ScrollArea::vertical().id_salt(("rebase", tab)).max_height(list_h).min_scrolled_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            // While a message is edited, the order stays put, so the message stays with its commit.
            let locked = plan.editing.is_some();
            for i in 0..n {
                let it = plan.items[i].clone();
                let selected = plan.focus == i;
                let dropped = it.action == Step::Drop;
                let editing = plan.editing == Some(i);
                let extra = if !cherry && !dropped && selected { 30.0 } else if !cherry && !dropped && !it.command.trim().is_empty() { 22.0 } else { 0.0 } + if editing { 130.0 } else { 0.0 };
                let (row, resp) = ui.allocate_exact_size(vec2(w, 34.0 + extra), Sense::click_and_drag());
                ui.painter().rect_filled(row, CornerRadius::ZERO, if selected { c.selrow } else if resp.hovered() { c.hoverrow } else { egui::Color32::TRANSPARENT });
                ui.painter().line_segment([row.left_bottom(), row.right_bottom()], Stroke::new(1.0, c.white(0.05)));
                if resp.clicked() {
                    plan.focus = i;
                }
                if resp.drag_started() && !locked {
                    new_drag = Some(i);
                }
                if let Some(from) = dragging
                    && from != i
                    && ui.rect_contains_pointer(row)
                {
                    ui.painter().line_segment([row.left_top(), row.right_top()], Stroke::new(2.0, super::theme::ACCENT));
                    if ui.input(|inp| inp.pointer.any_released()) {
                        drop_to = Some((from, i));
                    }
                }
                let top = Rect::from_min_size(row.min, vec2(w, 34.0));
                let mut x = top.left() + 6.0;
                icons::paint(ui, Rect::from_min_size(pos2(x, top.center().y - 7.0), Vec2::splat(14.0)), Icon::GripVertical, 14.0, c.fg3);
                x += 20.0;
                let seg = Rect::from_min_size(pos2(x, top.center().y - 10.0), vec2(22.0 * steps.len() as f32, 20.0));
                ui.painter().rect_stroke(seg, CornerRadius::same(3), Stroke::new(1.0, c.white(0.1)), StrokeKind::Inside);
                for (k, (step, label, key, color)) in steps.iter().enumerate() {
                    let br = Rect::from_min_size(pos2(seg.left() + k as f32 * 22.0, seg.top()), vec2(22.0, 20.0));
                    let on = it.action == *step;
                    let b = ui.interact(br, ui.scope_id().with(("step", i, k)), Sense::click()).tip(format!("{label} ({key})"));
                    if on {
                        ui.painter().rect_filled(br.shrink(1.0), CornerRadius::ZERO, *color);
                    }
                    ui.painter().text(br.center(), Align2::CENTER_CENTER, *key, super::fonts::bold(11.0), if on || b.hovered() { c.white(1.0) } else { c.fg3 });
                    if b.clicked() {
                        plan.focus = i;
                        set_action = Some((i, step.clone()));
                    }
                }
                x = seg.right() + 8.0;
                let (_, slabel, _, scolor) = STEPS.iter().find(|s| s.0 == it.action).cloned().unwrap_or(STEPS[0].clone());
                ui.painter().text(pos2(x, top.center().y), Align2::LEFT_CENTER, slabel, super::fonts::semibold(11.0), scolor);
                x += 58.0;
                widgets::paint_avatar(ui, pos2(x + 10.0, top.center().y), 20.0, &it.author, &it.email);
                x += 28.0;
                let arrows_w = 40.0;
                let sha: String = it.sha.chars().take(6).collect();
                let shag = ui.painter().layout_no_wrap(sha, FontId::monospace(11.0), c.fg3);
                let sha_x = top.right() - 12.0 - arrows_w - shag.size().x;
                let shown = if it.action == Step::Reword { it.message.lines().next().unwrap_or("").to_owned() } else { it.summary.clone() };
                let g = widgets::fit(ui, &shown, FontId::proportional(13.0), if dropped { c.fg3 } else { c.white(0.9) }, sha_x - x - 8.0);
                let gw = g.size().x;
                ui.painter().galley(pos2(x, top.center().y - g.size().y / 2.0), g, c.fg);
                if dropped {
                    ui.painter().line_segment([pos2(x, top.center().y), pos2(x + gw, top.center().y)], Stroke::new(1.0, c.fg3));
                }
                ui.painter().galley(pos2(sha_x, top.center().y - shag.size().y / 2.0), shag, c.fg3);
                for (k, (icon, dir, tip)) in [(Icon::ArrowUp, -1isize, "Move up (Ctrl+↑)"), (Icon::ArrowDown, 1, "Move down (Ctrl+↓)")].into_iter().enumerate() {
                    let ar = Rect::from_min_size(pos2(top.right() - 12.0 - arrows_w + k as f32 * 20.0, top.center().y - 8.0), Vec2::splat(16.0));
                    let a = ui.interact(ar, ui.scope_id().with(("mv", i, k)), if locked { Sense::hover() } else { Sense::click() }).tip(if locked { "Finish editing the message first" } else { tip });
                    icons::paint(ui, ar, icon, 13.0, if locked { c.fg3.gamma_multiply(0.4) } else if a.hovered() { c.white(1.0) } else { c.fg3 });
                    if a.clicked() && !locked {
                        moves = Some((i, dir));
                    }
                }
                let mut y = top.bottom();
                if !cherry && !dropped && selected {
                    let rr = Rect::from_min_size(pos2(row.left() + 38.0, y), vec2(w - 50.0, 26.0));
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rr).layout(egui::Layout::left_to_right(egui::Align::Center)));
                    child.label(egui::RichText::new("Run after this commit").size(11.5).color(c.fg3));
                    let fw = child.available_width();
                    widgets::field(&mut child, &mut plan.items[i].command, "Optional shell command", fw, 24.0, 12.0);
                    y += 30.0;
                } else if !cherry && !dropped && !it.command.trim().is_empty() {
                    ui.painter().text(pos2(row.left() + 38.0, y + 9.0), Align2::LEFT_CENTER, format!("Run {}", it.command), FontId::monospace(11.5), c.fg3);
                    y += 22.0;
                }
                if editing {
                    let er = Rect::from_min_size(pos2(row.left() + 38.0, y), vec2(w - 50.0, 124.0));
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(er).layout(egui::Layout::top_down(egui::Align::Min)));
                    let draft_id = child.scope_id().with(("reword_draft", i));
                    let mut draft: String = child.data(|d| d.get_temp(draft_id)).unwrap_or_else(|| it.message.clone());
                    egui::Frame::new().fill(c.field).stroke(Stroke::new(1.0, c.white(0.1))).corner_radius(CornerRadius::same(3)).inner_margin(egui::Margin::symmetric(8, 6)).show(&mut child, |ui| {
                        ui.add(egui::TextEdit::multiline(&mut draft).frame(egui::Frame::NONE).font(FontId::proportional(13.0)).desired_rows(4).desired_width(f32::INFINITY));
                    });
                    child.add_space(6.0);
                    child.horizontal(|ui| {
                        if widgets::button(ui, super::widgets::Kind::Primary, None, "Update Message", 24.0, !draft.trim().is_empty()).clicked() {
                            plan.items[i].message = draft.trim().to_owned();
                            plan.editing = None;
                        }
                        if widgets::small(ui, super::widgets::Kind::Default, "Cancel").clicked() {
                            plan.editing = None;
                        }
                    });
                    child.data_mut(|d| d.insert_temp(draft_id, draft));
                }
            }
        });
        if ui.input(|i| i.pointer.any_released()) {
            new_drag = None;
        }
        ui.data_mut(|d| match new_drag {
            Some(v) => {
                d.insert_temp(drag_id, v);
            }
            None => d.remove::<usize>(drag_id),
        });
        // Apply the edits.
        if let Some((i, step)) = set_action {
            let last = i + 1 == plan.items.len();
            if i < plan.items.len() && !(cherry && matches!(step, Step::Squash | Step::Fixup) && last) {
                if step == Step::Reword {
                    plan.editing = Some(i);
                }
                plan.items[i].action = step;
            }
        }
        if let Some((i, dir)) = moves {
            let j = i as isize + dir;
            if j >= 0 && (j as usize) < plan.items.len() {
                plan.items.swap(i, j as usize);
                plan.focus = j as usize;
            }
        }
        if let Some((from, to)) = drop_to {
            let item = plan.items.remove(from);
            plan.items.insert(to, item);
            plan.focus = to;
        }
        // Footer.
        let kept = plan.items.iter().filter(|i| i.action != Step::Drop).count();
        let bottom_squash = plan.items.iter().rev().find(|i| i.action != Step::Drop).is_some_and(|i| matches!(i.action, Step::Squash | Step::Fixup));
        let (foot, _) = ui.allocate_exact_size(vec2(w, footer_h), Sense::hover());
        ui.painter().rect_filled(foot, CornerRadius::ZERO, c.panel);
        ui.painter().line_segment([foot.left_top(), foot.right_top()], Stroke::new(1.0, c.white(0.1)));
        let mut fr = ui.new_child(egui::UiBuilder::new().max_rect(foot.shrink2(vec2(12.0, 0.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
        fr.spacing_mut().item_spacing.x = 8.0;
        let busy = self.repo(tab).is_some_and(|t| t.running.is_some());
        if widgets::button(&mut fr, super::widgets::Kind::Success, None, if busy { "Working…" } else if cherry { "Start Cherry Pick" } else { "Start Rebase" }, 24.0, !busy && kept > 0 && plan.editing.is_none()).clicked() {
            later.push(Box::new(move |app| app.run_plan(tab)));
        }
        if widgets::small(&mut fr, super::widgets::Kind::Danger, if cherry { "Cancel Cherry Pick" } else { "Cancel Rebase" }).clicked() {
            close = true;
        }
        if widgets::small(&mut fr, super::widgets::Kind::Default, "Reset").clicked() {
            plan.items = plan.original.clone();
        }
        if !cherry {
            let can = !busy && plan.editing.is_none() && plan.items.iter().any(|i| i.summary.starts_with("fixup! ") || i.summary.starts_with("squash! "));
            if widgets::button(&mut fr, super::widgets::Kind::Default, None, "Autosquash", 24.0, can).clicked() {
                plan.items = autosquash(&plan.items);
            }
        }
        let note = format!("{kept} of {n} commits kept{}", if bottom_squash { " · the first commit will be picked because it has no earlier commit to combine with" } else { "" });
        let room = fr.available_width();
        let g = widgets::fit(ui, &note, FontId::proportional(12.0), c.fg3, room);
        ui.painter().galley(pos2(foot.left() + 12.0, foot.center().y - g.size().y / 2.0), g, c.fg3);
        if close {
            r.ui.rebase = None;
        }
    }
}
