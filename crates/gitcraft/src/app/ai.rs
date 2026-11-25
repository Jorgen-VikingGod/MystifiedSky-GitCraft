//! The AI buttons: commit and stash messages, explaining a commit or branch, composing commits
//! and merging a conflict. Each request runs with a snapshot of Preferences > AI; untracked files
//! are listed for review before they are sent.

use super::App;
use super::dialogs::Dialog;
use super::widgets::Kind;
use gitcraft_core::actions::Action;
use gitcraft_core::ai::{self, AiConfig, ComposedCommit};
use gitcraft_core::ai_context::{self, PreviewKind};
use std::path::Path;

/// What an AI request produced, and where it goes.
pub enum AiOutput {
    CommitMessage { path: String, before: super::repo::Draft, summary: String, description: String },
    StashTitle { tab: u64, before: String, text: String },
    Explain { title: String, text: String },
    Compose { tab: u64, commits: Vec<ComposedCommit> },
    Merge { tab: u64, file: String, editor: u64, revision: u64, text: String },
    PullText { tab: u64, request: u64, title: String, body: String },
}

/// A request that may need the untracked files reviewed first.
#[derive(Clone, Copy)]
enum Reviewed {
    Commit,
    Stash,
    Compose,
}

impl App {
    /// Preferences > AI as the backend reads it.
    pub(crate) fn ai_config(&self) -> AiConfig {
        let mut base = serde_json::to_value(AiConfig::default()).unwrap_or_default();
        if let (Some(obj), Some(saved)) = (base.as_object_mut(), self.settings.get::<serde_json::Value>("ai").and_then(|v| v.as_object().cloned())) {
            for (k, v) in saved {
                obj.insert(k, v);
            }
        }
        serde_json::from_value(base).unwrap_or_default()
    }

    /// Run one AI request away from the window, with a busy note and Stop AI.
    fn ai_run(&mut self, label: &str, job: impl FnOnce() -> Result<AiOutput, String> + Send + 'static) {
        self.ai_requests += 1;
        let id = format!("ai-{}", self.ai_requests);
        self.ai_reads.push(id.clone());
        self.ai_label = Some(label.to_owned());
        let config = self.ai_config();
        let read = id.clone();
        self.spawn(move || {
            let result = gitcraft_core::read_jobs::scope_for(Some(read), std::time::Duration::from_secs(300), || ai::with_config(config, job));
            super::Msg::Ai { id, result: Box::new(result) }
        });
    }

    pub(crate) fn stop_ai(&mut self) {
        for id in std::mem::take(&mut self.ai_reads) {
            gitcraft_core::read_jobs::cancel(&id);
        }
        self.ai_label = None;
        // A stopped AI merge no longer keeps the conflict editor busy.
        for r in self.repos.values_mut() {
            r.conflict_ui.ai_busy = false;
        }
    }

    pub(crate) fn ai_done(&mut self, id: String, result: Result<AiOutput, String>) {
        // A request stopped with Stop AI changes nothing, whatever it brings back.
        if !self.ai_reads.contains(&id) {
            return;
        }
        self.ai_reads.retain(|r| *r != id);
        if self.ai_reads.is_empty() {
            self.ai_label = None;
        }
        let out = match result {
            Ok(out) => out,
            Err(e) => {
                if e.contains("Preferences > AI") {
                    self.open_prefs(Some("ai"));
                }
                return self.toast(false, e);
            }
        };
        match out {
            AiOutput::CommitMessage { path, before, summary, description } => {
                let now = self.draft(&path);
                if now != before {
                    let text = format!("{summary}\n\n{description}\n\nYour draft changed while this was generated.");
                    self.dialog = Some(Dialog::new("Generated commit message", super::dialogs::Body::Text(text)).choice("Use generated message", Kind::Default, move |app, _| {
                        let mut d = app.draft(&path);
                        d.summary = summary;
                        d.description = description;
                        app.set_draft(&path, d);
                    }));
                } else {
                    let mut d = now;
                    d.summary = summary;
                    d.description = description;
                    self.set_draft(&path, d);
                }
            }
            AiOutput::StashTitle { tab, before, text } => {
                // A title typed while this was generated is kept unless the generated one is chosen.
                let now = self.repo(tab).map(|r| r.work.stash_message.clone()).unwrap_or_default();
                if now == before {
                    if let Some(r) = self.repo_mut(tab) {
                        r.work.stash_message = text;
                    }
                } else {
                    let shown = format!("{text}\n\nYour stash title changed while this was generated.");
                    self.dialog = Some(Dialog::new("Generated stash title", super::dialogs::Body::Text(shown)).choice("Use generated title", Kind::Default, move |app, _| {
                        if let Some(r) = app.repo_mut(tab) {
                            r.work.stash_message = text;
                        }
                    }));
                }
            }
            AiOutput::Explain { title, text } => {
                let text = format!("{text}\n\nGenerated using your selected AI connection. Review the result before using it.");
                self.dialog = Some(Dialog::info(&title, &text));
            }
            AiOutput::Compose { tab, commits } => {
                let n = commits.len();
                let list = commits.iter().enumerate().map(|(i, c)| format!("{}. {}\n{}", i + 1, c.summary, c.files.iter().map(|f| format!("     {f}")).collect::<Vec<_>>().join("\n"))).collect::<Vec<_>>().join("\n\n");
                let plural = if n == 1 { "" } else { "s" };
                self.dialog = Some(Dialog::confirm_with(&format!("Create {n} commit{plural}?"), &format!("{list}\n\nYou can undo this with Undo."), &format!("Create {n} Commit{plural}"), false, move |app, _| app.act(tab, Action::ComposeCommits { commits })));
            }
            AiOutput::Merge { tab, file, editor, revision, text } => self.ai_merged(tab, &file, editor, revision, Ok(text)),
            AiOutput::PullText { tab, request, title, body } => self.pr_text_ready(tab, request, title, body),
        }
    }

