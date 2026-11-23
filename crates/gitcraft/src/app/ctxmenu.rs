//! Right-click menus for commits, branches, tags, stashes and files, with GitCraft's
//! items in their order. Menus are built as data while a frame is drawn; the chosen item runs
//! afterwards with the whole app.

use super::App;
use super::rebase::{base_for, head_line};
use super::theme::p;
use super::widgets::REMOVED;
use gitcraft_core::actions::Action;
use gitcraft_core::git::{Commit, GitRef, RefKind, Stash};
use gitcraft_core::session::RepoView;

/// Work to do with the app once the frame is drawn.
pub type Later = Box<dyn FnOnce(&mut App)>;

pub enum Entry {
    Item { label: String, enabled: bool, danger: bool, run: Later },
    Sub { label: String, enabled: bool, children: Vec<Entry> },
    Separator,
    Heading(String),
}

pub fn item(label: impl Into<String>, run: impl FnOnce(&mut App) + 'static) -> Entry {
    Entry::Item { label: label.into(), enabled: true, danger: false, run: Box::new(run) }
}

pub fn danger(label: impl Into<String>, run: impl FnOnce(&mut App) + 'static) -> Entry {
    Entry::Item { label: label.into(), enabled: true, danger: true, run: Box::new(run) }
}

pub fn off(entry: Entry, enabled: bool) -> Entry {
    match entry {
        Entry::Item { label, danger, run, .. } => Entry::Item { label, enabled, danger, run },
        Entry::Sub { label, children, .. } => Entry::Sub { label, enabled, children },
        other => other,
    }
}

pub fn sub(label: impl Into<String>, children: Vec<Entry>) -> Entry {
    Entry::Sub { label: label.into(), enabled: true, children }
}

pub fn act(tab: u64, action: Action) -> impl FnOnce(&mut App) {
    move |app| app.act(tab, action)
}

/// Draw `entries` in an open menu; the chosen one goes to `later`.
pub fn show(ui: &mut egui::Ui, entries: Vec<Entry>, later: &mut Vec<Later>) {
    let c = p();
    ui.set_min_width(220.0);
    let mut last_sep = true;
    let n = entries.len();
    for (i, e) in entries.into_iter().enumerate() {
        match e {
            Entry::Separator => {
                if !last_sep && i + 1 < n {
                    ui.separator();
                    last_sep = true;
                }
            }
            Entry::Heading(text) => {
                ui.label(egui::RichText::new(text).size(11.0).color(c.fg3));
                last_sep = false;
            }
            Entry::Item { label, enabled, danger, run } => {
                let text = egui::RichText::new(label).size(13.0);
                let text = if danger { text.color(REMOVED) } else { text };
                if ui.add_enabled(enabled, egui::Button::new(text).frame_when_inactive(false)).clicked() {
                    later.push(run);
                    ui.close();
                }
                last_sep = false;
            }
            Entry::Sub { label, enabled, children } => {
                ui.add_enabled_ui(enabled, |ui| {
                    ui.menu_button(egui::RichText::new(label).size(13.0), |ui| show(ui, children, later));
                });
                last_sep = false;
            }
        }
    }
}

/// Attach a right-click menu, built only when it opens.
pub fn attach(response: &egui::Response, later: &mut Vec<Later>, build: impl FnOnce() -> Vec<Entry>) {
    response.context_menu(|ui| show(ui, build(), later));
}

/// Which remote a remote-tracking name (origin/feature/x) belongs to; remote names can hold "/" too.
pub fn split_remote(repo: &RepoView, name: &str) -> Option<(String, String)> {
    let mut remotes = repo.data.remotes.clone();
    remotes.sort_by_key(|r| std::cmp::Reverse(r.len()));
    remotes.into_iter().find(|r| name.starts_with(&format!("{r}/"))).map(|r| {
        let branch = name[r.len() + 1..].to_owned();
        (r, branch)
    })
}

/// Whether `ancestor` is reachable from `tip` through loaded parents.
pub fn is_loaded_ancestor(repo: &RepoView, ancestor: &str, tip: &str) -> bool {
    let parents: std::collections::HashMap<&str, &Vec<String>> = repo.data.commits.iter().map(|c| (c.sha.as_str(), &c.parents)).collect();
    let mut pending = vec![tip.to_owned()];
    let mut seen = std::collections::HashSet::new();
    while let Some(sha) = pending.pop() {
        if sha == ancestor {
            return true;
        }
        if !seen.insert(sha.clone()) {
            continue;
        }
        if let Some(ps) = parents.get(sha.as_str()) {
            pending.extend(ps.iter().cloned());
        }
    }
    false
}

