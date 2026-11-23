//! The left panel: local and remote branches, worktrees, Gitflow, LFS, stashes, patches, pull
//! requests, issues, teams (conflict check), tags and submodules; and the strip of icons it
//! shrinks to while a file is open.

use super::widgets::Tip;
use super::App;
use super::ctxmenu::{self, Entry, Later, item};
use super::icons::{self, Icon};
use super::repo::{RepoTab, Selection};
use super::theme::{ACCENT, GOOD, WARN, p};
use super::widgets::{self, Kind};
use egui::{Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use gitcraft_core::actions::Action;
use gitcraft_core::git::{GitRef, RefKind};
use gitcraft_core::session::RepoView;

/// What the side sections read besides the repository itself.
#[derive(Default)]
pub struct SideData {
    pub generation: u64,
    pub gitflow: Option<gitcraft_core::manage::GitflowConfig>,
    pub lfs: Option<gitcraft_core::manage::LfsInfo>,
    pub locks: Option<Result<Vec<gitcraft_core::manage::LfsLock>, String>>,
    /// A lock list is being read: no second request meanwhile.
    pub locks_loading: bool,
    pub submodules: Option<Vec<gitcraft_core::manage::Submodule>>,
    pub conflicts: Option<gitcraft_core::extras::ConflictCheck>,
}

const SECTIONS: [&str; 12] = ["local", "remote", "worktrees", "gitflow", "lfs", "stashes", "patches", "prs", "issues", "teams", "tags", "submodules"];

impl App {
    /// Read the side sections' data again after the repository changed.
    pub(crate) fn want_side(&mut self, tab: u64) {
        let detect = self.settings.flag("conflictDetection", true);
        let Some(r) = self.repo_mut(tab) else { return };
        if r.side.generation == r.generation || r.view.is_none() {
            return;
        }
        r.side.generation = r.generation;
        let path = r.path.clone();
        let detect = detect && self.repo_pref(&path, "conflictDetection").and_then(|v| v.as_bool()).unwrap_or(true);
        let generation = self.repo(tab).map_or(0, |r| r.generation);
        self.spawn(move || {
            let dir = std::path::Path::new(&path);
            let side = SideData {
                generation,
                gitflow: Some(gitcraft_core::manage::gitflow_config(dir)),
                lfs: Some(gitcraft_core::manage::lfs_info(dir)),
                locks: None,
                locks_loading: false,
                submodules: Some(gitcraft_core::manage::submodules(dir)),
                conflicts: detect.then(|| gitcraft_core::extras::conflict_check(dir, None)),
            };
            super::Msg::Side { tab, side: Box::new(side) }
        });
    }

    pub(crate) fn side_loaded(&mut self, tab: u64, side: SideData) {
        if let Some(r) = self.repo_mut(tab)
            && r.side.generation == side.generation
        {
            let (locks, loading) = (r.side.locks.take(), r.side.locks_loading);
            r.side = side;
            r.side.locks = locks;
            r.side.locks_loading = loading;
        }
    }

    pub(crate) fn left_panel(&self, ui: &mut Ui, tab: u64, r: &mut RepoTab, later: &mut Vec<Later>) {
        let c = p();
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, CornerRadius::ZERO, c.panel);
        let agents_on = self.settings.flag("showAgentsView", true);
        let view = self.settings.string("leftView", "list");
        // Collapse button and the List / Agents switch.
        ui.add_space(11.0);
        ui.horizontal(|ui| {
            ui.set_height(34.0);
            ui.add_space(11.0);
            let (b, resp) = ui.allocate_exact_size(Vec2::splat(17.0), Sense::click());
            ui.painter().circle_filled(b.center(), 8.5, if resp.hovered() { c.white(1.0) } else { c.white(0.85) });
            icons::paint(ui, b, Icon::ChevronLeft, 13.0, c.panel);
            if resp.tip("Collapse Panel (Ctrl+J)").clicked() {
                later.push(Box::new(|app| app.settings.set("showLeft", false)));
            }
            ui.add_space(9.0);
            let w = ui.available_width() - 11.0;
            let (sw, _) = ui.allocate_exact_size(vec2(w, 24.0), Sense::hover());
            ui.painter().rect_stroke(sw, CornerRadius::same(3), Stroke::new(1.0, c.white(0.1)), StrokeKind::Inside);
            let parts: Vec<(&str, Icon, &str)> = if agents_on { vec![("list", Icon::List, "List"), ("agents", Icon::Bot, "Agents")] } else { vec![("list", Icon::List, "List")] };
            let pw = sw.width() / parts.len() as f32;
            for (i, (id, icon, label)) in parts.iter().enumerate() {
                let pr = Rect::from_min_size(pos2(sw.left() + i as f32 * pw, sw.top()), vec2(pw, sw.height()));
                let on = view == *id || (!agents_on && *id == "list");
                let resp = ui.interact(pr, ui.scope_id().with(("leftview", id)), Sense::click());
                if on {
                    ui.painter().rect(pr, CornerRadius::same(3), ACCENT.gamma_multiply(0.25), Stroke::new(1.0, ACCENT.gamma_multiply(0.7)), StrokeKind::Inside);
                } else {
                    ui.painter().rect_filled(pr.shrink(1.0), CornerRadius::ZERO, c.white(0.05));
                }
                let ink = if on || resp.hovered() { c.white(1.0) } else { c.fg2 };
                let g = ui.painter().layout_no_wrap(label.to_string(), FontId::proportional(13.0), ink);
                let total = 14.0 + 6.0 + g.size().x;
                let x0 = pr.center().x - total / 2.0;
                icons::paint(ui, Rect::from_min_size(pos2(x0, pr.center().y - 7.0), Vec2::splat(14.0)), *icon, 14.0, ink);
                ui.painter().galley(pos2(x0 + 20.0, pr.center().y - g.size().y / 2.0), g, ink);
                if resp.clicked() {
                    let id = id.to_string();
                    later.push(Box::new(move |app| app.settings.set("leftView", id)));
                }
            }
        });
        if view == "agents" && agents_on {
            return self.agents_view(ui, tab, r, later);
        }
        let Some(v) = r.view.as_ref() else {
            ui.add_space(20.0);
            ui.label(egui::RichText::new("Loading…").size(12.0).color(c.fg3));
            return;
        };
        let path = v.path.clone();
        let hidden = self.hidden_refs(&path);
        let solo = self.solo_refs(&path);
        let viewing = if solo.is_empty() { v.data.refs.iter().filter(|r| r.kind != RefKind::Tag && !hidden.contains(&r.full)).count() } else { solo.len() };
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            ui.label(egui::RichText::new("Viewing").size(13.0).color(c.fg));
            ui.label(egui::RichText::new(viewing.to_string()).size(13.0).strong().color(c.link));
        });
        ui.add_space(6.0);
        // Filter (Ctrl+Alt+F).
        let filter_id = ui.scope_id().with(("left_filter", tab));
        let mut filter: String = ui.data(|d| d.get_temp(filter_id)).unwrap_or_default();
        ui.horizontal(|ui| {
            ui.add_space(9.0);
            let w = ui.available_width() - 9.0;
            let (fr, _) = ui.allocate_exact_size(vec2(w, 22.0), Sense::hover());
            let id = ui.scope_id().with(("left_filter_edit", tab));
            let focused = ui.memory(|m| m.has_focus(id));
            ui.painter().rect(fr, CornerRadius::same(3), c.field, Stroke::new(1.0, if focused { ACCENT } else { c.white(0.1) }), StrokeKind::Inside);
            icons::paint(ui, Rect::from_min_size(pos2(fr.right() - 19.0, fr.center().y - 7.0), Vec2::splat(14.0)), Icon::Search, 14.0, c.white(0.7));
            let resp = ui.put(
                Rect::from_min_max(fr.min + vec2(5.0, 0.0), pos2(fr.right() - 22.0, fr.bottom())),
                egui::TextEdit::singleline(&mut filter).id(id).frame(egui::Frame::NONE).font(FontId::proportional(12.5)).hint_text(egui::RichText::new("Filter (Ctrl + Alt + f)").color(c.fg3)).align(Align2::LEFT_CENTER),
            );
            if self.focus_left_filter.get() {
                resp.request_focus();
                self.focus_left_filter.set(false);
            }
            if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                filter.clear();
                resp.surrender_focus();
            }
        });
        ui.data_mut(|d| d.insert_temp(filter_id, filter.clone()));
        ui.add_space(12.0);
        let f = filter.trim().to_lowercase();
        let matches = |n: &str| f.is_empty() || n.to_lowercase().contains(&f);
        let open_id = ui.scope_id().with(("left_open", tab));
        let mut open: std::collections::HashMap<String, bool> = ui.data(|d| d.get_temp(open_id)).unwrap_or_default();
        let is_open = |k: &str, open: &std::collections::HashMap<String, bool>| !f.is_empty() || open.get(k).copied().unwrap_or(false);
        let mut toggled: Option<String> = None;
        let locals: Vec<&GitRef> = v.data.refs.iter().filter(|r| r.kind == RefKind::Local && matches(&r.name)).collect();
        let remotes: Vec<&GitRef> = v.data.refs.iter().filter(|r| r.kind == RefKind::Remote && matches(&r.name)).collect();
        let mut tags: Vec<&GitRef> = v.data.refs.iter().filter(|r| r.kind == RefKind::Tag && matches(&r.name)).collect();
        tags.sort_by(|a, b| natural_cmp(&b.name, &a.name));
        let stashes: Vec<_> = v.data.stashes.iter().filter(|s| matches(&s.message)).collect();
        let worktrees: Vec<_> = v.data.worktrees.iter().filter(|w| matches(&w.path) || matches(&w.branch)).collect();
        let mut by_remote: Vec<(String, Vec<&GitRef>)> = Vec::new();
        for rr in &remotes {
            let (name, _) = ctxmenu::split_remote(v, &rr.name).unwrap_or_else(|| (rr.name.split('/').next().unwrap_or("").to_owned(), String::new()));
            match by_remote.iter_mut().find(|(n, _)| *n == name) {
                Some((_, list)) => list.push(rr),
                None => by_remote.push((name, vec![rr])),
            }
        }
        if f.is_empty() {
            for rem in &v.data.remotes {
                if !by_remote.iter().any(|(n, _)| n == rem) {
                    by_remote.push((rem.clone(), Vec::new()));
                }
            }
        }
        let mut select_sha: Option<String> = None;
        egui::ScrollArea::vertical().id_salt(("left", tab)).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            // LOCAL
            let head = v.data.head_branch.clone();
            let local_full: Vec<GitRef> = locals.iter().map(|r| (*r).clone()).collect();
            let resp = section(ui, "LOCAL", Icon::Laptop, Some(locals.len()), is_open("local", &open), Some((Icon::Brush, "Clean up local branches")), &mut |_| later.push(Box::new(move |app| app.cleanup_dialog(tab))));
            if resp.0 {
                toggled = Some("local".into());
            }
            self.hide_all_menu(&resp.1, tab, v, &local_full, "local branches", later);
            if is_open("local", &open) {
                let mut folders: Vec<(String, Vec<&GitRef>)> = Vec::new();
                let mut plain: Vec<&GitRef> = Vec::new();
                for rr in &locals {
                    match rr.name.split_once('/') {
                        Some((folder, _)) if !folder.is_empty() => match folders.iter_mut().find(|(n, _)| n == folder) {
                            Some((_, l)) => l.push(rr),
                            None => folders.push((folder.to_owned(), vec![rr])),
                        },
                        _ => plain.push(rr),
                    }
                }
                for (folder, items) in &folders {
                    row(ui, 1, Icon::Folder, folder, None, false, None);
                    for rr in items {
                        self.branch_row(ui, tab, v, rr, 2, &rr.name[folder.len() + 1..], &hidden, &solo, head.as_deref(), later, &mut select_sha);
                    }
                }
                for rr in &plain {
                    self.branch_row(ui, tab, v, rr, 1, &rr.name, &hidden, &solo, head.as_deref(), later, &mut select_sha);
                }
                ui.add_space(4.0);
            }
            // REMOTE
            let remote_full: Vec<GitRef> = by_remote.iter().flat_map(|(_, l)| l.iter().map(|r| (*r).clone())).collect();
            let resp = section(ui, "REMOTE", Icon::Cloud, Some(by_remote.len()), is_open("remote", &open), Some((Icon::Plus, "Add a remote")), &mut |_| later.push(Box::new(move |app| app.add_remote(tab))));
            if resp.0 {
                toggled = Some("remote".into());
            }
            self.hide_all_menu(&resp.1, tab, v, &remote_full, "remotes", later);
            if is_open("remote", &open) {
                for (rem, refs) in &by_remote {
                    let rr = row(ui, 1, Icon::Cloud, rem, None, true, None);
                    let (rem2, refs2): (String, Vec<GitRef>) = (rem.clone(), refs.iter().map(|r| (*r).clone()).collect());
                    ctxmenu::attach(&rr, later, || {
                        let mut items = vec![
                            item(format!("Fetch {rem2}"), ctxmenu::act(tab, Action::FetchRemote { remote: rem2.clone() })),
                            {
                                let r = rem2.clone();
                                item(format!("Edit {rem2}"), move |app| app.edit_remote(tab, &r))
                            },
                            {
                                let r = rem2.clone();
                                item(format!("Open Remote: {rem2}"), move |app| app.open_web_link(tab, &r, &super::weblinks::Target::Repo))
                            },
                            Entry::Separator,
                        ];
                        items.extend(self.hide_all_entries(tab, v, &refs2, &format!("{rem2} branches")));
                        items.push(Entry::Separator);
                        let r = rem2.clone();
                        items.push(ctxmenu::danger(format!("Remove {rem2}"), move |app| app.remove_remote(tab, &r)));
                        items
                    });
                    for rr in refs {
                        let label = rr.name.strip_prefix(&format!("{rem}/")).unwrap_or(&rr.name).to_owned();
                        self.branch_row(ui, tab, v, rr, 2, &label, &hidden, &solo, head.as_deref(), later, &mut select_sha);
                    }
                }
                ui.add_space(4.0);
            }
            // WORKTREES
            let resp = section(ui, "WORKTREES", Icon::Network, Some(worktrees.len()), is_open("worktrees", &open), Some((Icon::Plus, "Add a new worktree")), &mut |_| later.push(Box::new(move |app| app.add_worktree(tab, None))));
            if resp.0 {
                toggled = Some("worktrees".into());
            }
            if is_open("worktrees", &open) {
                let main = v.data.worktrees.first().map(|w| w.path.clone());
                for w in &worktrees {
                    let is_main = Some(&w.path) == main.as_ref();
                    let label = if w.branch.is_empty() { w.path.clone() } else { w.branch.clone() };
                    let note = if is_main { "main worktree".to_owned() } else { w.path.clone() };
                    let rr = row(ui, 1, if is_main { Icon::Check } else { Icon::Folder }, &label, Some(&note), false, if is_main { Some(GOOD) } else { None }).tip(&w.path);
                    if rr.double_clicked() {
                        let p2 = w.path.clone();
                        later.push(Box::new(move |app| app.open_repo(&p2)));
                    }
                    let (wp, locked) = (w.path.clone(), w.locked);
                    ctxmenu::attach(&rr, later, || {
                        let p1 = wp.clone();
                        let mut items = vec![item("Open worktree", move |app| app.open_repo(&p1))];
                        if !is_main {
                            let p2 = wp.clone();
                            items.push(if locked { item("Unlock worktree", ctxmenu::act(tab, Action::WorktreeUnlock { path: p2 })) } else { item("Lock worktree", move |app| app.lock_worktree(tab, &p2)) });
                            items.push(Entry::Separator);
                            let p3 = wp.clone();
                            items.push(ctxmenu::off(ctxmenu::danger("Remove worktree", move |app| app.remove_worktree(tab, &p3)), !locked));
                        }
                        items
                    });
                }
                ui.add_space(4.0);
            }
            // GITFLOW
            if let Some(gf) = r.side.gitflow.as_ref().filter(|g| g.initialized) {
                let resp = section(ui, "GITFLOW", Icon::GitFork, None, is_open("gitflow", &open), Some((Icon::Plus, "Gitflow settings")), &mut |_| later.push(Box::new(move |app| app.init_gitflow(tab))));
                if resp.0 {
                    toggled = Some("gitflow".into());
                }
                if is_open("gitflow", &open) {
                    ui.horizontal(|ui| {
                        ui.add_space(28.0);
                        ui.spacing_mut().item_spacing.x = 6.0;
                        for (kind, label) in [("feature", "Start Feature"), ("release", "Start Release"), ("hotfix", "Start Hotfix")] {
                            if widgets::button(ui, Kind::Default, None, label, 22.0, true).clicked() {
                                later.push(Box::new(move |app| app.start_gitflow(tab, kind)));
                            }
                        }
                    });
                    ui.add_space(4.0);
                    for (kind, prefix) in [("feature", gf.feature.clone()), ("release", gf.release.clone()), ("hotfix", gf.hotfix.clone())] {
                        if prefix.is_empty() {
                            continue;
                        }
                        for rr in v.data.refs.iter().filter(|r| r.kind == RefKind::Local && r.name.starts_with(&prefix)) {
                            let resp = row(ui, 1, Icon::GitFork, &rr.name, None, false, None).tip(format!("Right-click to finish this {kind}"));
                            let name = rr.name[prefix.len()..].to_owned();
                            let full = rr.name.clone();
                            ctxmenu::attach(&resp, later, || vec![item(format!("Finish {full}"), move |app| app.finish_gitflow(tab, kind, &name))]);
                        }
                    }
                }
            }
            // LFS
            if let Some(lfs) = r.side.lfs.as_ref().filter(|l| l.available && (l.enabled || !l.patterns.is_empty())) {
                let resp = section(ui, "LFS", Icon::Package, Some(lfs.patterns.len()), is_open("lfs", &open), None, &mut |_| {});
                if resp.0 {
                    toggled = Some("lfs".into());
                    if !is_open("lfs", &open) {
                        later.push(Box::new(move |app| app.want_lfs_locks(tab)));
                    }
                }
                if is_open("lfs", &open) {
                    for pat in &lfs.patterns {
                        let resp = row(ui, 1, Icon::Package, pat, None, false, None);
                        let p2 = pat.clone();
                        ctxmenu::attach(&resp, later, || vec![item(format!("Stop tracking {p2}"), ctxmenu::act(tab, Action::LfsUntrack { pattern: p2.clone() }))]);
                    }
                    match &r.side.locks {
                        Some(Ok(locks)) => {
                            for l in locks {
                                let who = if l.ours { "You".to_owned() } else if l.owner.is_empty() { "Another user".into() } else { l.owner.clone() };
                                let resp = row(ui, 1, Icon::Lock, &l.path, Some(&who), false, Some(WARN)).tip(format!("Locked by {}", l.owner));
                                let (lp, ours) = (l.path.clone(), l.ours);
                                ctxmenu::attach(&resp, later, || {
                                    if ours {
                                        vec![item(format!("Unlock {lp}"), ctxmenu::act(tab, Action::LfsUnlock { path: lp.clone(), force: false }))]
                                    } else {
                                        let lp2 = lp.clone();
                                        vec![ctxmenu::danger("Force unlock", move |app| app.confirm_act(tab, "Force unlock this file?", &format!("Remove the Git LFS lock on {lp2}?"), "Force Unlock", Action::LfsUnlock { path: lp2.clone(), force: true }))]
                                    }
                                });
                            }
                        }
                        Some(Err(_)) => note(ui, "Could not verify Git LFS locks."),
                        None => {}
                    }
                    if lfs.patterns.is_empty() {
                        note(ui, "No patterns tracked yet. Right-click a file to track its type with LFS.");
                    }
                }
            }
            // STASHES
            if !v.data.stashes.is_empty() {
                let resp = section(ui, "STASHES", Icon::Archive, Some(stashes.len()), is_open("stashes", &open), None, &mut |_| {});
                if resp.0 {
                    toggled = Some("stashes".into());
                }
                if is_open("stashes", &open) {
                    for st in &stashes {
                        let resp = row(ui, 1, Icon::Archive, &st.message, None, false, None).tip(&st.message);
                        if resp.clicked() {
                            select_sha = Some(st.sha.clone());
                        }
                        if resp.double_clicked() {
                            let (i, s) = (st.index, st.sha.clone());
                            later.push(Box::new(move |app| app.act(tab, Action::StashApply { index: i, expected: Some(s), without_index: false })));
                        }
                        let st2 = (*st).clone();
                        ctxmenu::attach(&resp, later, || self.stash_menu(tab, &st2));
                    }
                }
            }
            // PATCHES
            let resp = section(ui, "PATCHES", Icon::CloudCog, None, is_open("patches", &open), None, &mut |_| {});
            if resp.0 {
                toggled = Some("patches".into());
            }
            if is_open("patches", &open) {
                ui.horizontal(|ui| {
                    ui.add_space(44.0);
                    ui.spacing_mut().item_spacing.x = 10.0;
                    if widgets::link(ui, "Save uncommitted changes as a patch…", 12.0).clicked() {
                        let name = super::patches::patch_name(v.data.head_branch.as_deref().unwrap_or("changes"));
                        later.push(Box::new(move |app| app.save_patch(tab, Vec::new(), &name)));
                    }
                });
                ui.horizontal(|ui| {
                    ui.add_space(44.0);
                    if widgets::link(ui, "Apply a patch file…", 12.0).clicked() {
                        later.push(Box::new(move |app| app.apply_patch(tab)));
                    }
                });
                ui.add_space(4.0);
            }
            // PULL REQUESTS and ISSUES
            self.hosted_sections(ui, tab, &*r, &mut open, &f, later);
            // TEAMS: files that would conflict with the target branch.
            let conflicts = r.side.conflicts.as_ref();
            let n = conflicts.map_or(0, |c| c.files.len());
            let teams_open = is_open("teams", &open) || (n > 0 && !open.contains_key("teams"));
            let resp = section(ui, "TEAMS", Icon::Users, (n > 0).then_some(n), teams_open, None, &mut |_| {});
            if resp.0 {
                open.insert("teams".into(), !teams_open);
            }
            if teams_open {
                match conflicts {
                    None => note(ui, "Conflict prevention is off for this repository."),
                    Some(cc) if cc.files.is_empty() => note(ui, &cc.note.clone().unwrap_or_else(|| format!("No conflicts with {}.", cc.target))),
                    Some(cc) => {
                        note(ui, &format!("{} file{} would conflict with {}:", cc.files.len(), if cc.files.len() == 1 { "" } else { "s" }, cc.target));
                        for f in &cc.files {
                            row(ui, 1, Icon::TriangleAlert, f, None, false, Some(WARN));
                        }
                    }
                }
            }
            // TAGS
            if !tags.is_empty() {
                let tag_full: Vec<GitRef> = tags.iter().map(|r| (*r).clone()).collect();
                let resp = section(ui, "TAGS", Icon::Tag, Some(tags.len()), is_open("tags", &open), None, &mut |_| {});
                if resp.0 {
                    toggled = Some("tags".into());
                }
                self.hide_all_menu(&resp.1, tab, v, &tag_full, "tags", later);
                if is_open("tags", &open) {
                    for t in &tags {
                        let resp = row(ui, 1, Icon::Tag, &t.name, None, false, None);
                        if resp.clicked() {
                            select_sha = Some(t.sha.clone());
                        }
                        let t2 = (*t).clone();
                        ctxmenu::attach(&resp, later, || self.branch_menu(tab, v, &t2));
                    }
                }
            }
            // SUBMODULES
            if !v.data.submodules.is_empty() {
                let resp = section(ui, "SUBMODULES", Icon::Package, Some(v.data.submodules.len()), is_open("submodules", &open), Some((Icon::Plus, "Add Submodule")), &mut |_| later.push(Box::new(move |app| app.add_submodule(tab))));
                if resp.0 {
                    toggled = Some("submodules".into());
                }
                if is_open("submodules", &open) {
                    let list: Vec<gitcraft_core::manage::Submodule> = r.side.submodules.clone().unwrap_or_else(|| v.data.submodules.iter().map(|p| gitcraft_core::manage::Submodule { path: p.clone(), sha: String::new(), state: "in-sync".into(), url: String::new() }).collect());
                    for s in list {
                        let (color, tip) = match s.state.as_str() {
                            "in-sync" => (GOOD, "This submodule is in-sync with your repository."),
                            "out-of-sync" => (WARN, "This submodule is out-of-sync with your repository."),
                            "uninitialized" => (Color32::from_gray(0x88), "Your submodule directory has not been initialized."),
                            _ => (super::theme::BAD, "This submodule has a conflict."),
                        };
                        let short: String = s.sha.chars().take(7).collect();
                        let resp = row(ui, 1, Icon::Package, &s.path, (!short.is_empty()).then_some(short.as_str()), false, Some(color)).tip(format!("{}\n{tip}", s.url));
                        let sub_path = join_sub(&v.path, &s.path);
                        let uninit = s.state == "uninitialized";
                        if resp.double_clicked() && !uninit {
                            let sp = sub_path.clone();
                            later.push(Box::new(move |app| app.open_repo(&sp)));
                        }
                        let p2 = s.path.clone();
                        ctxmenu::attach(&resp, later, || {
                            let sp = sub_path.clone();
                            let (p3, p4) = (p2.clone(), p2.clone());
                            vec![
                                ctxmenu::off(item(format!("Open submodule: {p2}"), move |app| app.open_repo(&sp)), !uninit),
                                item(if uninit { format!("Initialize {p2}") } else { format!("Update {p2}") }, ctxmenu::act(tab, Action::SubmoduleUpdate { paths: vec![p3] })),
                                Entry::Separator,
                                ctxmenu::danger("Delete this submodule", move |app| app.confirm_act(tab, "Remove this submodule?", &format!("Remove {p4}? Local changes must be committed, stashed, or backed up first."), "Remove", Action::SubmoduleRemove { path: p4.clone() })),
                            ]
                        });
                    }
                }
            }
            ui.add_space(16.0);
        });
        if let Some(k) = toggled {
            let now = open.get(&k).copied().unwrap_or(false);
            open.insert(k, !now);
        }
        ui.data_mut(|d| d.insert_temp(open_id, open));
        if let Some(sha) = select_sha {
            r.ui.select(Selection::Commit(sha));
        }
        let _ = SECTIONS;
    }

    #[allow(clippy::too_many_arguments)]
    fn branch_row(&self, ui: &mut Ui, tab: u64, v: &RepoView, r: &GitRef, indent: usize, label: &str, hidden: &[String], solo: &[String], head: Option<&str>, later: &mut Vec<Later>, select: &mut Option<String>) {
        let c = p();
        let is_head = r.kind == RefKind::Local && Some(r.name.as_str()) == head;
        let is_hidden = hidden.contains(&r.full);
        let width = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(vec2(width, 24.0), Sense::click());
        if is_head {
            ui.painter().rect_filled(rect, CornerRadius::ZERO, GOOD.gamma_multiply(0.2));
        } else if resp.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::ZERO, c.hoverrow);
        }
        let mut x = rect.left() + 12.0 + indent as f32 * 16.0;
        let icon = if is_head { Icon::Check } else if r.kind == RefKind::Local { Icon::Laptop } else { Icon::Cloud };
        icons::paint(ui, Rect::from_min_size(pos2(x, rect.center().y - 6.5), Vec2::splat(13.0)), icon, 13.0, if is_head { GOOD } else { c.fg.gamma_multiply(0.6) });
        x += 20.0;
        let mut badges: Vec<String> = Vec::new();
        if solo.contains(&r.full) {
            badges.push("solo".into());
        }
        if r.ahead > 0 {
            badges.push(format!("{}↑", r.ahead));
        }
        if r.behind > 0 {
            badges.push(format!("{}↓", r.behind));
        }
        let badges_w: f32 = badges.iter().map(|b| b.chars().count() as f32 * 6.5 + 10.0).sum();
        let eye_w = 22.0;
        let font = if is_head { super::fonts::semibold(13.0) } else { FontId::proportional(13.0) };
        let g = widgets::fit(ui, label, font, if is_head { c.white(1.0) } else { c.fg }, rect.right() - x - 12.0 - badges_w - eye_w);
        ui.painter().galley(pos2(x, rect.center().y - g.size().y / 2.0), g.clone(), c.fg);
        x += g.size().x + 6.0;
        for b in &badges {
            let w = b.chars().count() as f32 * 6.5 + 6.0;
            let br = Rect::from_min_size(pos2(x, rect.center().y - 7.5), vec2(w, 15.0));
            ui.painter().rect_filled(br, CornerRadius::same(3), c.white(0.1));
            ui.painter().text(br.center(), Align2::CENTER_CENTER, b, FontId::proportional(10.5), c.fg2);
            x += w + 4.0;
        }
        // Hide / show in the graph.
        let eye = Rect::from_min_size(pos2(rect.right() - 12.0 - 16.0, rect.center().y - 8.0), Vec2::splat(16.0));
        let eye_resp = ui.interact(eye, ui.scope_id().with(("eye", &r.full)), Sense::click());
        if is_hidden || resp.hovered() || eye_resp.hovered() {
            icons::paint(ui, eye, if is_hidden { Icon::EyeOff } else { Icon::Eye }, 13.0, if is_hidden { WARN } else if eye_resp.hovered() { c.white(1.0) } else { c.fg2 });
        }
        if eye_resp.tip(if is_hidden { "Show in the graph" } else { "Hide in the graph" }).clicked() {
            let full = r.full.clone();
            later.push(Box::new(move |app| app.toggle_ref_list(tab, "hidden", &full)));
        } else if resp.clicked() {
            *select = Some(r.sha.clone());
        }
        let resp = resp.tip(if is_head {
            "Checked out"
        } else if r.kind == RefKind::Remote {
            "Double-click to check out and fast-forward the local branch if it's behind"
        } else {
            "Double-click to checkout"
        });
        if resp.double_clicked() && !is_head {
            let target = r.clone();
            later.push(Box::new(move |app| app.checkout_ref(tab, &target)));
        }
        let r2 = r.clone();
        ctxmenu::attach(&resp, later, || self.branch_menu(tab, v, &r2));
    }

    fn hide_all_entries(&self, tab: u64, v: &RepoView, refs: &[GitRef], what: &str) -> Vec<Entry> {
        if refs.is_empty() {
            return Vec::new();
        }
        let hidden = self.hidden_refs(&v.path);
        let hideable: Vec<String> = refs.iter().filter(|r| !(r.kind == RefKind::Local && Some(&r.name) == v.data.head_branch.as_ref())).map(|r| r.full.clone()).collect();
        let fulls: Vec<String> = refs.iter().map(|r| r.full.clone()).collect();
        let all_hidden = hideable.iter().all(|f| hidden.contains(f));
        let any_hidden = fulls.iter().any(|f| hidden.contains(f));
        vec![
            ctxmenu::off(item(format!("Hide all {what}"), move |app| app.set_hidden_many(tab, hideable, true)), !all_hidden),
            ctxmenu::off(item(format!("Show all {what}"), move |app| app.set_hidden_many(tab, fulls, false)), any_hidden),
        ]
    }

    fn hide_all_menu(&self, resp: &egui::Response, tab: u64, v: &RepoView, refs: &[GitRef], what: &str, later: &mut Vec<Later>) {
        if refs.is_empty() {
            return;
        }
        ctxmenu::attach(resp, later, || self.hide_all_entries(tab, v, refs, what));
    }

    pub(crate) fn want_lfs_locks(&mut self, tab: u64) {
        let Some(r) = self.repo_mut(tab) else { return };
        if r.side.locks_loading {
            return;
        }
        r.side.locks_loading = true;
        let path = r.path.clone();
        self.spawn(move || super::Msg::Locks { tab, result: gitcraft_core::manage::lfs_locks(std::path::Path::new(&path)) });
    }

    /// The left panel shrunk to icons with counts.
    pub(crate) fn left_strip(&self, ui: &mut Ui, tab: Option<u64>, r: Option<&RepoTab>, later: &mut Vec<Later>) {
        let c = p();
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, CornerRadius::ZERO, c.panel);
        ui.painter().line_segment([rect.right_top(), rect.right_bottom()], Stroke::new(1.0, c.black(0.3)));
        let v = r.and_then(|r| r.view.as_ref());
        let locals = v.map_or(0, |v| v.data.refs.iter().filter(|r| r.kind == RefKind::Local).count());
        let items: [(Icon, &str, Option<usize>); 7] = [
            (Icon::Laptop, "Local", Some(locals)),
            (Icon::Cloud, "Remote", Some(v.map_or(0, |v| v.data.remotes.len()))),
            (Icon::Network, "Worktrees", Some(v.map_or(0, |v| v.data.worktrees.len()))),
            (Icon::CloudCog, "Cloud Patches", Some(0)),
            // The loaded pull requests, as the full panel counts them; none before they load.
            (Icon::GitPullRequest, "Pull Requests", tab.and_then(|t| self.hosted.get(&t)).filter(|h| h.prs.started && !h.prs.loading).map(|h| h.prs.items.len())),
            (Icon::CircleDot, "Issues", None),
            (Icon::Users, "Teams", None),
        ];
        let mut y = rect.top() + 9.0;
        let b = Rect::from_center_size(pos2(rect.center().x, y + 8.5), Vec2::splat(17.0));
        let resp = ui.interact(b, ui.scope_id().with("strip_expand"), Sense::click());
        ui.painter().circle_filled(b.center(), 8.5, if resp.hovered() { c.white(1.0) } else { c.white(0.85) });
        icons::paint(ui, b, Icon::ChevronsRight, 12.0, c.panel);
        let mut expand = resp.tip("Expand Panel (Ctrl+J)").clicked();
        y += 17.0 + 8.0 + 3.0;
        for (icon, label, n) in items {
            let h = if n.is_some() { 32.0 } else { 23.0 };
            let ir = Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), h));
            let resp = ui.interact(ir, ui.scope_id().with(("strip", label)), Sense::click());
            let ink = if resp.hovered() { c.white(1.0) } else { c.white(0.7) };
            icons::paint(ui, Rect::from_center_size(pos2(ir.center().x, y + 4.0 + 7.5), Vec2::splat(15.0)), icon, 15.0, ink);
            if let Some(n) = n {
                ui.painter().text(pos2(ir.center().x, y + 4.0 + 15.0 + 6.0), Align2::CENTER_CENTER, n.to_string(), FontId::proportional(9.5), c.fg2);
            }
            if resp.tip(label).clicked() {
                expand = true;
            }
            y += h + 3.0;
        }
        if expand {
            later.push(Box::new(|app| {
                if app.settings.flag("showLeft", true) {
                    app.left_peek = true;
                } else {
                    app.settings.set("showLeft", true);
                }
            }));
        }
    }
}