    /// List the untracked files a request would send, ask, then send.
    fn with_reviewed(&mut self, tab: u64, kind: Reviewed) {
        let Some(path) = self.repo(tab).map(|r| r.path.clone()) else { return };
        let preview_kind = match kind {
            Reviewed::Commit => PreviewKind::Commit,
            Reviewed::Stash => PreviewKind::Stash,
            Reviewed::Compose => PreviewKind::Compose,
        };
        self.ai_label = Some("Checking which files to share…".into());
        self.spawn(move || {
            let result = ai_context::preview(Path::new(&path), preview_kind).map(|p| (p.id, p.files));
            super::Msg::AiPreview { tab, kind: kind as u8, result }
        });
    }

    pub(crate) fn ai_preview(&mut self, tab: u64, kind: u8, result: Result<(Option<u32>, Vec<String>), String>) {
        if self.ai_reads.is_empty() {
            self.ai_label = None;
        }
        let kind = match kind {
            0 => Reviewed::Commit,
            1 => Reviewed::Stash,
            _ => Reviewed::Compose,
        };
        let (id, files) = match result {
            Ok(r) => r,
            Err(e) => return self.toast(false, e),
        };
        if id.is_none() {
            return self.ai_send(tab, kind, None);
        }
        let text = files.iter().map(|f| format!("\"{}\"", f.replace('"', "\\\""))).collect::<Vec<_>>().join("\n");
        self.dialog = Some(Dialog::new("Send these untracked files to AI?", super::dialogs::Body::Text(text)).choice("Send and continue", Kind::Primary, move |app, _| app.ai_send(tab, kind, id)));
    }

    fn ai_send(&mut self, tab: u64, kind: Reviewed, preview: Option<u32>) {
        let Some(path) = self.repo(tab).map(|r| r.path.clone()) else { return };
        match kind {
            Reviewed::Commit => {
                let before = self.draft(&path);
                self.ai_run("Writing a commit message…", move || {
                    let dir = Path::new(&path);
                    let files = ai_context::take_preview(dir, PreviewKind::Commit, preview)?;
                    let (summary, description) = ai::commit_message(dir, None, &files.text)?;
                    Ok(AiOutput::CommitMessage { path, before, summary, description })
                });
            }
            Reviewed::Stash => {
                let before = self.repo(tab).map(|r| r.work.stash_message.clone()).unwrap_or_default();
                self.ai_run("Naming the stash…", move || {
                    let dir = Path::new(&path);
                    let files = ai_context::take_preview(dir, PreviewKind::Stash, preview)?;
                    Ok(AiOutput::StashTitle { tab, before, text: ai::stash_message(dir, &files.text)? })
                })
            }
            Reviewed::Compose => self.ai_run("Grouping your changes into commits…", move || {
                let dir = Path::new(&path);
                let files = ai_context::take_preview(dir, PreviewKind::Compose, preview)?;
                Ok(AiOutput::Compose { tab, commits: ai::compose(dir, files)? })
            }),
        }
    }

    pub(crate) fn ai_commit_message(&mut self, tab: u64) {
        self.with_reviewed(tab, Reviewed::Commit);
    }

    pub(crate) fn ai_stash_title(&mut self, tab: u64) {
        self.with_reviewed(tab, Reviewed::Stash);
    }

    pub(crate) fn compose_commits(&mut self, tab: u64) {
        self.with_reviewed(tab, Reviewed::Compose);
    }

    pub(crate) fn explain_commit(&mut self, tab: u64, sha: &str, summary: &str) {
        let Some(path) = self.repo(tab).map(|r| r.path.clone()) else { return };
        let (sha, title) = (sha.to_owned(), format!("Explain: {summary}"));
        self.ai_run("Explaining the commit…", move || Ok(AiOutput::Explain { title, text: ai::explain_commit(Path::new(&path), &sha)? }));
    }

    pub(crate) fn explain_branch(&mut self, tab: u64, branch: &str) {
        let Some(path) = self.repo(tab).map(|r| r.path.clone()) else { return };
        let (b, title) = (branch.to_owned(), format!("What {branch} changes"));
        self.ai_run(&format!("Explaining {branch}…"), move || {
            let dir = Path::new(&path);
            let base = gitcraft_core::extras::default_target(dir).ok_or("No branch to compare with")?;
            Ok(AiOutput::Explain { title, text: ai::explain_branch(dir, &b, &base)? })
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn ai_resolve_conflict(&mut self, tab: u64, base: String, ours: String, theirs: String, file: String, editor: u64, revision: u64) {
        self.ai_run("Merging both sides…", move || Ok(AiOutput::Merge { tab, editor, revision, text: ai::resolve_conflict(&base, &ours, &theirs, &file)?, file }));
        if let Some(r) = self.repo_mut(tab) {
            r.conflict_ui.ai_busy = true;
        }
    }

    pub(crate) fn ai_pull_request(&mut self, tab: u64, request: u64, source: String, target: String) {
        let Some(path) = self.repo(tab).map(|r| r.path.clone()) else { return };
        self.ai_run("Writing the pull request…", move || {
            let (title, body) = ai::pull_request_text(Path::new(&path), &source, &target)?;
            Ok(AiOutput::PullText { tab, request, title, body })
        });
    }
}