fn reset_menu(tab: u64, repo: &RepoView, sha: &str) -> Entry {
    let head = repo.data.head_branch.clone().unwrap_or_else(|| "HEAD".into());
    let (s1, s2, s3) = (sha.to_owned(), sha.to_owned(), sha.to_owned());
    let head2 = head.clone();
    sub(
        format!("Reset {head} to this commit"),
        vec![
            item("Soft - keep all changes", act(tab, Action::Reset { sha: s1, mode: "soft".into() })),
            item("Mixed - keep working copy but reset index", act(tab, Action::Reset { sha: s2, mode: "mixed".into() })),
            danger("Hard - discard all changes", move |app| {
                app.confirm_act(
                    tab,
                    &format!("Reset {head2} to this commit?"),
                    "Hard reset throws away all uncommitted changes. Undo can move the branch back, but cannot restore discarded work.",
                    "Reset Hard",
                    Action::Reset { sha: s3, mode: "hard".into() },
                )
            }),
        ],
    )
}

impl App {
    fn per_repo(&self, key: &str, path: &str) -> Vec<String> {
        self.settings.get::<std::collections::HashMap<String, Vec<String>>>(key).and_then(|m| m.get(path).cloned()).unwrap_or_default()
    }

    pub(crate) fn hidden_refs(&self, path: &str) -> Vec<String> {
        self.per_repo("hidden", path)
    }

    pub(crate) fn solo_refs(&self, path: &str) -> Vec<String> {
        self.per_repo("solo", path)
    }

    pub(crate) fn pinned_ref(&self, path: &str) -> Option<String> {
        self.settings.get::<std::collections::HashMap<String, String>>("pinned").and_then(|m| m.get(path).cloned())
    }