/// A submodule's folder inside the repository, with the system's separator.
fn join_sub(repo: &str, sub: &str) -> String {
    if cfg!(windows) { format!("{repo}\\{}", sub.replace('/', "\\")) } else { format!("{repo}/{sub}") }
}

/// Version-aware ordering (v1.10 after v1.9).
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(p), Some(q)) if p.is_ascii_digit() && q.is_ascii_digit() => {
                let mut n1 = String::new();
                while let Some(d) = x.peek().copied().filter(char::is_ascii_digit) {
                    n1.push(d);
                    x.next();
                }
                let mut n2 = String::new();
                while let Some(d) = y.peek().copied().filter(char::is_ascii_digit) {
                    n2.push(d);
                    y.next();
                }
                let (t1, t2) = (n1.trim_start_matches('0'), n2.trim_start_matches('0'));
                let o = t1.len().cmp(&t2.len()).then_with(|| t1.cmp(t2));
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
            }
            (Some(p), Some(q)) => {
                let o = p.to_lowercase().cmp(q.to_lowercase());
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}

/// A section header (30 px): chevron, icon, title, an action shown under the pointer, a count.
/// Returns whether the title was clicked, and the header's response (for its menu).
fn section(ui: &mut Ui, title: &str, icon: Icon, count: Option<usize>, open: bool, action: Option<(Icon, &str)>, on_action: &mut dyn FnMut(&mut Ui)) -> (bool, egui::Response) {
    let c = p();
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 30.0), Sense::click());
    let hovered = ui.rect_contains_pointer(rect);
    if hovered {
        ui.painter().rect_filled(rect, CornerRadius::ZERO, c.white(0.05));
    }
    let mut x = rect.left() + 8.0;
    widgets::chevron(ui, Rect::from_min_size(pos2(x, rect.center().y - 7.0), Vec2::splat(14.0)), open, 14.0, c.white(0.7));
    x += 21.0;
    icons::paint(ui, Rect::from_min_size(pos2(x, rect.center().y - 7.0), Vec2::splat(14.0)), icon, 14.0, c.white(0.75));
    x += 20.0;
    ui.painter().text(pos2(x, rect.center().y), Align2::LEFT_CENTER, title, super::fonts::semibold(12.5), c.fg);
    let mut right = rect.right() - 14.0;
    if let Some(n) = count {
        let g = ui.painter().layout_no_wrap(n.to_string(), FontId::proportional(12.5), c.fg);
        right -= g.size().x;
        ui.painter().galley(pos2(right, rect.center().y - g.size().y / 2.0), g, c.fg);
        right -= 8.0;
    }
    let mut action_clicked = false;
    if let Some((icon, tip)) = action
        && hovered
    {
        let ar = Rect::from_min_size(pos2(right - 18.0, rect.center().y - 9.0), Vec2::splat(18.0));
        let aresp = ui.interact(ar, ui.scope_id().with(("section_action", title)), Sense::click());
        if aresp.hovered() {
            ui.painter().rect_filled(ar, CornerRadius::same(3), c.white(0.1));
        }
        icons::paint(ui, ar, icon, 13.0, if aresp.hovered() { c.white(1.0) } else { c.white(0.7) });
        if aresp.tip(tip).clicked() {
            action_clicked = true;
            on_action(ui);
        }
    }
    (resp.clicked() && !action_clicked, resp)
}

