//! Unsaved editor text is never dropped without asking. The file editor's draft and the
//! conflict editor's picks or manual edit are held back when the tab moves to another file or
//! closes, when the window closes and when GitCraft restarts for an update, with a prompt
//! offering Save, Discard and Cancel. A failed save keeps the text and stays where it was.

use super::App;
use super::conflict::ConflictUi;
use super::diff::DiffUi;
use super::dialogs::{Body, Dialog};
use super::repo::{RepoTab, TabUi};
use super::widgets::Kind;
use std::cell::RefCell;
use std::rc::Rc;

/// Which of a tab's editors holds unsaved text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Editor {
    File,
    Conflict,
}

/// What runs once the saves asked for by Save have all finished.
pub struct Leaving {
    waiting: Vec<(u64, Editor)>,
    then: Box<dyn FnOnce(&mut App)>,
}

type Then = Rc<RefCell<Option<Box<dyn FnOnce(&mut App)>>>>;

pub fn file_dirty(d: &DiffUi) -> bool {
    d.editing && d.draft.is_some()
}

pub fn conflict_dirty(c: &ConflictUi) -> bool {
    c.base.is_some() && (!c.picks.is_empty() || c.manual.is_some())
}

pub fn discard_conflict(c: &mut ConflictUi) {
    c.base = None;
    c.picks.clear();
    c.manual = None;
}

fn discard(r: &mut RepoTab) {
    r.diff_ui.draft = None;
    r.diff_ui.editing = false;
    discard_conflict(&mut r.conflict_ui);
}

/// The tab's editors with unsaved text, and the file each one is of.
fn unsaved(r: &RepoTab) -> Vec<(Editor, String)> {
    let mut out = Vec::new();
    if file_dirty(&r.diff_ui)
        && let Some(t) = &r.diff_ui.target
    {
        out.push((Editor::File, t.file.clone()));
    }
    if conflict_dirty(&r.conflict_ui) {
        out.push((Editor::Conflict, r.conflict_ui.file.clone()));
    }
    out
}

/// The editor the tab's centre shows, as an editor's identity: the file editor of a diff (by
/// its key) or the conflict editor of a file.
fn shown(ui: &TabUi) -> Option<(Editor, String)> {
    if let Some(f) = &ui.conflict {
        return Some((Editor::Conflict, f.clone()));
    }
    ui.diff.as_ref().map(|t| (Editor::File, super::diff::diff_key(t, false)))
}

/// The identity of an editor with unsaved text, comparable with [`shown`].
fn identity(r: &RepoTab, editor: Editor) -> (Editor, String) {
    match editor {
        Editor::File => (Editor::File, r.diff_ui.key.clone()),
        Editor::Conflict => (Editor::Conflict, r.conflict_ui.file.clone()),
    }
}

/// Whether moving from `before` to `now` would hide or replace an editor with unsaved text.
fn loses(r: &RepoTab, before: &TabUi, now: &TabUi) -> bool {
    let dirty: Vec<(Editor, String)> = unsaved(r).into_iter().map(|(e, _)| identity(r, e)).collect();
    if dirty.is_empty() {
        return false;
    }
    let (was, is) = (shown(before), shown(now));
    // Leaving the editor shown.
    if was != is && was.as_ref().is_some_and(|w| dirty.contains(w)) {
        return true;
    }
    // Opening another file in an editor that still holds unsaved text (its view would reset).
    if let Some(t) = &now.diff
        && file_dirty(&r.diff_ui)
        && super::diff::diff_key(t, false) != r.diff_ui.key
    {
        return true;
    }
    now.conflict.as_ref().is_some_and(|f| conflict_dirty(&r.conflict_ui) && *f != r.conflict_ui.file)
}

impl App {
    /// Run each frame after everything that could move a tab: a move away from unsaved editor
    /// text is undone and held for the prompt.
    pub(crate) fn guard_unsaved_moves(&mut self) {
        if self.discarding {
            return;
        }
        let tabs: Vec<u64> = self.repos.keys().copied().collect();
        for tab in tabs {
            let Some(r) = self.repos.get_mut(&tab) else { continue };
            if unsaved(r).is_empty() {
                r.guard = None;
                continue;
            }
            let held = match &r.guard {
                Some(before) if loses(r, before, &r.ui) => std::mem::replace(&mut r.ui, before.clone()),
                _ => {
                    r.guard = Some(r.ui.clone());
                    continue;
                }
            };
            if self.dialog.is_some() {
                let file = unsaved(r).into_iter().next().map(|(_, f)| f).unwrap_or_default();
                self.toast(false, format!("Save or discard the changes to {file} first."));
                continue;
            }
            self.ask_unsaved(vec![tab], true, Box::new(move |app: &mut App| {
                if let Some(r) = app.repo_mut(tab) {
                    r.ui = held;
                    r.guard = None;
                }
            }));
        }
    }

    /// Whether any of `tabs` (all tabs when `None`) has unsaved editor text.
    pub(crate) fn has_unsaved(&self, tabs: Option<&[u64]>) -> bool {
        self.repos.iter().any(|(id, r)| tabs.is_none_or(|t| t.contains(id)) && !unsaved(r).is_empty())
    }