    pub(crate) fn branch_menu(&self, tab: u64, repo: &RepoView, r: &GitRef) -> Vec<Entry> {
        if r.kind == RefKind::Tag {
            return self.tag_menu(tab, repo, r);
        }
        let head = repo.data.head_branch.clone();
        let is_head = r.kind == RefKind::Local && Some(&r.name) == head.as_ref();
        let name = r.name.clone();
        let mut items = Vec::new();
        match r.kind {
            RefKind::Local if !is_head => items.push(item(format!("Checkout {name}"), act(tab, Action::Checkout { target: name.clone(), changes: Default::default() }))),
            RefKind::Remote => items.push(item(format!("Checkout {name}"), act(tab, Action::CheckoutRemote { remote_branch: name.clone(), stash_if_needed: false, changes: Default::default() }))),
            _ => {}
        }
        if let Some(head) = head.clone().filter(|_| !is_head) {
            items.push(item(format!("Merge {name} into {head}"), act(tab, Action::Merge { branch: name.clone() })));
            let (n2, h2) = (name.clone(), head.clone());
            items.push(item(format!("Squash merge {name} into {head}"), move |app| {
                if let Some(path) = app.repo(tab).map(|r| r.path.clone()) {
                    app.pending_drafts.push((path, super::repo::Draft { summary: format!("Squash merge {n2} into {h2}"), description: String::new(), amend: false }));
                }
                app.act(tab, Action::MergeSquash { branch: n2 });
            }));
            items.push(item(format!("Rebase {head} onto {name}"), act(tab, Action::Rebase { onto: name.clone() })));
            let n3 = name.clone();
            items.push(item(format!("Interactive Rebase {head} onto {name}"), move |app| app.open_rebase(tab, &n3, &n3, false)));
            if r.sha != repo.data.head_sha && is_loaded_ancestor(repo, &repo.data.head_sha, &r.sha) {
                items.push(item(format!("Fast-forward {head} to {name}"), act(tab, Action::FastForward { branch: head.clone(), to: name.clone() })));
            }
        }
        items.push(Entry::Separator);
        if is_head {
            items.push(item(format!("Push {name}"), act(tab, Action::Push { force: false, lease: None, review: None })));
        }
        if r.kind == RefKind::Local {
            let n = name.clone();
            items.push(item(format!("Start a pull request from {name}"), move |app| app.start_pull_request(tab, &n)));
        }
        let n = name.clone();
        items.push(item("Explain Branch Changes (Preview)", move |app| app.explain_branch(tab, &n)));
        if r.kind == RefKind::Local && !is_head && !r.upstream.is_empty() && r.behind > 0 && r.ahead == 0 {
            items.push(item(format!("Fast-forward {name} to {}", r.upstream), act(tab, Action::FastForward { branch: name.clone(), to: r.upstream.clone() })));
        }
        if r.kind == RefKind::Local {
            let label = if r.upstream.is_empty() { "Set Upstream".to_owned() } else { format!("Set Upstream (now {})", r.upstream) };
            let n = name.clone();
            items.push(item(label, move |app| app.prompt_upstream_for(tab, &n)));
        }
        let sha = r.sha.clone();
        items.push(item("Create branch here", move |app| app.prompt_branch(tab, Some(sha), true)));
        items.push(reset_menu(tab, repo, &r.sha));
        let sha = r.sha.clone();
        items.push(item(format!("Create worktree from {name}"), move |app| app.add_worktree(tab, Some(sha))));
        if repo.data.head_branch.is_some() && head_line(repo).contains(&r.sha) {
            let sha = r.sha.clone();
            items.push(item("Rename commit", move |app| app.reword_commit(tab, &sha)));
        }
        let sha = r.sha.clone();
        items.push(item("Create tag here", move |app| app.prompt_tag(tab, Some(sha), false, String::new())));
        items.push(Entry::Separator);
        if r.kind == RefKind::Local {
            let n = name.clone();
            items.push(item(format!("Rename {name}"), move |app| app.prompt_rename_branch(tab, &n)));
            let n = name.clone();
            items.push(off(
                danger(format!("Delete {name}"), move |app| {
                    app.confirm_act(tab, &format!("Delete {n}?"), "The branch label is removed. You can undo this with Undo.", "Delete", Action::DeleteBranch { name: n.clone() })
                }),
                !is_head,
            ));
        } else {
            let n = name.clone();
            items.push(danger(format!("Delete {name}"), move |app| {
                app.confirm_act(tab, &format!("Delete {n} from the remote?"), "This removes the branch on the remote server for everyone.", "Delete", Action::DeleteRemoteBranch { remote_branch: n.clone() })
            }));
        }
        items.push(Entry::Separator);
        let path = repo.path.clone();
        let hidden = self.hidden_refs(&path).contains(&r.full);
        let soloed = self.solo_refs(&path).contains(&r.full);
        let full = r.full.clone();
        items.push(off(item(if hidden { "Show in the graph" } else { "Hide in the graph" }, move |app| app.toggle_ref_list(tab, "hidden", &full)), !(is_head && !hidden)));
        let full = r.full.clone();
        items.push(item(if soloed { "Stop soloing this branch" } else { "Solo" }, move |app| app.toggle_ref_list(tab, "solo", &full)));
        let full = r.full.clone();
        let pinned = self.pinned_ref(&path).as_deref() == Some(r.full.as_str());
        items.push(item(if pinned { "Unpin from Left" } else { "Pin to Left" }, move |app| app.toggle_pinned(tab, &full)));
        if !self.solo_refs(&path).is_empty() {
            items.push(item("Stop soloing all branches", move |app| app.clear_solo(tab)));
        }
        items.push(Entry::Separator);
        let n = name.clone();
        items.push(item("Copy branch name", move |app| app.copy(&n)));
        let sha = r.sha.clone();
        items.push(item("Copy commit sha", move |app| app.copy(&sha)));
        let on_remote = split_remote(repo, if r.kind == RefKind::Remote { &r.name } else { &r.upstream });
        if let Some((remote, branch)) = on_remote {
            items.extend(self.link_items(tab, repo, "Copy link to branch", super::weblinks::Target::Branch(branch), Some(remote)));
        }
        items
    }