/// A 24 px row in a section.
fn row(ui: &mut Ui, indent: usize, icon: Icon, label: &str, note: Option<&str>, bold: bool, icon_color: Option<Color32>) -> egui::Response {
    let c = p();
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 24.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::ZERO, c.hoverrow);
    }
    let mut x = rect.left() + 12.0 + indent as f32 * 16.0;
    icons::paint(ui, Rect::from_min_size(pos2(x, rect.center().y - 6.5), Vec2::splat(13.0)), icon, 13.0, icon_color.unwrap_or(c.fg.gamma_multiply(0.6)));
    x += 20.0;
    let note_w = note.map_or(0.0, |_| (rect.right() - x) * 0.4);
    let font = if bold { super::fonts::semibold(13.0) } else { FontId::proportional(13.0) };
    let g = widgets::fit(ui, label, font, if bold { c.white(1.0) } else { c.fg }, rect.right() - x - 12.0 - note_w);
    ui.painter().galley(pos2(x, rect.center().y - g.size().y / 2.0), g.clone(), c.fg);
    if let Some(n) = note {
        let ng = widgets::fit(ui, n, FontId::proportional(11.0), c.fg3, note_w);
        ui.painter().galley(pos2(x + g.size().x + 7.0, rect.center().y - ng.size().y / 2.0), ng, c.fg3);
    }
    resp
}

