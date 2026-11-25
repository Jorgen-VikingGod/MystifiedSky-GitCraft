//! Repository-wide dialogs: clean up local branches, sparse checkout, Repository Management,
//! the activity log and the feature plan.

use super::widgets::Tip;
use super::App;
use super::dialogs::{Body, Dialog, Field, FieldKind};
use super::icons::{self, Icon};
use super::theme::{GOOD, WARN, p};
use super::widgets::{self, Kind, REMOVED};
use egui::{CornerRadius, FontId, Rect, Sense, Ui, Vec2, pos2, vec2};
use gitcraft_core::actions::{Action, RemoteBranch};
use gitcraft_core::manage::CleanupBranch;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// The latest branch review's number and, once it is done, its branches.
type Review = Arc<Mutex<(u64, Option<Vec<CleanupBranch>>)>>;
/// A repository's pull mode and its autostash preference for a rebase pull.
type PullMode = (String, Option<bool>);

impl App {
    /// Clean Up Local Branches: filter by age, merge and upstream state, then delete the picked ones.
    pub(crate) fn cleanup_dialog(&mut self, tab: u64) {
        let Some(path) = self.repo(tab).map(|r| r.path.clone()) else { return };
        // The branches of the latest review, with the number of that review: a slower, earlier
        // review never replaces a newer one's list.
        let list: Review = Default::default();
        let load = |list: &Review, app: &App, path: &str, patches: bool| {
            let request = {
                let mut l = list.lock().unwrap_or_else(|e| e.into_inner());
                l.0 += 1;
                l.1 = None;
                l.0
            };
            let (l, path) = (list.clone(), path.to_owned());
            let ctx = app.ctx.clone();
            std::thread::spawn(move || {
                let branches = gitcraft_core::manage::review_cleanup(Path::new(&path), patches);
                let mut l = l.lock().unwrap_or_else(|e| e.into_inner());
                if l.0 == request {
                    l.1 = Some(branches);
                }
                drop(l);
                ctx.request_repaint();
            });
        };
        load(&list, self, &path, false);
        let mut days = String::from("0");
        let mut merge = "merged";
        let mut up = "any";
        let mut patches = false;
        let mut picked: HashSet<String> = HashSet::new();
        let mut remotes: HashSet<String> = HashSet::new();
        let path2 = path.clone();
        let draw = move |app: &mut App, ui: &mut Ui| -> bool {
            let c = p();
            let mut close = false;
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
                ui.label(egui::RichText::new("Older than (days):").size(12.5).color(c.fg2));
                widgets::field(ui, &mut days, "", 56.0, 28.0, 12.5);
                days.retain(|ch| ch.is_ascii_digit());
                ui.label(egui::RichText::new("Merge status:").size(12.5).color(c.fg2));
                egui::ComboBox::from_id_salt("cleanup_merge").selected_text(match merge {
                    "any" => "Any",
                    "merged" => "Merged",
                    _ => "Not merged",
                }).show_ui(ui, |ui| {
                    for (v, l) in [("any", "Any"), ("merged", "Merged"), ("unmerged", "Not merged")] {
                        ui.selectable_value(&mut merge, v, l);
                    }
                });
                ui.label(egui::RichText::new("Remote status:").size(12.5).color(c.fg2));
                egui::ComboBox::from_id_salt("cleanup_up").selected_text(match up {
                    "any" => "Any",
                    "has" => "Has upstream",
                    "none" => "Doesn't have upstream",
                    _ => "Upstream gone",
                }).show_ui(ui, |ui| {
                    for (v, l) in [("any", "Any"), ("has", "Has upstream"), ("none", "Doesn't have upstream"), ("gone", "Upstream gone")] {
                        ui.selectable_value(&mut up, v, l);
                    }
                });
            });
            ui.add_space(8.0);
            if widgets::checkbox(ui, &mut patches, "Include squash- and rebase-merged changes", 12.0).tip("Compare patches against the default branch. Equivalent patches may have different commits.").clicked() {
                picked.clear();
                remotes.clear();
                load(&list, app, &path2, patches);
            }
            ui.add_space(8.0);
            let now = chrono::Local::now().timestamp();
            // An age that isn't a number of days, or that overflows, deletes nothing until it is fixed.
            let min_age = days.trim().parse::<i64>().ok().and_then(|d| d.checked_mul(86400)).filter(|d| *d >= 0);
            if min_age.is_none() {
                ui.label(egui::RichText::new("Enter the age in whole days.").size(12.0).color(REMOVED));
            }
            let age_valid = min_age.is_some();
            let min_age = min_age.unwrap_or(i64::MAX);
            let all = list.lock().unwrap_or_else(|e| e.into_inner()).1.clone();
            let shown: Vec<CleanupBranch> = all.clone().unwrap_or_default().into_iter().filter(|b| {
                now - b.time >= min_age
                    && (merge == "any" || (merge == "merged") == (b.merged || b.patch_equivalent))
                    && match up {
                        "gone" => b.upstream_gone,
                        "has" => !b.upstream.is_empty() && !b.upstream_gone,
                        "none" => b.upstream.is_empty() && !b.upstream_gone,
                        _ => true,
                    }
            }).collect();
            ui.label(egui::RichText::new(if shown.len() == 1 { "1 branch matching filters".to_owned() } else { format!("{} branches matching filters", shown.len()) }).size(12.0).color(c.fg3));
            ui.add_space(6.0);
            egui::Frame::new().stroke(egui::Stroke::new(1.0, c.white(0.05))).corner_radius(CornerRadius::same(4)).show(ui, |ui| {
                egui::ScrollArea::vertical().max_height(300.0).auto_shrink([false, true]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    if all.is_none() {
                        ui.label(egui::RichText::new("Loading…").size(12.0).color(c.fg3));
                    } else if shown.is_empty() {
                        ui.label(egui::RichText::new("No branches match the current filters").size(12.5).color(c.fg3));
                    }
                    let fmt = app.settings.string("dateFormat", "MM/DD/YYYY");
                    for b in &shown {
                        ui.horizontal(|ui| {
                            ui.set_height(30.0);
                            ui.add_space(8.0);
                            let mut on = picked.contains(&b.name);
                            if widgets::checkbox(ui, &mut on, "", 12.0).clicked() {
                                if on { picked.insert(b.name.clone()) } else { picked.remove(&b.name) };
                            }
                            let w = ui.available_width() - 340.0;
                            let g = widgets::fit(ui, &b.name, FontId::proportional(12.5), c.white(0.9), w.max(60.0));
                            let (r, _) = ui.allocate_exact_size(vec2(w.max(60.0), 20.0), Sense::hover());
                            ui.painter().galley(pos2(r.left(), r.center().y - g.size().y / 2.0), g, c.white(0.9));
                            let state = if b.merged { ("merged", GOOD) } else if b.patch_equivalent { ("equivalent patch", c.fg3) } else { ("not merged", c.fg3) };
                            ui.label(egui::RichText::new(state.0).size(11.0).color(state.1));
                            ui.add_sized([80.0, 20.0], egui::Label::new(egui::RichText::new(widgets::format_date(b.time, &fmt)).size(11.0).color(c.fg3)));
                            let upstream = if b.upstream_gone { format!("{} (gone)", if b.upstream.is_empty() { "Upstream" } else { &b.upstream }) } else if b.upstream.is_empty() { "None".into() } else { b.upstream.clone() };
                            ui.add_sized([110.0, 20.0], egui::Label::new(egui::RichText::new(upstream).size(11.0).color(c.fg2)).truncate());
                            if !b.upstream.is_empty() && !b.upstream_gone {
                                let mut r = remotes.contains(&b.name);
                                if widgets::checkbox(ui, &mut r, "remote", 11.0).tip(format!("Also delete remote branch {}", b.upstream)).clicked() {
                                    if r { remotes.insert(b.name.clone()) } else { remotes.remove(&b.name) };
                                }
                            }
                        });
                    }
                });
            });
            ui.add_space(8.0);
            if widgets::link(ui, "Select all branches", 12.0).clicked() {
                picked = shown.iter().map(|b| b.name.clone()).collect();
            }
            ui.add_space(12.0);
            let remote_count = remotes.iter().filter(|r| picked.contains(*r) && all.as_ref().is_some_and(|l| l.iter().any(|b| &b.name == *r && !b.upstream.is_empty() && !b.upstream_gone))).count();
            ui.horizontal(|ui| {
                if remote_count > 0 {
                    ui.label(egui::RichText::new(format!("Warning: {remote_count} remote branch{} will be deleted. This cannot be undone.", if remote_count == 1 { "" } else { "es" })).size(12.0).color(REMOVED));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let n = picked.len();
                    if widgets::button(ui, Kind::Danger, None, &format!("Delete {n} branch{}", if n == 1 { "" } else { "es" }), 28.0, n > 0 && age_valid).clicked() {
                        let names: Vec<String> = picked.iter().cloned().collect();
                        // Each remote branch as reviewed here, so a changed upstream isn't deleted.
                        let ups: Vec<RemoteBranch> = all.iter().flatten().filter(|b| picked.contains(&b.name) && remotes.contains(&b.name) && !b.upstream.is_empty() && !b.upstream_gone).map(|b| RemoteBranch { branch: b.name.clone(), remote: b.remote.clone(), reference: b.remote_ref.clone() }).collect();
                        app.act(tab, Action::DeleteBranches { names, remotes: ups });
                        close = true;
                    }
                    if widgets::button(ui, Kind::Default, None, "Cancel", 28.0, true).clicked() {
                        close = true;
                    }
                });
            });
            close
        };
        let d = Dialog::new("Clean Up Local Branches", Body::Custom(Box::new(draw))).width(640.0).no_cancel();
        self.dialog = Some(d);
    }

    /// Sparse Checkout: the patterns of files to keep in the working folder.
    pub(crate) fn sparse_dialog(&mut self, tab: u64) {
        let Some(path) = self.repo(tab).map(|r| r.path.clone()) else { return };
        let info = gitcraft_core::manage::sparse_info(Path::new(&path));
        let value = if info.patterns.is_empty() { "/src/\n/docs/".to_owned() } else { info.patterns.join("\n") };
        let fields = vec![Field::new("patterns", "", FieldKind::Multiline(7))];
        let mut d = Dialog::new("Sparse Checkout", Body::Form { intro: String::new(), fields, grid: false })
            .width(480.0)
            .text_value("patterns", &value)
            .choice(if info.enabled { "Update" } else { "Turn On" }, Kind::Primary, move |app, v| {
                let patterns = v.text("patterns").lines().map(|l| l.trim().to_owned()).filter(|l| !l.is_empty()).collect();
                app.act(tab, Action::SparseSet { patterns });
            });
        if let Body::Form { intro, .. } = &mut d.body {
            *intro = String::new();
        }
        if info.enabled {
            d.choices.insert(0, super::dialogs::Choice { label: "Turn Off".into(), kind: Kind::Danger, then: Box::new(move |app, _| app.act(tab, Action::SparseDisable)), again: None });
        }
        // The explanation above the box.
        let fields_note = "Only files matching these patterns stay in the working folder (one per line, .gitignore style). Everything else is still in the repository.";
        if let Body::Form { fields, .. } = &mut d.body {
            fields[0].label = fields_note.into();
        }
        self.dialog = Some(d);
    }

    /// Repository Management: open, favourite and recent repositories with their branch and
    /// changes, and fetch, pull, open or remove one or all of them.
    pub(crate) fn repo_management_dialog(&mut self) {
        let summaries: Arc<Mutex<HashMap<String, gitcraft_core::extras::RepoSummary>>> = Default::default();
        let asked: Arc<Mutex<HashSet<String>>> = Default::default();
        let mut query = String::new();
        let mut collapsed: HashSet<&'static str> = HashSet::new();
        let draw = move |app: &mut App, ui: &mut Ui| -> bool {
            let c = p();
            let open: Vec<String> = {
                let mut v: Vec<String> = Vec::new();
                for t in app.tabs.iter().filter_map(|t| t.path()) {
                    if !v.iter().any(|x| super::same_folder(x, t)) {
                        v.push(t.to_owned());
                    }
                }
                v
            };
            let favorites: Vec<String> = app.settings.get("favorites").unwrap_or_default();
            let recents = app.settings.recents();
            let groups: [(&'static str, Vec<String>); 3] = [("Open", open), ("Favorites", favorites.clone()), ("Recent", recents.clone())];
            let mut all: Vec<String> = Vec::new();
            for (_, ps) in &groups {
                for p in ps {
                    if !all.contains(p) {
                        all.push(p.clone());
                    }
                }
            }
            let mut run: Option<(Vec<String>, bool)> = None;
            ui.horizontal(|ui| {
                let w = ui.available_width() - 200.0;
                widgets::field(ui, &mut query, "Search repositories", w, 26.0, 12.5);
                if widgets::button(ui, Kind::Default, Some(Icon::CloudDownload), "Fetch all", 26.0, true).tip("Fetch all repositories").clicked() {
                    run = Some((all.clone(), false));
                }
                if widgets::button(ui, Kind::Default, Some(Icon::Download), "Pull all", 26.0, true).tip("Pull all repositories").clicked() {
                    run = Some((all.clone(), true));
                }
            });
            ui.add_space(12.0);
            let q = query.to_lowercase();
            let mut close = false;
            let mut toggle_fav: Option<String> = None;
            let mut remove: Vec<String> = Vec::new();
            let mut open_paths: Vec<String> = Vec::new();
            egui::ScrollArea::vertical().max_height(ui.ctx().content_rect().height() * 0.58).auto_shrink([false, true]).show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                for (title, paths) in &groups {
                    let list: Vec<&String> = paths.iter().filter(|p| q.is_empty() || p.to_lowercase().contains(&q)).collect();
                    let is_collapsed = collapsed.contains(title);
                    ui.horizontal(|ui| {
                        ui.set_height(28.0);
                        let (r, resp) = ui.allocate_exact_size(vec2(14.0, 28.0), Sense::click());
                        widgets::chevron(ui, r, !is_collapsed, 14.0, c.fg2);
                        let head = ui.add(egui::Label::new(egui::RichText::new(format!("{} ({})", title.to_uppercase(), list.len())).font(super::fonts::semibold(12.0)).color(c.fg2)).sense(Sense::click()));
                        if resp.clicked() || head.clicked() {
                            if is_collapsed { collapsed.remove(title) } else { collapsed.insert(title) };
                        }
                        if list.len() > 1 {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.spacing_mut().item_spacing.x = 12.0;
                                if *title == "Recent" && widgets::link(ui, "Remove all", 12.0).clicked() {
                                    remove.extend(list.iter().map(|s| s.to_string()));
                                }
                                if widgets::link(ui, &format!("Fetch {}", list.len()), 12.0).clicked() {
                                    run = Some((list.iter().map(|s| s.to_string()).collect(), false));
                                }
                                if *title != "Open" && widgets::link(ui, &format!("Open {} repositories", list.len()), 12.0).clicked() {
                                    open_paths.extend(list.iter().map(|s| s.to_string()));
                                }
                            });
                        }
                    });
                    if is_collapsed {
                        continue;
                    }
                    if list.is_empty() {
                        ui.horizontal(|ui| {
                            ui.add_space(24.0);
                            ui.label(egui::RichText::new(if q.is_empty() { "None yet" } else { "No results found" }).size(12.0).color(c.fg3));
                        });
                    }
                    for path in list {
                        // Read the summary once it shows.
                        if asked.lock().unwrap_or_else(|e| e.into_inner()).insert(path.clone()) {
                            let (s, p2, ctx) = (summaries.clone(), path.clone(), app.ctx.clone());
                            std::thread::spawn(move || {
                                let sum = gitcraft_core::extras::repo_summary(Path::new(&p2));
                                s.lock().unwrap_or_else(|e| e.into_inner()).insert(p2, sum);
                                ctx.request_repaint();
                            });
                        }
                        let sum = summaries.lock().unwrap_or_else(|e| e.into_inner()).get(path).cloned();
                        let w = ui.available_width();
                        let (row, resp) = ui.allocate_exact_size(vec2(w, 34.0), Sense::hover());
                        let hovered = resp.hovered() || ui.rect_contains_pointer(row);
                        if hovered {
                            ui.painter().rect_filled(row, CornerRadius::same(4), c.hoverrow);
                        }
                        let fav = favorites.iter().any(|f| super::same_folder(f, path));
                        let star = Rect::from_min_size(pos2(row.left() + 20.0, row.center().y - 7.0), Vec2::splat(14.0));
                        let sresp = ui.interact(star, ui.scope_id().with(("fav", title, path)), Sense::click()).tip(if fav { "Remove repository from favorites" } else { "Add repository to favorites" });
                        icons::paint(ui, star, Icon::Star, 14.0, if fav { egui::Color32::from_rgb(0xe5, 0xc0, 0x7b) } else if sresp.hovered() { c.white(1.0) } else { c.fg3 });
                        if sresp.clicked() {
                            toggle_fav = Some(path.clone());
                        }
                        let name = super::folder_name(path);
                        let ng = ui.painter().layout_no_wrap(name, super::fonts::semibold(12.5), c.link);
                        let nr = Rect::from_min_size(pos2(star.right() + 8.0, row.center().y - ng.size().y / 2.0), ng.size());
                        ui.painter().galley(nr.min, ng, c.link);
                        if ui.interact(nr, ui.scope_id().with(("open", title, path)), Sense::click()).tip("Open").clicked() {
                            open_paths.push(path.clone());
                            close = true;
                        }
                        let right_w = 250.0;
                        let pg = widgets::fit(ui, path, FontId::proportional(12.5), c.fg3, row.right() - nr.right() - right_w - 16.0);
                        ui.painter().galley(pos2(nr.right() + 8.0, row.center().y - pg.size().y / 2.0), pg, c.fg3);
                        let mut x = row.right() - 8.0;
                        if hovered {
                            let mut buttons: Vec<(Icon, &str, u8)> = vec![(Icon::FolderOpen, "Open in File Manager", 2), (Icon::Download, "Pull repository", 1), (Icon::CloudDownload, "Fetch repository", 0)];
                            if *title == "Recent" {
                                buttons.insert(0, (Icon::Trash2, "Remove repository", 3));
                            }
                            for (icon, tip, what) in buttons {
                                let br = Rect::from_min_size(pos2(x - 22.0, row.center().y - 11.0), Vec2::splat(22.0));
                                let b = ui.interact(br, ui.scope_id().with(("btn", title, path, what)), Sense::click()).tip(tip);
                                icons::paint(ui, br, icon, 13.0, if b.hovered() { c.white(1.0) } else { c.fg2 });
                                if b.clicked() {
                                    match what {
                                        0 => run = Some((vec![path.clone()], false)),
                                        1 => run = Some((vec![path.clone()], true)),
                                        2 => {
                                            let _ = gitcraft_core::shell::open_path(Path::new(path));
                                        }
                                        _ => remove.push(path.clone()),
                                    }
                                }
                                x -= 26.0;
                            }
                        }
                        match sum {
                            Some(s) if !s.exists => {
                                ui.painter().text(pos2(x, row.center().y), egui::Align2::RIGHT_CENTER, "Repo is unreachable and may have been deleted", FontId::proportional(11.5), REMOVED);
                            }
                            Some(s) => {
                                let changes = if s.changes > 0 { format!("{} change{}", s.changes, if s.changes == 1 { "" } else { "s" }) } else { "clean".into() };
                                let r = ui.painter().text(pos2(x, row.center().y), egui::Align2::RIGHT_CENTER, changes, FontId::proportional(11.5), if s.changes > 0 { WARN } else { c.fg3 });
                                x = r.left() - 8.0;
                                if s.ahead > 0 || s.behind > 0 {
                                    let r = ui.painter().text(pos2(x, row.center().y), egui::Align2::RIGHT_CENTER, format!("↑{} ↓{}", s.ahead, s.behind), FontId::proportional(11.5), c.fg2);
                                    x = r.left() - 8.0;
                                }
                                if let Some(b) = &s.branch {
                                    let g = ui.painter().layout_no_wrap(b.clone(), FontId::proportional(11.5), c.white(0.85));
                                    let br = Rect::from_min_size(pos2(x - g.size().x - 12.0, row.center().y - 9.0), vec2(g.size().x + 12.0, 18.0));
                                    ui.painter().rect_filled(br, CornerRadius::same(3), c.white(0.08));
                                    ui.painter().galley(pos2(br.left() + 6.0, br.center().y - g.size().y / 2.0), g, c.white(0.85));
                                }
                            }
                            None => {}
                        }
                    }
                    ui.add_space(8.0);
                }
            });
            if let Some(p) = toggle_fav {
                let mut fav: Vec<String> = app.settings.get("favorites").unwrap_or_default();
                if let Some(i) = fav.iter().position(|f| super::same_folder(f, &p)) {
                    fav.remove(i);
                } else {
                    fav.push(p);
                }
                app.settings.set("favorites", fav);
            }
            if !remove.is_empty() {
                let mut rec = app.settings.recents();
                rec.retain(|r| !remove.iter().any(|x| super::same_folder(x, r)));
                app.settings.set("recents", rec);
            }
            for p in open_paths {
                app.open_repo_checked(&p);
            }
            if let Some((paths, pull)) = run {
                app.run_all(paths, pull, asked.clone());
            }
            close
        };
        self.dialog = Some(Dialog::new("Repository Management", Body::Custom(Box::new(draw))).width(760.0).no_cancel());
    }

    /// Fetch or pull several repositories, three at a time, with progress in the status bar.
    fn run_all(&mut self, paths: Vec<String>, pull: bool, refresh: Arc<Mutex<HashSet<String>>>) {
        if paths.is_empty() {
            return self.toast(false, if pull { "No repositories to pull" } else { "No repositories to fetch" });
        }
        // One bulk run at a time, so one can't clear another's progress.
        if self.bulk.is_some() {
            return self.toast(false, "Wait for the repositories being pulled or fetched to finish");
        }
        if paths.iter().any(|path| self.repo_pull_busy(path)) {
            return self.toast(false, "Wait for the current repository operation to finish before fetching or pulling this selection");
        }
        self.bulk_batch += 1;
        let batch = self.bulk_batch;
        self.bulk = Some((batch, 0));
        self.busy_repos.extend(paths.iter().cloned());
        // Each repository's pull mode, and its autostash preference for a rebase pull.
        let modes: Vec<PullMode> = paths.iter().map(|p| {
            let m = self.pull_mode(p);
            let m = if m == "fetch" { "ff".to_owned() } else { m };
            let autostash = if m == "rebase" { self.repo_pref(p, "pullAutostash").and_then(|v| v.as_bool()) } else { None };
            (m, autostash)
        }).collect();
        let engine = self.engine.clone();
        let total = paths.len();
        let verb = if pull { "Pulling" } else { "Fetching" };
        self.bulk_message = Some(format!("{verb} repositories (0 of {total} complete)…"));
        self.update_busy();
        let inbox = self.inbox.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let queue: Arc<Mutex<Vec<(String, PullMode)>>> = Arc::new(Mutex::new(paths.into_iter().zip(modes).collect()));
            let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let failed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let workers: Vec<_> = (0..3.min(total)).map(|_| {
                let (q, done, failed, engine, inbox, ctx) = (queue.clone(), done.clone(), failed.clone(), engine.clone(), inbox.clone(), ctx.clone());
                std::thread::spawn(move || loop {
                    let Some((path, (mode, autostash))) = q.lock().unwrap_or_else(|e| e.into_inner()).pop() else { break };
                    let action = if pull { Action::Pull { mode, autostash } } else { Action::Fetch };
                    if !engine.perform(&path, action).ok {
                        failed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    // Counts arrive from several workers: only a higher count of this batch shows.
                    inbox.post(super::Msg::Apply(Box::new(move |app: &mut App| {
                        app.finish_busy_repo(&path);
                        if let Some((b, shown)) = app.bulk.as_mut()
                            && *b == batch
                            && n > *shown
                        {
                            *shown = n;
                            app.bulk_message = Some(format!("{verb} repositories ({n} of {total} complete)…"));
                            app.update_busy();
                        }
                    })));
                    ctx.request_repaint();
                })
            }).collect();
            for w in workers {
                let _ = w.join();
            }
            let failed = failed.load(std::sync::atomic::Ordering::Relaxed);
            inbox.post(super::Msg::Apply(Box::new(move |app: &mut App| {
                if app.bulk.is_some_and(|(b, _)| b == batch) {
                    app.bulk = None;
                    app.bulk_message = None;
                    app.update_busy();
                }
                refresh.lock().unwrap_or_else(|e| e.into_inner()).clear();
                let what = if pull { "Pulled" } else { "Fetched" };
                app.toast(failed == 0, format!("{what} {} of {total} repositories{}", total - failed, if failed > 0 { format!("; {failed} failed") } else { String::new() }));
                app.reload_all();
            })));
            ctx.request_repaint();
        });
    }

    /// The activity log: every Git command GitCraft ran, newest first.
    pub(crate) fn log_dialog(&mut self) {
        let rows = self.engine.log_entries().into_iter().rev().take(500).map(|e| (e.time.clone(), format!("{}{}\n{}", if e.ok { "" } else { "✗ " }, e.command, e.output))).collect();
        self.dialog = Some(Dialog::new("Activity Log", Body::List(rows)).width(720.0).cancel_label("Close"));
    }

    pub(crate) fn plan_dialog(&mut self) {
        let stages: [(&str, &str, &str); 6] = [
            ("Stage 1", "Main window and everyday Git — done", "Tabs, toolbar, panels, commit graph · Stage, commit, amend, discard · Branches, tags, stashes · Fetch, pull, push · Undo / redo, search, activity log"),
            ("Stage 2", "Deeper graph and diffs — done", "Hunk / inline / split diffs, line staging, edit and save · File history and blame · More graph columns, solo / hide branches · Compare commits, load more history, image diffs"),
            ("Stage 3", "Rewriting history and conflicts", "Interactive rebase · Merge conflict tool · Reword, squash, drop"),
            ("Stage 4", "Repository management and accounts", "Remotes, worktrees, submodules · Gitflow, LFS · Credential Manager, SSH, GPG · Job queue with progress"),
            ("Stage 5", "Productivity", "Command palette · Built-in terminal · Workspaces, profiles, preferences · Themes"),
            ("Stage 6", "Online services, AI and shipping", "Pull requests, issues, Launchpad · Cloud patches, teams · AI helpers, Agents view · Installers and updates"),
        ];
        let rows = stages.iter().map(|(n, t, items)| (format!("{n} · {t}"), items.to_string())).collect();
        self.dialog = Some(Dialog::new("GitCraft feature plan", Body::List(rows)).width(560.0).cancel_label("Close"));
    }
}