    fn tag_menu(&self, tab: u64, repo: &RepoView, r: &GitRef) -> Vec<Entry> {
        let name = r.name.clone();
        // The tag's commit itself: a branch of the same name must not be checked out instead.
        let mut items = vec![item(format!("Checkout the commit at {name}"), act(tab, Action::Checkout { target: r.sha.clone(), changes: Default::default() }))];
        let sha = r.sha.clone();
        items.push(item("Create branch here", move |app| app.prompt_branch(tab, Some(sha), true)));
        items.push(Entry::Separator);
        if !repo.data.remotes.is_empty() {
            items.push(sub(format!("Push {name} to remote"), repo.data.remotes.iter().map(|remote| item(remote.clone(), act(tab, Action::PushTag { name: name.clone(), remote: remote.clone() }))).collect()));
            items.push(sub(
                format!("Delete {name} from remote"),
                repo.data
                    .remotes
                    .iter()
                    .map(|remote| {
                        let (n, rem) = (name.clone(), remote.clone());
                        danger(remote.clone(), move |app| {
                            app.confirm_act(tab, &format!("Delete {n} from {rem}?"), "This deletes the tag on the remote for everyone. Your local tag is kept.", "Delete Remote Tag", Action::DeleteRemoteTag { name: n.clone(), remote: rem.clone() })
                        })
                    })
                    .collect(),
            ));
            items.push(Entry::Separator);
        }
        items.push(danger(format!("Delete {name} locally"), act(tab, Action::DeleteTag { name: name.clone() })));
        let hidden = self.hidden_refs(&repo.path).contains(&r.full);
        let full = r.full.clone();
        items.push(item(if hidden { "Show in the graph" } else { "Hide in the graph" }, move |app| app.toggle_ref_list(tab, "hidden", &full)));
        items.push(Entry::Separator);
        let n = name.clone();
        items.push(item("Copy tag name", move |app| app.copy(&n)));
        items.extend(self.link_items(tab, repo, "Copy link to this tag", super::weblinks::Target::Tag(name), None));
        items
    }

    fn commit_action(&self, tab: u64, repo: &RepoView, c: &Commit, revert: bool, enabled: bool) -> Entry {
        let label = if revert { "Revert commit" } else { "Cherry pick commit" };
        let sha = c.sha.clone();
        if c.parents.len() < 2 {
            let action = if revert { Action::Revert { sha, mainline: None } } else { Action::CherryPick { sha, mainline: None } };
            return off(item(label, act(tab, action)), enabled);
        }
        let children = c
            .parents
            .iter()
            .enumerate()
            .map(|(i, parent)| {
                let summary = repo.data.commits.iter().find(|p| &p.sha == parent).map(|p| format!(" — {}", p.summary)).unwrap_or_default();
                let short: String = parent.chars().take(6).collect();
                let (sha, parent) = (sha.clone(), parent.clone());
                item(format!("Relative to parent {}: {short}{summary}", i + 1), move |app| {
                    let action = if revert { Action::Revert { sha, mainline: Some(i + 1) } } else { Action::CherryPick { sha, mainline: Some(i + 1) } };
                    let verb = if revert { "Undo" } else { "Apply" };
                    app.confirm_act(
                        tab,
                        &format!("{label} relative to parent {}?", i + 1),
                        &format!("{verb} the merge's changes compared with parent {} ({parent}).", i + 1),
                        if revert { "Revert" } else { "Cherry Pick" },
                        action,
                    );
                })
            })
            .collect();
        off(sub(label, children), enabled)
    }