fn note(ui: &mut Ui, text: &str) {
    let c = p();
    ui.horizontal(|ui| {
        ui.add_space(44.0);
        ui.add(egui::Label::new(egui::RichText::new(text).size(12.0).color(c.fg3)).wrap());
    });
    ui.add_space(4.0);
}

pub(crate) fn section_row(ui: &mut Ui, indent: usize, icon: Icon, label: &str, note_text: Option<&str>, icon_color: Option<Color32>) -> egui::Response {
    row(ui, indent, icon, label, note_text, false, icon_color)
}

pub(crate) fn section_head(ui: &mut Ui, title: &str, icon: Icon, count: Option<usize>, open: bool, action: Option<(Icon, &str)>, on_action: &mut dyn FnMut(&mut Ui)) -> (bool, egui::Response) {
    section(ui, title, icon, count, open, action, on_action)
}

pub(crate) fn section_note(ui: &mut Ui, text: &str) {
    note(ui, text);
}

#[cfg(test)]
mod tests {
    #[test]
    fn tags_sort_by_version() {
        let mut v = vec!["v1.9", "v1.10", "v1.2"];
        v.sort_by(|a, b| super::natural_cmp(b, a));
        assert_eq!(v, ["v1.10", "v1.9", "v1.2"]);
    }
}