    /// Ask before `then` when any of `tabs` (all tabs when empty) has unsaved editor text.
    /// Returns false when there is none and the caller goes on itself; otherwise the prompt runs
    /// `then` after Save (once every save succeeded) or Discard. `clear` makes Discard drop the
    /// text; without it Discard lets `then` go ahead with the text left in place (the window is
    /// closing), so a failure on the way keeps it.
    pub(crate) fn ask_unsaved(&mut self, tabs: Vec<u64>, clear: bool, then: Box<dyn FnOnce(&mut App)>) -> bool {
        if self.discarding {
            return false;
        }
        let mut pending: Vec<(u64, Editor, String)> = Vec::new();
        let mut ids: Vec<&u64> = self.repos.keys().filter(|id| tabs.is_empty() || tabs.contains(id)).collect();
        ids.sort();
        for id in ids {
            pending.extend(unsaved(&self.repos[id]).into_iter().map(|(e, f)| (*id, e, f)));
        }
        if pending.is_empty() {
            return false;
        }
        if pending.iter().any(|(id, ..)| self.repos.get(id).is_some_and(|r| r.diff_ui.saving || r.conflict_ui.saving)) || self.leaving.is_some() {
            self.toast(false, "Wait for the file to finish saving.");
            return true;
        }
        let files: Vec<&str> = pending.iter().map(|(.., f)| f.as_str()).collect();
        let title = if pending.len() == 1 { "Save changes before leaving?" } else { "Save changes before closing?" };
        let text = format!("{}\n\nYour changes are lost if you don't save them.", files.join("\n"));
        let then: Then = Rc::new(RefCell::new(Some(then)));
        let (discard_then, save_then) = (then.clone(), then);
        let discard_tabs: Vec<u64> = pending.iter().map(|(id, ..)| *id).collect();
        let waiting: Vec<(u64, Editor)> = pending.iter().map(|(id, e, _)| (*id, *e)).collect();
        self.dialog = Some(
            Dialog::new(title, Body::Text(text))
                .choice("Discard", Kind::Danger, move |app, _| {
                    if clear {
                        for id in &discard_tabs {
                            if let Some(r) = app.repo_mut(*id) {
                                discard(r);
                                r.guard = None;
                            }
                        }
                    } else {
                        app.discarding = true;
                    }
                    if let Some(f) = discard_then.borrow_mut().take() {
                        f(app);
                    }
                })
                .choice("Save", Kind::Primary, move |app, _| {
                    let Some(then) = save_then.borrow_mut().take() else { return };
                    app.leaving = Some(Leaving { waiting: waiting.clone(), then });
                    for (id, editor) in waiting {
                        let started = match editor {
                            Editor::File => app.save_draft(id, false),
                            Editor::Conflict => app.save_conflict(id, false),
                        };
                        if !started {
                            app.left_saved(id, editor, false);
                        }
                    }
                }),
        );
        true
    }

    /// A save asked for by the prompt finished. A failure keeps the text and drops what waited;
    /// the last success runs it.
    pub(crate) fn left_saved(&mut self, tab: u64, editor: Editor, ok: bool) {
        let Some(l) = self.leaving.as_mut() else { return };
        let Some(at) = l.waiting.iter().position(|w| *w == (tab, editor)) else { return };
        if !ok {
            self.leaving = None;
            return;
        }
        l.waiting.remove(at);
        if l.waiting.is_empty()
            && let Some(l) = self.leaving.take()
        {
            (l.then)(self);
        }
    }

    /// The window was asked to close: hold it while editor text is unsaved. Runs in `logic`,
    /// which also runs while the window is minimized.
    pub(crate) fn guard_close(&mut self, ctx: &egui::Context) {
        if self.discarding || !ctx.input(|i| i.viewport().close_requested()) || !self.has_unsaved(None) {
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        self.ask_unsaved(Vec::new(), false, Box::new(|app: &mut App| app.ctx.send_viewport_cmd(egui::ViewportCommand::Close)));
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::repo::DiffTarget;
    use gitcraft_core::git::DiffSource;

    fn target(file: &str) -> DiffTarget {
        DiffTarget { source: DiffSource::Unstaged, file: file.into(), untracked: false, parent: None }
    }

    fn editing(file: &str) -> DiffUi {
        let t = target(file);
        DiffUi { key: super::super::diff::diff_key(&t, false), target: Some(t), editing: true, draft: Some("edited".into()), ..Default::default() }
    }

    fn ui_with(file: Option<&str>) -> TabUi {
        TabUi { diff: file.map(target), ..Default::default() }
    }

    #[test]
    fn only_a_draft_being_edited_is_unsaved() {
        let mut d = editing("a.txt");
        assert!(file_dirty(&d));
        d.editing = false;
        assert!(!file_dirty(&d));
        let c = ConflictUi::default();
        assert!(!conflict_dirty(&c));
    }

    #[test]
    fn moves_away_from_or_over_a_draft_are_held() {
        let mut r = RepoTab::new("/repo");
        r.diff_ui = editing("a.txt");
        let before = ui_with(Some("a.txt"));
        assert!(!loses(&r, &before, &ui_with(Some("a.txt"))), "staying on the file");
        assert!(loses(&r, &before, &ui_with(Some("b.txt"))), "another file");
        assert!(loses(&r, &before, &ui_with(None)), "closing the file");
        // The draft's view was hidden (a pull request over it): opening another file still asks.
        assert!(loses(&r, &ui_with(None), &ui_with(Some("b.txt"))));
        assert!(!loses(&r, &ui_with(None), &ui_with(Some("a.txt"))));
    }
}