    pub(crate) fn commit_menu(&self, tab: u64, repo: &RepoView, c: &Commit) -> Vec<Entry> {
        let head = repo.data.head_branch.clone().unwrap_or_else(|| "HEAD".into());
        let is_head = c.sha == repo.data.head_sha;
        let line = head_line(repo);
        let on_line = repo.data.head_branch.is_some() && line.contains(&c.sha);
        let refs: Vec<&GitRef> = repo.data.refs.iter().filter(|r| r.sha == c.sha && r.kind != RefKind::Tag).collect();
        let sha = c.sha.clone();
        let mut items = Vec::new();
        match &repo.data.bisect {
            Some(b) if b.first_bad.is_none() => {
                let (a, b2, s) = (sha.clone(), sha.clone(), sha.clone());
                items.push(sub(
                    "Bisect",
                    vec![
                        item("Mark this commit good", move |app| app.run_bisect(tab, "good", Some(a), None)),
                        item("Mark this commit bad", move |app| app.run_bisect(tab, "bad", Some(b2), None)),
                        item("Skip this commit", move |app| app.run_bisect(tab, "skip", Some(s), None)),
                    ],
                ));
            }
            None => {
                let s = sha.clone();
                items.push(item("Start bisect here (bad)", move |app| {
                    let s = s.clone();
                    app.show_dialog(super::dialogs::Dialog::prompt("Start bisect", "Known-good commit, branch, or tag", "", "Start bisect", move |app, v| app.run_bisect(tab, "start", Some(s), Some(v.text("value").trim().to_owned()))));
                }));
            }
            _ => {}
        }
        if repo.data.head_branch.is_some() && !is_head {
            items.push(item(format!("Rebase {head} onto this commit"), act(tab, Action::Rebase { onto: sha.clone() })));
        }
        let mut checkout: Vec<Entry> = refs
            .iter()
            .map(|r| {
                let action = if r.kind == RefKind::Remote { Action::CheckoutRemote { remote_branch: r.name.clone(), stash_if_needed: false, changes: Default::default() } } else { Action::Checkout { target: r.name.clone(), changes: Default::default() } };
                off(item(r.name.clone(), act(tab, action)), !(r.kind == RefKind::Local && Some(&r.name) == repo.data.head_branch.as_ref()))
            })
            .collect();
        checkout.push(item("this commit (detached HEAD)", act(tab, Action::Checkout { target: sha.clone(), changes: Default::default() })));
        items.push(sub("Checkout", checkout));
        let s = sha.clone();
        let mut worktree = vec![item("this commit", move |app| app.add_worktree(tab, Some(s)))];
        for r in &refs {
            let s = r.sha.clone();
            worktree.push(item(r.name.clone(), move |app| app.add_worktree(tab, Some(s))));
        }
        items.push(sub("Create worktree from", worktree));
        let s = sha.clone();
        items.push(item("Create branch here", move |app| app.prompt_branch(tab, Some(s), true)));
        items.push(Entry::Separator);
        items.push(self.commit_action(tab, repo, c, false, !is_head));
        items.push(reset_menu(tab, repo, &c.sha));
        let s = sha.clone();
        items.push(off(item("Rename commit", move |app| app.reword_commit(tab, &s)), on_line));
        let (s, summary) = (sha.clone(), c.summary.clone());
        items.push(off(
            item("Create fixup commit for this commit", move |app| {
                app.confirm_act(tab, "Create fixup commit", &format!("Commit the currently staged changes as a fixup for “{summary}”?"), "Create Fixup", Action::CreateFixup { target: s.clone() })
            }),
            on_line && !repo.data.status.staged.is_empty(),
        ));
        items.push(self.commit_action(tab, repo, c, true, true));
        let (s, name) = (sha.clone(), super::patches::patch_name(&c.summary));
        items.push(item("Create patch from commit", move |app| app.save_patch(tab, vec![s], &name)));
        if repo.data.head_branch.is_some() && !repo.data.remotes.is_empty() {
            let h = head.clone();
            items.push(item(format!("Push {head} and start a pull request"), move |app| app.start_pull_request(tab, &h)));
        }
        if let Some(r) = refs.first() {
            let n = r.name.clone();
            items.push(item("Explain Branch Changes (Preview)", move |app| app.explain_branch(tab, &n)));
        }
        items.push(Entry::Separator);
        let s = sha.clone();
        items.push(item("Compare commit against working directory", move |app| {
            if let Some(r) = app.repo_mut(tab) {
                r.ui.compare_workdir = Some(s.clone());
                r.ui.multi.clear();
                r.ui.selection = Some(super::repo::Selection::Commit(s));
                r.ui.diff = None;
                r.ui.history = None;
            }
        }));
        items.push(Entry::Separator);
        items.push(sub("Edit commit history", self.history_items(tab, repo, c)));
        items.push(Entry::Separator);
        let pinned = self.pinned_ref(&repo.path);
        for r in &refs {
            let n = r.name.clone();
            items.push(item(format!("Copy branch name: {}", r.name), move |app| app.copy(&n)));
            let full = r.full.clone();
            let label = if pinned.as_deref() == Some(r.full.as_str()) { format!("Unpin {} from Left", r.name) } else { format!("Pin to Left: {}", r.name) };
            items.push(item(label, move |app| app.toggle_pinned(tab, &full)));
            let full = r.full.clone();
            items.push(item(format!("Solo: {}", r.name), move |app| app.toggle_ref_list(tab, "solo", &full)));
        }
        let s = sha.clone();
        items.push(item("Copy commit sha", move |app| app.copy(&s)));
        let message = if c.body.is_empty() { c.summary.clone() } else { format!("{}\n\n{}", c.summary, c.body) };
        items.push(item("Copy commit message", move |app| app.copy(&message)));
        items.extend(self.link_items(tab, repo, "Copy link to this commit", super::weblinks::Target::Commit(sha.clone()), None));
        items.push(Entry::Separator);
        let s = sha.clone();
        items.push(item("Create tag here", move |app| app.prompt_tag(tab, Some(s), false, String::new())));
        let s = sha.clone();
        items.push(item("Create annotated tag here", move |app| app.prompt_tag(tab, Some(s), true, String::new())));
        items
    }

    fn history_items(&self, tab: u64, repo: &RepoView, c: &Commit) -> Vec<Entry> {
        let line = head_line(repo);
        let Some(pos) = line.iter().position(|s| *s == c.sha).filter(|_| repo.data.head_branch.is_some()) else {
            return vec![off(item("Select a commit on the checked-out branch", |_| {}), false)];
        };
        let head = repo.data.head_branch.clone().unwrap_or_default();
        let short: String = c.sha.chars().take(6).collect();
        let mut items = Vec::new();
        if pos > 0 {
            let s = c.sha.clone();
            let label = short.clone();
            items.push(item(format!("Interactive Rebase {pos} children of {short}"), move |app| app.open_rebase(tab, &s, &label, false)));
        }
        let base = base_for(repo, &c.sha);
        let h = head.clone();
        items.push(item(format!("Interactive Rebase {head} from here"), move |app| app.open_rebase(tab, &base, &h, false)));
        if pos + 1 < line.len() {
            let s = c.sha.clone();
            items.push(item("Squash into the commit below", move |app| app.squash_commits(tab, vec![s])));
        }
        let s = c.sha.clone();
        items.push(danger("Drop commit", move |app| app.drop_commits(tab, vec![s])));
        if pos > 0 {
            let s = c.sha.clone();
            items.push(item("Move commit up", move |app| app.move_commit(tab, &s, true)));
        }
        if pos + 1 < line.len() {
            let s = c.sha.clone();
            items.push(item("Move commit down", move |app| app.move_commit(tab, &s, false)));
        }
        items
    }

    pub(crate) fn stash_menu(&self, tab: u64, st: &Stash) -> Vec<Entry> {
        let (index, sha, message) = (st.index, st.sha.clone(), st.message.clone());
        let (s1, s2, s3, s4, s5) = (sha.clone(), sha.clone(), sha.clone(), sha.clone(), sha.clone());
        let m = message.clone();
        vec![
            item("Apply Stash", act(tab, Action::StashApply { index, expected: Some(s1), without_index: false })),
            item("Pop Stash", act(tab, Action::StashPop { index, expected: Some(s2), without_index: false })),
            item("Edit stash message", move |app| {
                app.show_dialog(super::dialogs::Dialog::prompt("Edit stash message", "Stash message", &m, "Save", move |app, v| {
                    app.act(tab, Action::StashRename { index, expected: Some(s3), message: v.text("value") })
                }))
            }),
            danger("Delete Stash", move |app| app.confirm_act(tab, "Delete this stash?", &message, "Delete", Action::StashDrop { index, expected: Some(s4) })),
            Entry::Separator,
            item("Copy stash sha", move |app| app.copy(&s5)),
        ]
    }

    /// The menu of a file in the right panel. `commit`: the commit (or stash) the file is shown from.
    pub(crate) fn file_menu(&self, tab: u64, repo: &RepoView, file: &str, staged: bool, in_commit: bool, commit: Option<&str>, storage: Option<&str>) -> Vec<Entry> {
        let f = file.to_owned();
        let mut items = Vec::new();
        if let Some(stash) = commit.and_then(|sha| repo.data.stashes.iter().find(|s| s.sha == sha)) {
            items.push(item("Apply stashed file", act(tab, Action::StashTakeFile { index: stash.index, expected: Some(stash.sha.clone()), path: f.clone() })));
            items.push(Entry::Separator);
        }
        if !in_commit {
            if staged {
                let f2 = f.clone();
                items.push(item("Unstage", move |app| app.act_quiet(tab, Action::Unstage { paths: vec![f2] }, true)));
            } else {
                let f2 = f.clone();
                items.push(item("Stage", move |app| app.act_quiet(tab, Action::Stage { paths: vec![f2] }, true)));
                items.push(item("Stash file", act(tab, Action::StashFiles { paths: vec![f.clone()], message: String::new() })));
                let f2 = f.clone();
                items.push(danger("Discard changes", move |app| {
                    app.confirm_act(tab, "Discard changes?", &format!("Discard all changes to {f2}? You can undo this."), "Discard", Action::Discard { paths: vec![f2.clone()] })
                }));
            }
            items.push(Entry::Separator);
        }
        let dir = file.rfind('/').map(|i| file[..=i].to_owned()).unwrap_or_default();
        let name = file.rsplit('/').next().unwrap_or(file);
        let ext = name.rfind('.').filter(|i| *i > 0).map(|i| name[i..].to_owned());
        let tracked = staged || in_commit || repo.data.status.unstaged.iter().any(|x| x.path == file && x.status != '?');
        let ignore = |pattern: String, untrack: Vec<String>| act(tab, Action::Ignore { pattern, untrack: if tracked { untrack } else { Vec::new() } });
        let mut ign = vec![item("Ignore this file", ignore(literal_ignore_path(file), vec![f.clone()]))];
        if !dir.is_empty() {
            ign.push(item(format!("Ignore all files in \"{dir}\""), ignore(literal_ignore_path(&dir), vec![dir.clone()])));
        }
        if let Some(ext) = &ext {
            ign.push(item(format!("Ignore all files with extension {ext}"), ignore(format!("*{ext}"), vec![f.clone()])));
        }
        items.push(sub(if tracked { "Ignore and stop tracking" } else { "Ignore" }, ign));
        if let Some(ext) = &ext {
            items.push(item(format!("Track {ext} files with Git LFS"), act(tab, Action::LfsTrack { pattern: format!("*{ext}"), lockable: false })));
        }
        if !in_commit {
            let mut lfs = vec![
                item("Lock file", act(tab, Action::LfsLock { path: f.clone() })),
                item("Unlock file", act(tab, Action::LfsUnlock { path: f.clone(), force: false })),
            ];
            if let Some(ext) = &ext {
                lfs.push(item("Track this extension and require a lock", act(tab, Action::LfsTrack { pattern: format!("*{ext}"), lockable: true })));
            }
            items.push(sub("Git LFS", lfs));
            let tool = self.settings.string("diffTool", "");
            items.push(item("Open in External Diff Tool", act(tab, Action::ExternalDiff { file: f.clone(), staged, tool })));
        }
        if repo.data.status.unstaged.iter().any(|x| x.path == file && x.status == 'U') {
            let tool = self.settings.string("mergeTool", "");
            items.push(item("Open in External Merge Tool", act(tab, Action::ExternalMerge { file: f.clone(), tool })));
        }
        items.push(Entry::Separator);
        let f2 = f.clone();
        items.push(item("Copy file path", move |app| app.copy(&f2)));
        let rev = if in_commit { storage.or(commit).map(str::to_owned) } else if staged { Some(":".to_owned()) } else { None };
        let f2 = f.clone();
        items.push(item("Copy file contents to clipboard", move |app| app.copy_file_text(tab, &f2, rev)));
        let editor = super::external_tools::editor_name(&self.settings.string("externalEditor", "vscode"));
        let f2 = f.clone();
        items.push(item(format!("Open in {editor}"), move |app| app.open_in_editor(tab, Some(&f2))));
        let f2 = f.clone();
        items.push(item("Open file in default program", move |app| app.open_default(tab, &f2)));
        let f2 = f.clone();
        items.push(item("Open in File Manager", move |app| app.reveal_file(tab, &f2)));
        items.push(Entry::Separator);
        let f2 = f.clone();
        items.push(item("File History", move |app| app.open_history(tab, &f2, super::repo::HistoryMode::History)));
        if let Some(sep) = file.rfind('/').filter(|i| *i > 0) {
            let folder = file[..sep].to_owned();
            items.push(item("Containing folder history", move |app| {
                app.set_history_filter(tab, Some(gitcraft_core::git::HistoryFilter { path: Some(folder), ..Default::default() }));
            }));
        }
        let f2 = f.clone();
        items.push(item("Blame", move |app| app.open_history(tab, &f2, super::repo::HistoryMode::Blame)));
        items
    }
}

/// A .gitignore pattern that matches exactly this path (special characters escaped).
pub fn literal_ignore_path(path: &str) -> String {
    let mut out = String::from("/");
    for ch in path.chars() {
        if matches!(ch, '\\' | '*' | '?' | '[' | ']' | '#' | '!' | ' ') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignore_patterns_are_literal() {
        assert_eq!(literal_ignore_path("src/a*.rs"), "/src/a\\*.rs");
        assert_eq!(literal_ignore_path("#my notes"), "/\\#my\\ notes");
    }
}
