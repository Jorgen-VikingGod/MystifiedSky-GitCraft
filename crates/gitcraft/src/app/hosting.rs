//! Pull requests, issues and checks from the hosting services: the PULL REQUESTS and ISSUES
//! sections of the left panel, the pull request and issue views, CI checks, starting a pull
//! request, and the Launchpad across every connected service.

use super::widgets::Tip;
use super::App;
use super::ctxmenu::{self, Later, item};
use super::dialogs::{Dialog, Field, FieldKind};
use super::icons::{self, Icon};
use super::repo::{Loaded, RepoTab};
use super::theme::{BAD, GOOD, p};
use super::widgets::{self, Kind, REMOVED};
use egui::{Align2, CornerRadius, FontId, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};
use gitcraft_core::actions::Action;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

/// A list read page by page.
#[derive(Default, Clone)]
pub struct List {
    pub items: Vec<Value>,
    pub cursor: Option<String>,
    pub loading: bool,
    pub error: Option<String>,
    pub started: bool,
    /// Counts the requests, so only the latest one's answer changes the list.
    pub request: u64,
}

/// What the hosting service holds for one repository tab.
#[derive(Default)]
pub struct Hosted {
    pub remote: Option<Loaded<Option<Value>>>,
    pub prs: List,
    pub filter: String,
    pub issues: List,
    pub pr: HashMap<u64, Loaded<Value>>,
    pub comments: HashMap<String, List>,
    pub checks: HashMap<String, Loaded<Vec<Value>>>,
    pub review: HashMap<u64, Loaded<Value>>,
    /// The source commit each review's patch was read for: a pull request that moved on needs a new read.
    pub review_sha: HashMap<u64, String>,
    /// Recently merged pull requests (number, source branch, source commit, URL), for the Merged pills.
    pub merged: Option<Loaded<Vec<(u64, String, String, String)>>>,
}

/// The pull request view's own state.
#[derive(Default)]
pub struct PrUi {
    pub number: u64,
    pub comment: String,
    pub method: String,
    pub squash: String,
    pub busy: bool,
    pub merged: Option<(u64, String, String)>,
    pub remote_deleted: bool,
    pub review_open: bool,
    pub review_text: String,
    pub reviewers: String,
    pub review_note: Option<(bool, String)>,
    /// File review: the changed file shown, the line picked for a comment and the comment.
    pub review_file: String,
    pub review_line: Option<ReviewLine>,
    pub inline_text: String,
}

/// A line of a changed file's patch, as a review comment addresses it.
#[derive(Clone, Debug, PartialEq)]
pub struct ReviewLine {
    /// "LEFT" (the old version) or "RIGHT" (the new one).
    pub side: &'static str,
    pub number: u64,
    pub text: String,
    /// A context line's number in the old version.
    pub previous: Option<u64>,
}

/// The commentable lines of a unified patch: removed lines on the left, added and context lines
/// on the right.
pub fn review_lines(patch: &str) -> Vec<ReviewLine> {
    let mut out = Vec::new();
    let (mut left, mut right) = (0u64, 0u64);
    for text in patch.split('\n') {
        if let Some(rest) = text.strip_prefix("@@ -") {
            let mut parts = rest.split(' ');
            let num = |p: Option<&str>| p.and_then(|p| p.trim_start_matches(['-', '+']).split(',').next()).and_then(|n| n.parse::<u64>().ok());
            if let (Some(l), Some(r)) = (num(parts.next()), num(parts.next())) {
                (left, right) = (l, r);
            }
        } else if text.starts_with('+') {
            out.push(ReviewLine { side: "RIGHT", number: right, text: text.to_owned(), previous: None });
            right += 1;
        } else if text.starts_with('-') {
            out.push(ReviewLine { side: "LEFT", number: left, text: text.to_owned(), previous: None });
            left += 1;
        } else if text.starts_with(' ') {
            out.push(ReviewLine { side: "RIGHT", number: right, text: text.to_owned(), previous: Some(left) });
            right += 1;
            left += 1;
        }
    }
    out
}

/// What a review request was sent from: the review message or the reviewers field.
pub enum Sent {
    Message(String),
    Reviewers(String),
    /// An inline comment: its text, and the line it was on.
    Inline(String, ReviewLine),
}

#[derive(Default)]
pub struct Launchpad {
    pub statuses: Option<Loaded<Vec<gitcraft_core::hosting::ServiceStatus>>>,
    pub pages: HashMap<String, List>,
}

const LAUNCHPAD_SERVICES: [&str; 10] = ["github", "github-enterprise", "gitlab", "gitlab-self", "bitbucket", "bitbucket-server", "azure", "jira", "jira-server", "trello"];

fn s(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(x)) => x.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn iso_age(iso: &str) -> (String, egui::Color32) {
    match chrono::DateTime::parse_from_rfc3339(iso) {
        Ok(t) => super::history::time_since(t.timestamp(), chrono::Local::now().timestamp()),
        Err(_) => (String::new(), p().fg3),
    }
}

pub fn issue_identity(i: &Value) -> String {
    let key = s(i, "key");
    serde_json::json!([s(i, "tracker"), s(i, "repo"), if key.is_empty() { Value::from(i["number"].as_u64().unwrap_or(0)) } else { Value::String(key) }]).to_string()
}

impl App {
    fn gitlab(&self) -> String {
        self.settings.string("gitlabHost", "")
    }

    /// Read the repository's recently merged pull requests once, for the Merged pills.
    pub(crate) fn want_merged_prs(&mut self, tab: u64) {
        let Some(path) = self.repo(tab).map(|r| r.path.clone()) else { return };
        let gitlab = self.gitlab();
        let h = self.hosted_mut(tab);
        if h.merged.is_some() {
            return;
        }
        h.merged = Some(Loaded::Loading);
        self.spawn(move || {
            let r = gitcraft_core::session::remote_of(&path, &gitlab).and_then(|remote| gitcraft_core::hosting::pull_requests_page(&remote, None, gitcraft_core::hosting::PullFilter::Merged));
            super::Msg::Apply(Box::new(move |app: &mut App| {
                app.hosted_mut(tab).merged = Some(match r {
                    Ok(page) => Loaded::Ready(page.items.into_iter().map(|pr| (pr.number, pr.source, pr.source_sha, pr.url)).collect()),
                    Err(e) => Loaded::Failed(e),
                });
            }))
        });
    }

    /// The merged pull request whose source was `branch` or ended at commit `sha`: its number and URL. Asks for the
    /// list the first time.
    pub(crate) fn merged_pr(&self, tab: u64, branch: Option<&str>, sha: Option<&str>, later: &mut Vec<Later>) -> Option<(u64, String)> {
        match self.hosted.get(&tab).and_then(|h| h.merged.as_ref()) {
            None => {
                later.push(Box::new(move |app| app.want_merged_prs(tab)));
                None
            }
            Some(Loaded::Ready(list)) => list
                .iter()
                .find(|(_, source, source_sha, _)| branch.is_some_and(|b| !b.is_empty() && source == b) || sha.is_some_and(|s| !s.is_empty() && source_sha == s))
                .map(|(n, _, _, url)| (*n, url.clone())),
            _ => None,
        }
    }

    /// A "Merged #N" pill for `branch` (or commit `sha`) when its pull request was merged; it opens the pull request.
    pub(crate) fn merged_pill(&self, ui: &mut Ui, tab: u64, branch: Option<&str>, sha: Option<&str>, later: &mut Vec<Later>) {
        let Some((number, url)) = self.merged_pr(tab, branch, sha, later) else { return };
        let purple = egui::Color32::from_rgb(0x8f, 0x6f, 0xe0);
        let resp = egui::Frame::new().fill(purple.gamma_multiply(0.25)).stroke(Stroke::new(1.0, purple)).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::symmetric(6, 1)).show(ui, |ui| {
            ui.label(egui::RichText::new(format!("Merged #{number}")).size(11.0).color(egui::Color32::WHITE));
        }).response.interact(Sense::click());
        if resp.tip("Its pull request was merged. Click to open it.").clicked() {
            later.push(Box::new(move |app| app.open_url(&url)));
        }
    }

    fn hosted_mut(&mut self, tab: u64) -> &mut Hosted {
        self.hosted.entry(tab).or_default()
    }

    /// A service was connected or disconnected: read the hosted lists again.
    pub(crate) fn hosting_changed(&mut self) {
        self.hosted.clear();
        self.reset_launchpad();
    }

    fn want_remote(&mut self, tab: u64) {
        let Some(path) = self.repo(tab).map(|r| r.path.clone()) else { return };
        let gitlab = self.gitlab();
        let h = self.hosted_mut(tab);
        if h.remote.is_some() {
            return;
        }
        h.remote = Some(Loaded::Loading);
        self.spawn(move || {
            let r = gitcraft_core::session::remote_of(&path, &gitlab).ok().and_then(|r| serde_json::to_value(r).ok());
            super::Msg::Apply(Box::new(move |app: &mut App| app.hosted_mut(tab).remote = Some(Loaded::Ready(r))))
        });
    }

    fn want_prs(&mut self, tab: u64, more: bool) {
        let Some(path) = self.repo(tab).map(|r| r.path.clone()) else { return };
        let gitlab = self.gitlab();
        let h = self.hosted_mut(tab);
        if h.prs.loading || (h.prs.started && !more) {
            return;
        }
        h.prs.loading = true;
        h.prs.started = true;
        h.prs.request += 1;
        let request = h.prs.request;
        let cursor = if more { h.prs.cursor.clone() } else { None };
        let filter = if h.filter.is_empty() { "open".to_owned() } else { h.filter.clone() };
        let wanted = filter.clone();
        self.spawn(move || {
            let r = gitcraft_core::session::remote_of(&path, &gitlab).and_then(|remote| {
                let f: gitcraft_core::hosting::PullFilter = serde_json::from_value(Value::String(filter)).unwrap_or_default();
                gitcraft_core::hosting::pull_requests_page(&remote, cursor.as_deref(), f)
            });
            super::Msg::Apply(Box::new(move |app: &mut App| {
                let h = app.hosted_mut(tab);
                if (if h.filter.is_empty() { "open" } else { h.filter.as_str() }) != wanted || h.prs.request != request {
                    return;
                }
                h.prs.loading = false;
                match r {
                    Ok(page) => {
                        for pr in page.items {
                            let v = serde_json::to_value(pr).unwrap_or_default();
                            if !h.prs.items.iter().any(|x| x["number"] == v["number"]) {
                                h.prs.items.push(v);
                            }
                        }
                        h.prs.cursor = page.cursor;
                        h.prs.error = None;
                    }
                    Err(e) => h.prs.error = Some(e),
                }
            }))
        });
    }

    fn want_issues(&mut self, tab: u64, more: bool) {
        let Some(path) = self.repo(tab).map(|r| r.path.clone()) else { return };
        let gitlab = self.gitlab();
        let tracker: Option<gitcraft_core::hosting::Tracker> = self.repo_pref(&path, "tracker").and_then(|v| serde_json::from_value(v).ok());
        let h = self.hosted_mut(tab);
        if h.issues.loading || (h.issues.started && !more) {
            return;
        }
        h.issues.loading = true;
        h.issues.started = true;
        let cursor = if more { h.issues.cursor.clone() } else { None };
        self.spawn(move || {
            let r = gitcraft_core::pagination::one_page(cursor, || match tracker.filter(|t| !t.kind.is_empty()) {
                Some(t) => gitcraft_core::hosting::tracker_issues(&t, false),
                None => gitcraft_core::hosting::issues(&gitcraft_core::session::remote_of(&path, &gitlab)?),
            });
            super::Msg::Apply(Box::new(move |app: &mut App| {
                let h = app.hosted_mut(tab);
                h.issues.loading = false;
                match r {
                    Ok(page) => {
                        for i in page.items {
                            let v = serde_json::to_value(i).unwrap_or_default();
                            if !h.issues.items.iter().any(|x| issue_identity(x) == issue_identity(&v)) {
                                h.issues.items.push(v);
                            }
                        }
                        h.issues.cursor = page.cursor;
                        h.issues.error = None;
                    }
                    Err(e) => h.issues.error = Some(e),
                }
            }))
        });
    }

    pub(crate) fn hosted_sections(&self, ui: &mut Ui, tab: u64, r: &RepoTab, open: &mut HashMap<String, bool>, filter: &str, later: &mut Vec<Later>) {
        let c = p();
        let h = self.hosted.get(&tab);
        if h.and_then(|h| h.remote.as_ref()).is_none() {
            later.push(Box::new(move |app| app.want_remote(tab)));
        }
        let remote = h.and_then(|h| match &h.remote {
            Some(Loaded::Ready(r)) => r.clone(),
            _ => None,
        });
        let on = remote.is_some();
        let provider = remote.as_ref().map(|r| s(r, "provider")).unwrap_or_default();
        let service = remote.as_ref().map(|r| s(r, "service")).unwrap_or_default();
        let service_label = match provider.as_str() {
            "gitlab" => "GitLab",
            "bitbucket" => "Bitbucket",
            "azure" => "Azure DevOps",
            "gitea" if service == "codeberg" => "Codeberg",
            "gitea" if service == "forgejo" => "Forgejo",
            "gitea" => "Gitea",
            _ => "GitHub",
        };
        let is_open = |k: &str, open: &HashMap<String, bool>| !filter.is_empty() || open.get(k).copied().unwrap_or(false);
        let empty = List::default();
        let prs = h.map_or(&empty, |h| &h.prs);
        let issues = h.map_or(&empty, |h| &h.issues);
        let head = r.view.as_ref().and_then(|v| v.data.head_branch.clone());
        // The panel's filter narrows the loaded rows too, as it does the other sections.
        let needle = filter.trim().to_lowercase();
        let matches = |text: String| needle.is_empty() || text.to_lowercase().contains(&needle);
        let note = |ui: &mut Ui, list: &List| {
            if !on {
                super::left::section_note(ui, "This repository's remote is outside the configured hosting services.");
            } else if list.loading && list.items.is_empty() {
                super::left::section_note(ui, "Loading…");
            } else if let Some(e) = &list.error {
                super::left::section_note(ui, e);
            }
        };
        let title = if provider == "gitlab" { "MERGE REQUESTS" } else { "PULL REQUESTS" };
        let mut action = |ui: &mut Ui| {
            if let Some(b) = head.clone() {
                later.push(Box::new(move |app| app.start_pull_request(tab, &b)));
            }
            let _ = ui;
        };
        let pr_action = (on && head.is_some()).then_some((Icon::Plus, "Start a pull request"));
        let (clicked, _) = super::left::section_head(ui, title, Icon::GitPullRequest, Some(prs.items.len()), is_open("prs", open), pr_action, &mut action);
        if clicked {
            let now = is_open("prs", open);
            open.insert("prs".into(), !now);
        }
        if is_open("prs", open) {
            if on && !prs.started {
                later.push(Box::new(move |app| app.want_prs(tab, false)));
            }
            if on {
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    let current = h.map(|h| h.filter.clone()).filter(|f| !f.is_empty()).unwrap_or_else(|| "open".into());
                    let options = [("open", "Open"), ("closed", "Closed"), ("merged", "Merged"), ("mine", "Created by me"), ("reviewRequested", "Review requested")];
                    let shown = options.iter().find(|(v, _)| *v == current).map_or("Open", |(_, l)| *l);
                    let mut chosen = None;
                    egui::ComboBox::from_id_salt(("pr_filter", tab)).selected_text(shown).width(ui.available_width() - 24.0).show_ui(ui, |ui| {
                        for (v, l) in options {
                            if ui.selectable_label(v == current, l).clicked() {
                                chosen = Some(v.to_owned());
                            }
                        }
                    });
                    if let Some(f) = chosen {
                        later.push(Box::new(move |app| {
                            let h = app.hosted_mut(tab);
                            h.filter = f;
                            h.prs = List::default();
                            app.want_prs(tab, false);
                        }));
                    }
                });
                ui.add_space(4.0);
            }
            note(ui, prs);
            for pr in prs.items.iter().filter(|pr| matches(format!("{} #{} {} {} {}", s(pr, "title"), pr["number"].as_u64().unwrap_or(0), s(pr, "author"), s(pr, "source"), s(pr, "target")))) {
                let number = pr["number"].as_u64().unwrap_or(0);
                let draft = pr["draft"].as_bool().unwrap_or(false);
                let label = s(pr, "title");
                let resp = super::left::section_row(ui, 1, Icon::GitPullRequest, &label, Some(&format!("#{number}")), Some(if draft { c.fg3 } else { GOOD })).tip(format!("{} → {}\nby {}", s(pr, "source"), s(pr, "target"), s(pr, "author")));
                if resp.clicked() {
                    later.push(Box::new(move |app| app.open_pr(tab, number)));
                }
                let url = s(pr, "url");
                let sl = service_label;
                ctxmenu::attach(&resp, later, || vec![item("View pull request", move |app| app.open_pr(tab, number)), item(format!("View on {sl}"), move |app| app.open_url(&url))]);
            }
            if prs.cursor.is_some() {
                ui.horizontal(|ui| {
                    ui.add_space(30.0);
                    if widgets::link(ui, if prs.loading { "Loading more…" } else { "Load more pull requests" }, 12.0).clicked() {
                        later.push(Box::new(move |app| app.want_prs(tab, true)));
                    }
                });
            }
            if on && prs.started && !prs.loading && prs.error.is_none() && prs.items.is_empty() {
                super::left::section_note(ui, if prs.cursor.is_some() { "No matching pull requests on the loaded pages." } else { "No matching pull requests." });
            }
        }
        let (clicked, _) = super::left::section_head(ui, "ISSUES", Icon::CircleDot, Some(issues.items.len()), is_open("issues", open), None, &mut |_| {});
        if clicked {
            let now = is_open("issues", open);
            open.insert("issues".into(), !now);
        }
        if is_open("issues", open) {
            if on && !issues.started {
                later.push(Box::new(move |app| app.want_issues(tab, false)));
            }
            note(ui, issues);
            for i in issues.items.iter().filter(|i| matches(format!("{} #{} {} {}", s(i, "title"), i["number"].as_u64().unwrap_or(0), s(i, "key"), i["labels"].as_array().map(|l| l.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")).unwrap_or_default()))) {
                let key = s(i, "key");
                let label = if key.is_empty() { format!("#{}", i["number"].as_u64().unwrap_or(0)) } else { key };
                let resp = super::left::section_row(ui, 1, Icon::CircleDot, &s(i, "title"), Some(&label), Some(GOOD)).tip(i["labels"].as_array().map(|l| l.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default());
                let id = issue_identity(i);
                if resp.clicked() {
                    let id = id.clone();
                    later.push(Box::new(move |app| app.open_issue(tab, &id)));
                }
                let (url, issue) = (s(i, "url"), i.clone());
                let sl = service_label;
                ctxmenu::attach(&resp, later, || {
                    let id2 = id.clone();
                    let (number, title) = (if s(&issue, "key").is_empty() { issue["number"].as_u64().unwrap_or(0).to_string() } else { s(&issue, "key") }, s(&issue, "title"));
                    vec![
                        item("View issue", move |app| app.open_issue(tab, &id2)),
                        item("Start an Agent Session", move |app| app.start_agent_for_issue(tab, &number, &title)),
                        item("Create branch for this issue", move |app| app.branch_from_issue(tab, &issue)),
                        item(format!("View issue in {sl}"), move |app| app.open_url(&url)),
                    ]
                });
            }
            if issues.cursor.is_some() {
                ui.horizontal(|ui| {
                    ui.add_space(30.0);
                    if widgets::link(ui, if issues.loading { "Loading more…" } else { "Load more issues" }, 12.0).clicked() {
                        later.push(Box::new(move |app| app.want_issues(tab, true)));
                    }
                });
            }
            if on && issues.started && !issues.loading && issues.error.is_none() && issues.items.is_empty() {
                super::left::section_note(ui, "No open issues.");
            }
        }
    }

    pub(crate) fn open_pr(&mut self, tab: u64, number: u64) {
        if let Some(r) = self.repo_mut(tab) {
            r.ui.pr = Some(number);
            r.ui.issue = None;
            r.ui.diff = None;
            r.ui.history = None;
        }
    }

    pub(crate) fn open_issue(&mut self, tab: u64, id: &str) {
        if let Some(r) = self.repo_mut(tab) {
            r.ui.issue = Some(id.to_owned());
            r.ui.pr = None;
            r.ui.diff = None;
            r.ui.history = None;
        }
    }

    pub(crate) fn branch_from_issue(&mut self, tab: u64, issue: &Value) {
        let key = s(issue, "key");
        let title = s(issue, "title");
        let number = issue["number"].as_u64().unwrap_or(0);
        let name = if key.is_empty() {
            gitcraft_core::hosting::branch_for_issue(number, &title)
        } else {
            let slug: String = title.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
            let slug: String = slug.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-").chars().take(45).collect();
            format!("{}-{slug}", key.to_lowercase())
        };
        let label = if key.is_empty() { format!("#{number}") } else { key };
        let mut d = Dialog::prompt(&format!("Create a branch for {label}"), "Branch name", &name, "Create and Checkout", move |app, v| app.act(tab, Action::CreateBranch { name: v.text("value").trim().to_owned(), at: None, checkout: true }));
        d.check = Some(Box::new(|v| v.text("value").trim().is_empty().then(String::new)));
        self.dialog = Some(d);
    }

    fn want_pr(&mut self, tab: u64, number: u64, again: bool) {
        let Some(path) = self.repo(tab).map(|r| r.path.clone()) else { return };
        let gitlab = self.gitlab();
        let h = self.hosted_mut(tab);
        if h.pr.contains_key(&number) && !again {
            return;
        }
        if !h.pr.contains_key(&number) {
            h.pr.insert(number, Loaded::Loading);
        }
        self.spawn(move || {
            let r = gitcraft_core::session::remote_of(&path, &gitlab).and_then(|remote| gitcraft_core::hosting::pull_request_details(&remote, number)).map(|d| serde_json::to_value(d).unwrap_or_default());
            super::Msg::Apply(Box::new(move |app: &mut App| {
                app.hosted_mut(tab).pr.insert(number, match r {
                    Ok(v) => Loaded::Ready(v),
                    Err(e) => Loaded::Failed(e),
                });
            }))
        });
    }

    fn want_comments(&mut self, tab: u64, key: String, more: bool, issue: bool, number: u64) {
        let Some(path) = self.repo(tab).map(|r| r.path.clone()) else { return };
        let gitlab = self.gitlab();
        let list = self.hosted_mut(tab).comments.entry(key.clone()).or_default();
        if list.loading || (list.started && !more) {
            return;
        }
        list.loading = true;
        list.started = true;
        let cursor = if more { list.cursor.clone() } else { None };
        self.spawn(move || {
            let r = gitcraft_core::session::remote_of(&path, &gitlab).and_then(|remote| gitcraft_core::pagination::one_page(cursor, || if issue { gitcraft_core::hosting::issue_comments(&remote, number) } else { gitcraft_core::hosting::comments(&remote, number) }));
            super::Msg::Apply(Box::new(move |app: &mut App| {
                let list = app.hosted_mut(tab).comments.entry(key).or_default();
                list.loading = false;
                match r {
                    Ok(page) => {
                        list.items.extend(page.items.into_iter().map(|c| serde_json::to_value(c).unwrap_or_default()));
                        list.cursor = page.cursor;
                        list.error = None;
                    }
                    Err(e) => list.error = Some(e),
                }
            }))
        });
    }

    /// CI checks for a commit (folded until opened).
    pub(crate) fn commit_checks(&self, ui: &mut Ui, tab: u64, _path: &str, sha: &str, later: &mut Vec<Later>) {
        self.checks_ui(ui, tab, sha, None, None, later);
    }

    fn checks_ui(&self, ui: &mut Ui, tab: u64, sha: &str, number: Option<u64>, remote: Option<String>, later: &mut Vec<Later>) {
        let c = p();
        let key = format!("{sha}|{number:?}|{remote:?}");
        let open_id = ui.scope_id().with(("checks_open", &key));
        let mut open: bool = ui.data(|d| d.get_temp(open_id)).unwrap_or(false);
        let state = self.hosted.get(&tab).and_then(|h| h.checks.get(&key));
        let count = match state {
            Some(Loaded::Ready(l)) if !l.is_empty() => format!(" ({})", l.len()),
            _ => String::new(),
        };
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let (r, resp) = ui.allocate_exact_size(vec2(12.0, 16.0), Sense::click());
            widgets::chevron(ui, r, open, 12.0, c.fg2);
            let l = ui.add(egui::Label::new(egui::RichText::new(format!("Checks{count}")).size(12.0).color(c.fg2)).sense(Sense::click()));
            if resp.clicked() || l.clicked() {
                open = !open;
            }
        });
        ui.data_mut(|d| d.insert_temp(open_id, open));
        if !open {
            return;
        }
        match state {
            None => {
                let (k, sha, remote) = (key.clone(), sha.to_owned(), remote.clone());
                later.push(Box::new(move |app| app.want_checks(tab, k, sha, number, remote)));
                ui.label(egui::RichText::new("Loading checks…").size(12.0).color(c.fg2));
            }
            Some(Loaded::Loading) => {
                ui.label(egui::RichText::new("Loading checks…").size(12.0).color(c.fg2));
            }
            Some(Loaded::Failed(e)) => {
                ui.add(egui::Label::new(egui::RichText::new(e).size(12.0).color(c.fg2)).wrap());
            }
            Some(Loaded::Ready(list)) => {
                if list.is_empty() {
                    ui.label(egui::RichText::new("No checks were reported.").size(12.0).color(c.fg2));
                }
                for check in list {
                    ui.horizontal(|ui| {
                        let state = s(check, "state");
                        let color = match state.as_str() {
                            "passed" => GOOD,
                            "failed" => BAD,
                            _ => c.fg3,
                        };
                        ui.label(egui::RichText::new(&state).size(12.0).color(color));
                        let url = s(check, "url");
                        let name = s(check, "name");
                        let ok_url = url::Url::parse(&url).ok().filter(|u| u.scheme() == "https" && u.username().is_empty() && u.password().is_none());
                        if let Some(u) = ok_url {
                            if widgets::link(ui, &name, 12.0).clicked() {
                                let u = u.to_string();
                                later.push(Box::new(move |app| app.open_url(&u)));
                            }
                        } else {
                            ui.label(egui::RichText::new(&name).size(12.0).color(c.fg2));
                        }
                    });
                }
            }
        }
        if !matches!(state, None | Some(Loaded::Loading)) && widgets::link(ui, "Refresh checks", 12.0).clicked() {
            let k = key.clone();
            later.push(Box::new(move |app| {
                app.hosted_mut(tab).checks.remove(&k);
            }));
        }
    }

    fn want_checks(&mut self, tab: u64, key: String, sha: String, number: Option<u64>, remote_name: Option<String>) {
        let Some(path) = self.repo(tab).map(|r| r.path.clone()) else { return };
        let gitlab = self.gitlab();
        let h = self.hosted_mut(tab);
        if h.checks.contains_key(&key) {
            return;
        }
        h.checks.insert(key.clone(), Loaded::Loading);
        self.spawn(move || {
            let r = (|| {
                let remote = match remote_name {
                    Some(name) => {
                        let o = gitcraft_core::git::run(Path::new(&path), &["remote", "get-url", "--", &name]);
                        if !o.ok {
                            return Err(o.message());
                        }
                        gitcraft_core::hosting::parse_remote(o.stdout.trim(), &gitlab).ok_or("This branch's remote is outside the configured hosting services.")?
                    }
                    None => gitcraft_core::session::remote_of(&path, &gitlab)?,
                };
                gitcraft_core::hosting::commit_checks(&remote, &sha, number)
            })()
            .map(|l| l.into_iter().map(|c| serde_json::to_value(c).unwrap_or_default()).collect::<Vec<_>>());
            super::Msg::Apply(Box::new(move |app: &mut App| {
                app.hosted_mut(tab).checks.insert(key, match r {
                    Ok(l) => Loaded::Ready(l),
                    Err(e) => Loaded::Failed(e),
                });
            }))
        });
    }

    pub(crate) fn pr_view(&self, ui: &mut Ui, tab: u64, r: &mut RepoTab, number: u64, later: &mut Vec<Later>) {
        let c = p();
        if r.pr_ui.number != number {
            r.pr_ui = PrUi { number, ..Default::default() };
        }
        let h = self.hosted.get(&tab);
        let pr = match h.and_then(|h| h.pr.get(&number)) {
            Some(Loaded::Ready(v)) => Some(v.clone()),
            None => {
                later.push(Box::new(move |app| app.want_pr(tab, number, false)));
                None
            }
            _ => None,
        };
        if h.and_then(|h| h.remote.as_ref()).is_none() {
            later.push(Box::new(move |app| app.want_remote(tab)));
        }
        let service = h.and_then(|h| match &h.remote {
            Some(Loaded::Ready(Some(r))) => Some(s(r, "service")),
            _ => None,
        }).unwrap_or_default();
        let w = ui.available_width();
        let (bar, _) = ui.allocate_exact_size(vec2(w, 32.0), Sense::hover());
        ui.painter().rect_filled(bar, CornerRadius::ZERO, c.toolbar.gamma_multiply(0.6));
        ui.painter().line_segment([bar.left_bottom(), bar.right_bottom()], Stroke::new(1.0, c.black(0.3)));
        icons::paint(ui, Rect::from_min_size(pos2(bar.left() + 12.0, bar.center().y - 7.0), Vec2::splat(14.0)), Icon::GitPullRequest, 14.0, GOOD);
        let title = format!("#{number} {}", pr.as_ref().map(|p| s(p, "title")).unwrap_or_default());
        let g = widgets::fit(ui, &title, super::fonts::semibold(13.0), c.white(1.0), w - 110.0);
        ui.painter().galley(pos2(bar.left() + 34.0, bar.center().y - g.size().y / 2.0), g, c.white(1.0));
        let mut close = false;
        let mut right = ui.new_child(egui::UiBuilder::new().max_rect(bar.shrink2(vec2(8.0, 0.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
        if widgets::icon_button(&mut right, Icon::X, 16.0, "Close (Esc)").clicked() {
            close = true;
        }
        if let Some(prv) = &pr
            && widgets::icon_button(&mut right, Icon::ExternalLink, 14.0, "Open in browser").clicked()
        {
            let u = s(prv, "url");
            later.push(Box::new(move |app| app.open_url(&u)));
        }
        if !ui.ctx().egui_wants_keyboard_input() && !super::dialog_open(ui.ctx()) && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        if close {
            r.ui.pr = None;
            return;
        }
        let Some(pr) = pr else {
            let text = match h.and_then(|h| h.pr.get(&number)) {
                Some(Loaded::Failed(e)) => e.clone(),
                _ => "Loading…".into(),
            };
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                ui.label(egui::RichText::new(text).size(13.0).color(c.fg3));
            });
            return;
        };
        let v = r.view.as_ref().expect("view");
        let pu = &mut r.pr_ui;
        let methods: Vec<String> = pr["merge_methods"].as_array().map(|a| a.iter().filter_map(|m| m.as_str().map(str::to_owned)).collect()).filter(|m: &Vec<String>| !m.is_empty()).unwrap_or_else(|| if service == "bitbucket-server" { vec!["default".into()] } else { Vec::new() });
        let selected = if methods.contains(&pu.method) { pu.method.clone() } else { methods.first().cloned().unwrap_or_default() };
        let method_label = |m: &str| match m {
            "default" => "Server default",
            "unsquashed" => "Merge without squashing",
            "merge" => "Merge commit",
            "squash" => "Squash and merge",
            "rebase" => "Rebase and merge",
            "fast_forward" => "Fast-forward",
            "rebase_merge" => "Rebase with merge commit",
            other => if other.is_empty() { "" } else { "Merge" },
        };
        let draft = pr["draft"].as_bool().unwrap_or(false);
        let merge_status = pr["merge_status"].as_str().map(str::to_owned).or_else(|| draft.then(|| "Draft pull requests cannot be merged.".into()));
        let source_sha = s(&pr, "source_sha");
        let (source, target) = (s(&pr, "source"), s(&pr, "target"));
        let state = s(&pr, "state").to_lowercase();
        let merged_source = match &pu.merged {
            Some((n, src, sha)) if *n == number => Some((src.clone(), sha.clone())),
            _ if !source_sha.is_empty() && (state == "merged" || state == "completed") => Some((source.clone(), source_sha.clone())),
            _ => None,
        };
        let local_source = merged_source.as_ref().is_some_and(|(src, sha)| v.data.head_branch.as_ref() != Some(src) && v.data.refs.iter().any(|x| x.kind == gitcraft_core::git::RefKind::Local && &x.name == src && &x.sha == sha));
        let url = s(&pr, "url");
        egui::ScrollArea::vertical().id_salt(("pr", tab, number)).auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::new().inner_margin(egui::Margin::same(16)).show(ui, |ui| {
                ui.set_width(w - 32.0);
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                ui.horizontal_wrapped(|ui| {
                    widgets::avatar(ui, 20.0, &s(&pr, "author"), &s(&pr, "author"));
                    ui.label(egui::RichText::new(s(&pr, "author")).size(12.5).color(c.white(1.0)));
                    ui.label(egui::RichText::new("wants to merge").size(12.5).color(c.fg2));
                    let pill = |ui: &mut Ui, t: &str| {
                        egui::Frame::new().fill(c.white(0.1)).corner_radius(CornerRadius::same(3)).inner_margin(egui::Margin::symmetric(6, 1)).show(ui, |ui| {
                            ui.label(egui::RichText::new(t).monospace().size(12.0).color(c.white(1.0)));
                        });
                    };
                    pill(ui, &source);
                    ui.label(egui::RichText::new("into").size(12.5).color(c.fg2));
                    pill(ui, &target);
                    if draft {
                        pill(ui, "Draft");
                    }
                    let (age, color) = iso_age(&s(&pr, "updated"));
                    ui.label(egui::RichText::new("· updated").size(12.5).color(c.fg2));
                    ui.label(egui::RichText::new(age).size(12.5).color(color));
                });
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    if widgets::small(ui, Kind::Default, &format!("Checkout {source}")).tip("Check out this pull request’s exact source commit in detached HEAD mode").clicked() && !pu.busy {
                        later.push(Box::new(move |app| app.checkout_pr(tab, number)));
                    }
                    if merged_source.is_none() && !methods.is_empty() {
                        egui::ComboBox::from_id_salt(("merge_method", number)).selected_text(method_label(&selected)).show_ui(ui, |ui| {
                            for m in &methods {
                                if ui.selectable_label(*m == selected, method_label(m)).clicked() {
                                    pu.method = m.clone();
                                }
                            }
                        });
                    }
                    if merged_source.is_none() {
                        let enabled = !pu.busy && merge_status.is_none() && !selected.is_empty();
                        if widgets::button(ui, Kind::Success, None, "Merge Pull Request", 24.0, enabled).tip(merge_status.clone().unwrap_or_default()).clicked() {
                            let (m, sha, squash, src, tgt) = (selected.clone(), source_sha.clone(), pu.squash.trim().to_owned(), source.clone(), target.clone());
                            let label = if m == "default" { "server default strategy".to_owned() } else { method_label(&m).to_owned() };
                            later.push(Box::new(move |app| {
                                app.dialog = Some(Dialog::confirm_with(&format!("Merge #{number}?"), &format!("This merges {src} into {tgt} on the server ({label})."), "Merge", true, move |app, _| app.merge_pr(tab, number, m, sha, squash, src)));
                            }));
                        }
                    }
                    if widgets::button(ui, Kind::Default, None, "Refresh merge status", 24.0, !pu.busy).clicked() {
                        later.push(Box::new(move |app| app.want_pr(tab, number, true)));
                    }
                    if let Some((src, sha)) = &merged_source {
                        if !pu.remote_deleted && widgets::button(ui, Kind::Default, None, "Delete remote source branch", 24.0, !pu.busy).clicked() {
                            let (src, sha) = (src.clone(), sha.clone());
                            later.push(Box::new(move |app| app.delete_merged_source(tab, number, src, sha, true)));
                        }
                        if local_source && widgets::button(ui, Kind::Default, None, "Delete local source branch", 24.0, !pu.busy).clicked() {
                            let (src, sha) = (src.clone(), sha.clone());
                            later.push(Box::new(move |app| app.delete_merged_source(tab, number, src, sha, false)));
                        }
                    }
                });
                if merged_source.is_none() && selected == "squash" && merge_status.is_none() {
                    ui.label(egui::RichText::new("Squash commit message").size(12.5).color(c.fg2));
                    egui::Frame::new().fill(c.field).stroke(Stroke::new(1.0, c.line)).corner_radius(CornerRadius::same(3)).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
                        ui.add(egui::TextEdit::multiline(&mut pu.squash).frame(egui::Frame::NONE).hint_text("Leave blank to use the server default").desired_rows(2).desired_width(f32::INFINITY));
                    });
                }
                if merged_source.is_some() {
                    ui.label(egui::RichText::new("Merged").size(12.5).color(GOOD));
                } else if merge_status.is_some() || selected.is_empty() {
                    ui.label(egui::RichText::new(merge_status.clone().unwrap_or_else(|| "The hosting service did not provide an available merge method. Check it in the browser.".into())).size(12.5).color(c.fg2));
                }
                ui.add_space(8.0);
                egui::Frame::new().fill(c.panel).stroke(Stroke::new(1.0, c.white(0.1))).corner_radius(CornerRadius::same(4)).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
                    ui.set_width(w - 56.0);
                    let body = s(&pr, "body");
                    if body.trim().is_empty() {
                        ui.label(egui::RichText::new("No description.").size(13.0).color(c.fg3));
                    } else if let Some(link) = super::markdown::show(ui, &body, &url) {
                        later.push(Box::new(move |app| app.open_url(&link)));
                    }
                });
                if !source_sha.is_empty() {
                    self.checks_ui(ui, tab, &source_sha, Some(number), None, later);
                    if ["github", "github-enterprise", "gitlab", "gitlab-self"].contains(&service.as_str()) {
                        self.review_ui(ui, tab, number, &pr, pu, later);
                    }
                }
                ui.add_space(12.0);
                ui.label(egui::RichText::new("COMMENTS").font(super::fonts::semibold(12.0)).color(c.fg2));
                self.comments_ui(ui, tab, number, false, &url, later);
                egui::Frame::new().fill(c.field).stroke(Stroke::new(1.0, c.white(0.1))).corner_radius(CornerRadius::same(3)).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
                    ui.add(egui::TextEdit::multiline(&mut pu.comment).frame(egui::Frame::NONE).hint_text("Leave a comment").desired_rows(4).desired_width(f32::INFINITY));
                });
                if widgets::button(ui, Kind::Primary, None, "Comment", 24.0, !pu.busy && !pu.comment.trim().is_empty()).clicked() {
                    let draft = pu.comment.clone();
                    pu.busy = true;
                    later.push(Box::new(move |app| app.send_pr_comment(tab, number, draft)));
                }
            });
        });
    }

    fn comments_ui(&self, ui: &mut Ui, tab: u64, number: u64, issue: bool, url: &str, later: &mut Vec<Later>) {
        let c = p();
        let key = format!("{}{number}", if issue { "i" } else { "p" });
        let list = self.hosted.get(&tab).and_then(|h| h.comments.get(&key));
        let Some(list) = list else {
            later.push(Box::new(move |app| app.want_comments(tab, key, false, issue, number)));
            ui.label(egui::RichText::new("Loading comments…").size(12.5).color(c.fg3));
            return;
        };
        if list.loading && list.items.is_empty() {
            ui.label(egui::RichText::new("Loading comments…").size(12.5).color(c.fg3));
        } else if list.items.is_empty() && list.error.is_none() {
            ui.label(egui::RichText::new("No comments yet.").size(12.5).color(c.fg3));
        }
        for cm in &list.items {
            ui.horizontal_top(|ui| {
                widgets::avatar(ui, 24.0, &s(cm, "author"), &s(cm, "author"));
                ui.vertical(|ui| {
                    let (age, color) = iso_age(&s(cm, "created"));
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(s(cm, "author")).size(12.0).color(c.white(1.0)));
                        ui.label(egui::RichText::new("·").size(12.0).color(c.fg2));
                        ui.label(egui::RichText::new(age).size(12.0).color(color));
                    });
                    if let Some(link) = super::markdown::show(ui, &s(cm, "body"), url) {
                        later.push(Box::new(move |app| app.open_url(&link)));
                    }
                });
            });
            ui.add_space(6.0);
        }
        if let Some(e) = &list.error {
            ui.label(egui::RichText::new(e).size(12.5).color(REMOVED));
        }
        if list.cursor.is_some() && widgets::link(ui, "Load more comments", 12.5).clicked() {
            later.push(Box::new(move |app| app.want_comments(tab, key, true, issue, number)));
        } else if list.cursor.is_none() && list.started && !list.loading && list.error.is_some() && widgets::link(ui, "Retry", 12.5).clicked() {
            // The first page failed: start the list over (a later page's failure keeps its cursor instead).
            later.push(Box::new(move |app| {
                app.hosted_mut(tab).comments.remove(&key);
                app.want_comments(tab, key, false, issue, number);
            }));
        }
    }

    fn review_ui(&self, ui: &mut Ui, tab: u64, number: u64, pr: &Value, pu: &mut PrUi, later: &mut Vec<Later>) {
        let c = p();
        ui.horizontal(|ui| {
            let (r, resp) = ui.allocate_exact_size(vec2(12.0, 16.0), Sense::click());
            widgets::chevron(ui, r, pu.review_open, 12.0, c.fg2);
            if resp.clicked() || ui.add(egui::Label::new(egui::RichText::new("Review").size(12.5).color(c.fg2)).sense(Sense::click())).clicked() {
                pu.review_open = !pu.review_open;
            }
        });
        if !pu.review_open {
            return;
        }
        let current = self.hosted.get(&tab).and_then(|h| h.review_sha.get(&number)).is_some_and(|read| *read == s(pr, "source_sha"));
        let data = self.hosted.get(&tab).and_then(|h| h.review.get(&number)).filter(|_| current);
        let sha = s(pr, "source_sha");
        let Some(data) = data else {
            let sha2 = sha.clone();
            later.push(Box::new(move |app| app.want_review(tab, number, sha2)));
            ui.label(egui::RichText::new("Loading review…").size(12.5).color(c.fg2));
            return;
        };
        match data {
            Loaded::Loading => {
                ui.label(egui::RichText::new("Loading review…").size(12.5).color(c.fg2));
            }
            Loaded::Failed(e) => {
                ui.label(egui::RichText::new(e).size(12.5).color(REMOVED));
            }
            Loaded::Ready(d) => {
                if let Some((ok, text)) = &pu.review_note {
                    ui.label(egui::RichText::new(text).size(12.5).color(if *ok { GOOD } else { REMOVED }));
                }
                let reviewers: Vec<String> = d["reviewers"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default();
                ui.label(egui::RichText::new(if reviewers.is_empty() { "No reviewers requested.".to_owned() } else { format!("Requested reviewers · {}", reviewers.join(", ")) }).size(12.5).color(c.fg2));
                for rv in d["reviews"].as_array().cloned().unwrap_or_default() {
                    let a = rv.as_array().cloned().unwrap_or_default();
                    let (author, state, body) = (a.first().and_then(Value::as_str).unwrap_or(""), a.get(1).and_then(Value::as_str).unwrap_or(""), a.get(2).and_then(Value::as_str).unwrap_or(""));
                    ui.label(egui::RichText::new(format!("{author} · {}", state.replace('_', " ").to_lowercase())).size(12.5).color(c.fg2));
                    if !body.is_empty()
                        && let Some(link) = super::markdown::show(ui, body, &s(pr, "url"))
                    {
                        later.push(Box::new(move |app| app.open_url(&link)));
                    }
                }
                let open = ["open", "opened"].contains(&s(pr, "state").as_str());
                if open {
                    ui.horizontal(|ui| {
                        let w = ui.available_width() - 130.0;
                        widgets::field(ui, &mut pu.reviewers, "Comma-separated usernames", w, 26.0, 12.5);
                        if widgets::button(ui, Kind::Default, None, "Request review", 24.0, !pu.busy && !pu.reviewers.trim().is_empty()).clicked() {
                            let names: Vec<String> = pu.reviewers.split(',').map(|n| n.trim().to_owned()).filter(|n| !n.is_empty()).collect();
                            let (sha2, sent) = (sha.clone(), Sent::Reviewers(pu.reviewers.clone()));
                            later.push(Box::new(move |app| app.submit_review(tab, number, sha2, serde_json::json!({"kind": "requestReviewers", "reviewers": names}), sent)));
                        }
                    });
                    egui::Frame::new().fill(c.field).stroke(Stroke::new(1.0, c.line)).corner_radius(CornerRadius::same(3)).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
                        ui.add(egui::TextEdit::multiline(&mut pu.review_text).frame(egui::Frame::NONE).hint_text("Review message").desired_rows(3).desired_width(f32::INFINITY));
                    });
                    ui.horizontal(|ui| {
                        if widgets::button(ui, Kind::Default, None, "Approve", 24.0, !pu.busy).clicked() {
                            let body = pu.review_text.clone();
                            let (sha2, sent) = (sha.clone(), Sent::Message(body.clone()));
                            later.push(Box::new(move |app| app.submit_review(tab, number, sha2, serde_json::json!({"kind": "approve", "body": body}), sent)));
                        }
                        if widgets::button(ui, Kind::Default, None, "Request changes", 24.0, !pu.busy && !pu.review_text.trim().is_empty()).clicked() {
                            let body = pu.review_text.clone();
                            let (sha2, sent) = (sha.clone(), Sent::Message(body.clone()));
                            later.push(Box::new(move |app| app.submit_review(tab, number, sha2, serde_json::json!({"kind": "requestChanges", "body": body}), sent)));
                        }
                    });
                }
                self.file_review(ui, tab, number, &sha, &s(pr, "url"), open, d, pu, later);
            }
        }
    }

    /// The changed files of a review: a file's patch lines, a line picked for an inline
    /// comment, and the comments on the file.
    #[allow(clippy::too_many_arguments)]
    fn file_review(&self, ui: &mut Ui, tab: u64, number: u64, sha: &str, url: &str, open: bool, d: &Value, pu: &mut PrUi, later: &mut Vec<Later>) {
        let c = p();
        let files = d["files"].as_array().cloned().unwrap_or_default();
        if !files.is_empty() {
            ui.add_space(6.0);
            let shown = if pu.review_file.is_empty() { "Choose a file".to_owned() } else { pu.review_file.clone() };
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Changed file").size(12.5).color(c.fg2));
                egui::ComboBox::from_id_salt(("review_file", tab, number)).selected_text(shown).width(ui.available_width().min(420.0)).show_ui(ui, |ui| {
                    for f in &files {
                        let path = f["path"].as_str().unwrap_or("").to_owned();
                        if ui.selectable_label(pu.review_file == path, &path).clicked() && pu.review_file != path {
                            pu.review_file = path;
                            pu.review_line = None;
                            pu.inline_text.clear();
                        }
                    }
                });
            });
        }
        let patch = files.iter().find(|f| f["path"].as_str() == Some(pu.review_file.as_str())).and_then(|f| f["patch"].as_str());
        if let Some(patch) = patch {
            let lines = review_lines(patch);
            const MAX: usize = 1500;
            egui::Frame::new().stroke(Stroke::new(1.0, c.line)).show(ui, |ui| {
                egui::ScrollArea::both().id_salt(("review_lines", tab, number, &pu.review_file)).max_height(210.0).auto_shrink([false, true]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for line in lines.iter().take(MAX) {
                        let picked = pu.review_line.as_ref() == Some(line);
                        let color = match line.text.chars().next() {
                            Some('+') => GOOD,
                            Some('-') => REMOVED,
                            _ => c.fg,
                        };
                        let text = egui::RichText::new(format!("{:>5} {}", line.number, line.text)).font(FontId::monospace(11.5)).color(color);
                        let resp = ui.add_enabled(open, egui::Button::selectable(picked, text).frame_when_inactive(false));
                        let resp = if open { resp.on_hover_text("Select this line for an inline comment") } else { resp };
                        if resp.clicked() {
                            pu.review_line = Some(line.clone());
                        }
                    }
                    if lines.len() > MAX {
                        ui.label(egui::RichText::new("Showing the first 1,500 diff lines. Open the pull request in your browser to review later lines.").size(12.0).color(c.fg3));
                    }
                });
            });
        }
        if open && let Some(line) = pu.review_line.clone() {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(format!("Comment on line {} ({})", line.number, if line.side == "RIGHT" { "new" } else { "old" })).size(12.5).color(c.fg2));
            egui::Frame::new().fill(c.field).stroke(Stroke::new(1.0, c.line)).corner_radius(CornerRadius::same(3)).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
                ui.add(egui::TextEdit::multiline(&mut pu.inline_text).frame(egui::Frame::NONE).hint_text("Inline comment").desired_rows(2).desired_width(f32::INFINITY));
            });
            if widgets::button(ui, Kind::Default, None, "Post inline comment", 24.0, !pu.busy && !pu.inline_text.trim().is_empty()).clicked() {
                let body = pu.inline_text.clone();
                let action = serde_json::json!({"kind": "inline", "path": pu.review_file, "side": line.side, "line": line.number, "body": body, "old_line": line.previous});
                let (sha2, sent) = (sha.to_owned(), Sent::Inline(body, line));
                later.push(Box::new(move |app| app.submit_review(tab, number, sha2, action, sent)));
            }
        }
        for cm in d["comments"].as_array().cloned().unwrap_or_default() {
            let path = cm["path"].as_str().unwrap_or("");
            if !pu.review_file.is_empty() && path != pu.review_file {
                continue;
            }
            let at = cm["line"].as_u64().map(|l| format!(" · line {l}")).unwrap_or_default();
            ui.add_space(4.0);
            ui.label(egui::RichText::new(format!("{} · {path}{at}", cm["author"].as_str().unwrap_or(""))).size(12.5).color(c.fg2));
            if let Some(link) = super::markdown::show(ui, cm["body"].as_str().unwrap_or(""), url) {
                later.push(Box::new(move |app| app.open_url(&link)));
            }
        }
    }

    fn want_review(&mut self, tab: u64, number: u64, sha: String) {
        let Some(path) = self.repo(tab).map(|r| r.path.clone()) else { return };
        let gitlab = self.gitlab();
        let h = self.hosted_mut(tab);
        if h.review.contains_key(&number) && h.review_sha.get(&number) == Some(&sha) {
            return;
        }
        let replaced = h.review.insert(number, Loaded::Loading).is_some();
        h.review_sha.insert(number, sha.clone());
        if replaced && let Some(r) = self.repo_mut(tab).filter(|r| r.pr_ui.number == number) {
            // A line picked in the old patch means nothing in the new one.
            r.pr_ui.review_line = None;
            r.pr_ui.inline_text.clear();
        }
        let read_for = sha.clone();
        self.spawn(move || {
            let r = gitcraft_core::session::remote_of(&path, &gitlab).and_then(|remote| gitcraft_core::hosting::review_data(&remote, number, &sha)).map(|d| serde_json::to_value(d).unwrap_or_default());
            super::Msg::Apply(Box::new(move |app: &mut App| {
                let h = app.hosted_mut(tab);
                if h.review_sha.get(&number) != Some(&read_for) {
                    return;
                }
                h.review.insert(number, match r {
                    Ok(v) => Loaded::Ready(v),
                    Err(e) => Loaded::Failed(e),
                });
            }))
        });
    }

    /// Send a review. The field it came from is cleared only once it was sent, and only if it
    /// still holds what was sent, so a failure or newer typing keeps the text.
    fn submit_review(&mut self, tab: u64, number: u64, sha: String, action: Value, sent: Sent) {
        let Some(path) = self.repo(tab).map(|r| r.path.clone()) else { return };
        let gitlab = self.gitlab();
        let Ok(action) = serde_json::from_value::<gitcraft_core::hosting::ReviewAction>(action) else { return };
        if let Some(r) = self.repo_mut(tab) {
            r.pr_ui.busy = true;
        }
        self.spawn(move || {
            let r = gitcraft_core::session::remote_of(&path, &gitlab).and_then(|remote| gitcraft_core::hosting::submit_review(&remote, number, &sha, action));
            super::Msg::Apply(Box::new(move |app: &mut App| {
                if let Some(t) = app.repo_mut(tab) {
                    t.pr_ui.busy = false;
                    if r.is_ok() && t.pr_ui.number == number {
                        match &sent {
                            Sent::Message(text) if t.pr_ui.review_text == *text => t.pr_ui.review_text.clear(),
                            Sent::Reviewers(text) if t.pr_ui.reviewers == *text => t.pr_ui.reviewers.clear(),
                            Sent::Inline(text, line) if t.pr_ui.inline_text == *text && t.pr_ui.review_line.as_ref() == Some(line) => {
                                t.pr_ui.inline_text.clear();
                                t.pr_ui.review_line = None;
                            }
                            _ => {}
                        }
                    }
                    t.pr_ui.review_note = Some(match &r {
                        Ok(()) => (true, "Review updated.".into()),
                        Err(e) => (false, e.clone()),
                    });
                }
                let h = app.hosted_mut(tab);
                h.review.remove(&number);
                h.review_sha.remove(&number);
                h.pr.remove(&number);
                h.prs = List::default();
            }))
        });
    }

    fn checkout_pr(&mut self, tab: u64, number: u64) {
        let Some(path) = self.repo(tab).map(|r| r.path.clone()) else { return };
        let gitlab = self.gitlab();
        self.spawn(move || {
            let r = gitcraft_core::session::remote_of(&path, &gitlab).and_then(|remote| gitcraft_core::hosting::pr_checkout(&remote, &gitcraft_core::hosting::pull_request(&remote, number)?));
            super::Msg::Apply(Box::new(move |app: &mut App| match r {
                Ok(t) => app.act(tab, Action::CheckoutPullRequest { url: t.url, reference: t.reference, expected: t.expected }),
                Err(e) => app.toast(false, e),
            }))
        });
    }

    fn merge_pr(&mut self, tab: u64, number: u64, method: String, sha: String, squash: String, source: String) {
        let Some(path) = self.repo(tab).map(|r| r.path.clone()) else { return };
        let gitlab = self.gitlab();
        if let Some(r) = self.repo_mut(tab) {
            r.pr_ui.busy = true;
        }
        self.spawn(move || {
            let expected = (!sha.is_empty()).then_some(sha.clone());
            let squash_message = (method == "squash" && !squash.is_empty()).then_some(squash);
            let r = gitcraft_core::session::remote_of(&path, &gitlab).and_then(|remote| gitcraft_core::hosting::merge_pr(&remote, number, &method, expected.as_deref(), squash_message.as_deref()));
            super::Msg::Apply(Box::new(move |app: &mut App| {
                if let Some(t) = app.repo_mut(tab) {
                    t.pr_ui.busy = false;
                }
                match r {
                    Ok(()) => {
                        if let Some(t) = app.repo_mut(tab)
                            && !sha.is_empty()
                        {
                            t.pr_ui.merged = Some((number, source, sha));
                        }
                        app.toast(true, format!("Merged #{number}"));
                        let h = app.hosted_mut(tab);
                        h.prs = List::default();
                        h.pr.remove(&number);
                        app.reset_launchpad();
                        app.act_quiet(tab, Action::Fetch, true);
                    }
                    Err(e) => app.toast(false, e),
                }
            }))
        });
    }

    fn delete_merged_source(&mut self, tab: u64, number: u64, source: String, sha: String, remote: bool) {
        let where_ = if remote { "the source repository" } else { "this clone" };
        let text = format!("Delete {source} from {where_} if it still matches the merged commit?");
        self.dialog = Some(Dialog::confirm_with("Delete merged branch?", &text, "Delete Branch", true, move |app, _| {
            if !remote {
                app.act(tab, Action::DeleteMergedLocal { name: source, expected: sha });
                return;
            }
            let Some(path) = app.repo(tab).map(|r| r.path.clone()) else { return };
            let gitlab = app.gitlab();
            app.spawn(move || {
                let r = (|| {
                    let target = gitcraft_core::session::remote_of(&path, &gitlab)?;
                    let pr = gitcraft_core::hosting::merged_source(&target, number, &sha)?;
                    let src = gitcraft_core::hosting::parse_remote(&pr.source_url, &gitlab).filter(|x| x.service == target.service && x.host == target.host).ok_or("The hosting service did not return a verified HTTPS source repository.")?;
                    let dir = Path::new(&path);
                    let reference = format!("refs/heads/{}", pr.source);
                    if !gitcraft_core::git::run(dir, &["check-ref-format", &reference]).ok {
                        return Err("The source branch name is invalid.".to_owned());
                    }
                    let head = gitcraft_core::git::run(dir, &["ls-remote", "--symref", "--", &pr.source_url, "HEAD"]);
                    if !head.ok {
                        return Err(head.message());
                    }
                    let default = head.stdout.lines().find_map(|l| l.strip_prefix("ref: ").and_then(|l| l.split_once('\t')).filter(|(_, n)| *n == "HEAD").map(|(r, _)| r.to_owned()));
                    if default.is_none() || default.as_deref() == Some(reference.as_str()) {
                        return Err(format!("Could not confirm that {} is safe to delete from {}.", pr.source, src.web));
                    }
                    let guard = format!("--force-with-lease={reference}:{sha}");
                    let refspec = format!(":{reference}");
                    let o = gitcraft_core::git::run(dir, &["push", &guard, "--", &pr.source_url, &refspec]);
                    if o.ok { Ok(pr.source) } else { Err(o.message()) }
                })();
                super::Msg::Apply(Box::new(move |app: &mut App| match r {
                    Ok(name) => {
                        if let Some(t) = app.repo_mut(tab) {
                            t.pr_ui.remote_deleted = true;
                        }
                        app.toast(true, format!("Deleted {name} from the source repository"));
                        app.act_quiet(tab, Action::Fetch, true);
                    }
                    Err(e) => app.toast(false, e),
                }))
            });
        }));
    }

    /// Post `draft` (trimmed) on the pull request; the box is cleared only if it still holds it.
    fn send_pr_comment(&mut self, tab: u64, number: u64, draft: String) {
        let Some(path) = self.repo(tab).map(|r| r.path.clone()) else { return };
        let gitlab = self.gitlab();
        self.spawn(move || {
            let body = draft.trim();
            let r = gitcraft_core::session::remote_of(&path, &gitlab).and_then(|remote| gitcraft_core::hosting::add_comment(&remote, number, body));
            super::Msg::Apply(Box::new(move |app: &mut App| {
                if let Some(t) = app.repo_mut(tab) {
                    t.pr_ui.busy = false;
                    // Cleared only on the pull request it was posted to, if not edited since.
                    if r.is_ok() && t.pr_ui.number == number && t.pr_ui.comment == draft {
                        t.pr_ui.comment.clear();
                    }
                }
                match r {
                    Ok(()) => {
                        app.hosted_mut(tab).comments.remove(&format!("p{number}"));
                    }
                    Err(e) => app.toast(false, e),
                }
            }))
        });
    }

    pub(crate) fn issue_view(&self, ui: &mut Ui, tab: u64, r: &mut RepoTab, id: &str, later: &mut Vec<Later>) {
        let c = p();
        let h = self.hosted.get(&tab);
        if !h.is_some_and(|h| h.issues.started) {
            later.push(Box::new(move |app| app.want_issues(tab, false)));
        }
        let issue = h.and_then(|h| h.issues.items.iter().find(|i| issue_identity(i) == id)).cloned();
        let w = ui.available_width();
        let (bar, _) = ui.allocate_exact_size(vec2(w, 32.0), Sense::hover());
        ui.painter().rect_filled(bar, CornerRadius::ZERO, c.toolbar.gamma_multiply(0.6));
        ui.painter().line_segment([bar.left_bottom(), bar.right_bottom()], Stroke::new(1.0, c.black(0.3)));
        icons::paint(ui, Rect::from_min_size(pos2(bar.left() + 12.0, bar.center().y - 7.0), Vec2::splat(14.0)), Icon::CircleDot, 14.0, GOOD);
        let label = issue.as_ref().map(|i| {
            let key = s(i, "key");
            format!("{} {}", if key.is_empty() { format!("#{}", i["number"].as_u64().unwrap_or(0)) } else { key }, s(i, "title"))
        }).unwrap_or_else(|| "Issue".into());
        let g = widgets::fit(ui, &label, super::fonts::semibold(13.0), c.white(1.0), w - 110.0);
        ui.painter().galley(pos2(bar.left() + 34.0, bar.center().y - g.size().y / 2.0), g, c.white(1.0));
        let mut close = false;
        let mut right = ui.new_child(egui::UiBuilder::new().max_rect(bar.shrink2(vec2(8.0, 0.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
        if widgets::icon_button(&mut right, Icon::X, 16.0, "Close (Esc)").clicked() {
            close = true;
        }
        if let Some(i) = &issue
            && widgets::icon_button(&mut right, Icon::ExternalLink, 14.0, "Open in browser").clicked()
        {
            let u = s(i, "url");
            later.push(Box::new(move |app| app.open_url(&u)));
        }
        if !ui.ctx().egui_wants_keyboard_input() && !super::dialog_open(ui.ctx()) && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        if close {
            r.ui.issue = None;
            return;
        }
        let Some(issue) = issue else {
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                ui.label(egui::RichText::new(if h.is_some_and(|h| h.issues.loading) { "Loading…" } else { "This issue is no longer open." }).size(13.0).color(c.fg3));
            });
            return;
        };
        let url = s(&issue, "url");
        egui::ScrollArea::vertical().id_salt(("issue", tab, id)).auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::new().inner_margin(egui::Margin::same(16)).show(ui, |ui| {
                ui.set_width(w - 32.0);
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new("opened by").size(12.5).color(c.fg2));
                    ui.label(egui::RichText::new(s(&issue, "author")).size(12.5).color(c.white(1.0)));
                    for l in issue["labels"].as_array().cloned().unwrap_or_default() {
                        egui::Frame::new().fill(super::theme::ACCENT.gamma_multiply(0.2)).corner_radius(CornerRadius::same(3)).inner_margin(egui::Margin::symmetric(6, 1)).show(ui, |ui| {
                            ui.label(egui::RichText::new(l.as_str().unwrap_or("")).size(11.5).color(c.white(1.0)));
                        });
                    }
                });
                if widgets::small(ui, Kind::Primary, "Create branch for this issue").clicked() {
                    let i2 = issue.clone();
                    later.push(Box::new(move |app| app.branch_from_issue(tab, &i2)));
                }
                if widgets::small(ui, Kind::Default, "Start an Agent Session").clicked() {
                    let number = if s(&issue, "key").is_empty() { issue["number"].as_u64().unwrap_or(0).to_string() } else { s(&issue, "key") };
                    let title = s(&issue, "title");
                    later.push(Box::new(move |app| app.start_agent_for_issue(tab, &number, &title)));
                }
                ui.add_space(8.0);
                egui::Frame::new().fill(c.panel).stroke(Stroke::new(1.0, c.white(0.1))).corner_radius(CornerRadius::same(4)).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
                    ui.set_width(w - 56.0);
                    let body = s(&issue, "body");
                    if body.trim().is_empty() {
                        ui.label(egui::RichText::new("No description.").size(13.0).color(c.fg3));
                    } else if let Some(link) = super::markdown::show(ui, &body, &url) {
                        later.push(Box::new(move |app| app.open_url(&link)));
                    }
                });
                if ["github", "gitlab", "bitbucket", "gitea", "forgejo", "codeberg"].contains(&s(&issue, "tracker").as_str()) {
                    ui.add_space(12.0);
                    ui.label(egui::RichText::new("COMMENTS").font(super::fonts::semibold(12.0)).color(c.fg2));
                    self.comments_ui(ui, tab, issue["number"].as_u64().unwrap_or(0), true, &url, later);
                }
            });
        });
    }

    /// Start a pull request from `branch`: a template, the target, title and description, and
    /// pushing the branch first when the remote doesn't have it.
    pub(crate) fn start_pull_request(&mut self, tab: u64, branch: &str) {
        let Some(path) = self.repo(tab).map(|r| r.path.clone()) else { return };
        let branch = branch.to_owned();
        let gitlab = self.gitlab();
        self.spawn(move || {
            let dir = Path::new(&path);
            let templates = gitcraft_core::extras::pr_templates(dir);
            let target = gitcraft_core::extras::default_branch_for(dir, &branch).map(|(_, b)| b);
            let mut templates: Vec<(String, String)> = templates.into_iter().map(|t| (t.name, t.body)).collect();
            // A repository without a template of its own uses its GitHub organization's default one.
            let remote = gitcraft_core::session::remote_of(&path, &gitlab).ok();
            if templates.is_empty()
                && let Some(t) = remote.as_ref().and_then(gitcraft_core::hosting::org_pr_template)
            {
                templates.push(t);
            }
            // Stacked pull requests: a branch made from another open pull request's branch targets that branch, so it
            // shows only its own changes; the form says which pull request it stacks on.
            let open = remote.as_ref().and_then(|r| gitcraft_core::hosting::pull_requests(r).ok()).unwrap_or_default();
            let below = open.iter().filter(|pr| pr.source != branch && !pr.source_sha.is_empty())
                .filter(|pr| gitcraft_core::git::run(dir, &["merge-base", "--is-ancestor", &pr.source_sha, &branch]).ok)
                // The nearest: the one with the fewest commits between it and this branch.
                .min_by_key(|pr| gitcraft_core::git::run(dir, &["rev-list", "--count", &format!("{}..{branch}", pr.source_sha)]).stdout.trim().parse::<u64>().unwrap_or(u64::MAX))
                .cloned();
            let target = below.as_ref().map(|pr| pr.source.clone()).or(target);
            let stack = below.map(|pr| format!("This pull request stacks on #{} ({}), so it targets {}.", pr.number, pr.title, pr.source));
            let stack_key = (tab, branch.clone());
            super::Msg::Apply(Box::new(move |app: &mut App| {
                app.pr_stack = stack.map(|note| (stack_key.0, stack_key.1, note));
                app.pull_request_form(tab, branch, target, templates)
            }))
        });
    }

    fn pull_request_form(&mut self, tab: u64, branch: String, target: Option<String>, templates: Vec<(String, String)>) {
        if templates.len() >= 2 {
            let mut options: Vec<(String, String)> = templates.iter().map(|(n, _)| (n.clone(), n.clone())).collect();
            options.push((String::new(), "No template".into()));
            let first = templates[0].0.clone();
            let d = Dialog::form("Choose a pull request template", vec![Field::new("template", "Template", FieldKind::Select(options))], "Continue", move |app, v| {
                let body = templates.iter().find(|(n, _)| *n == v.text("template")).map(|(_, b)| b.clone());
                app.pull_request_dialog(tab, branch, target, body, None);
            })
            .text_value("template", &first);
            self.dialog = Some(d);
            return;
        }
        let body = templates.first().map(|(_, b)| b.clone());
        self.pull_request_dialog(tab, branch, target, body, None);
    }

    /// `text`: the title, description and draft choice to start from (after Write with AI).
    fn pull_request_dialog(&mut self, tab: u64, branch: String, target: Option<String>, template: Option<String>, text: Option<(String, String, bool)>) {
        let Some(v) = self.repo(tab).and_then(|r| r.view.as_ref()) else { return };
        let r = v.data.refs.iter().find(|x| x.kind == gitcraft_core::git::RefKind::Local && x.name == branch).cloned();
        let head = r.as_ref().and_then(|r| v.data.commits.iter().find(|c| c.sha == r.sha)).cloned();
        let needs_push = r.as_ref().is_none_or(|r| r.upstream.is_empty() || r.ahead > 0);
        let fields = vec![
            Field::new("target", "Into branch", FieldKind::Text),
            Field::new("title", "Title", FieldKind::Text),
            Field::new("body", "Description", FieldKind::Multiline(8)),
            Field::new("draft", "Create as a draft", FieldKind::Check),
        ];
        let body = [head.as_ref().map(|h| h.body.clone()).filter(|b| !b.is_empty()), template].into_iter().flatten().collect::<Vec<_>>().join("\n\n");
        let b2 = branch.clone();
        let (title, body, draft) = text.unwrap_or_else(|| (head.map(|h| h.summary).unwrap_or_else(|| branch.clone()), body, false));
        let mut intro = "The branch is pushed first if the remote doesn't have it yet.".to_owned();
        if let Some((_, _, note)) = self.pr_stack.as_ref().filter(|(t, b, _)| *t == tab && *b == branch) {
            intro = format!("{note}\n{intro}");
        }
        let d = Dialog::new(&format!("Start a pull request from {branch}"), super::dialogs::Body::Form { intro, fields, grid: false })
            .width(560.0)
            .text_value("target", &target.unwrap_or_else(|| "main".into()))
            .text_value("title", &title)
            .text_value("body", &body)
            .flag_value("draft", draft)
            .choice("Write with AI", Kind::Ai, move |app, v| {
                let target = v.text("target").trim().to_owned();
                let request = super::unique_id();
                app.pr_waiting = Some((tab, b2.clone(), target.clone(), v.flag("draft"), request));
                app.ai_pull_request(tab, request, b2, target);
            })
            .choice("Create Pull Request", Kind::Primary, move |app, v| {
                let (target, title, body, draft) = (v.text("target").trim().to_owned(), v.text("title").trim().to_owned(), v.text("body"), v.flag("draft"));
                app.create_pull_request(tab, branch, target, title, body, draft, needs_push);
            });
        let mut d = d;
        d.stay_open = false;
        self.dialog = Some(d);
    }

    /// "Write with AI" filled the title and description.
    pub(crate) fn pr_text_ready(&mut self, tab: u64, request: u64, title: String, body: String) {
        // Only the request the form is waiting for fills it; an older one's text is dropped without touching the wait.
        if self.pr_waiting.as_ref().is_none_or(|w| w.0 != tab || w.4 != request) {
            return;
        }
        match self.pr_waiting.take() {
            Some((t, branch, target, draft, _)) if self.dialog.is_none() => {
                debug_assert_eq!(t, tab);
                self.pull_request_dialog(tab, branch, Some(target), None, Some((title, body, draft)));
            }
            other => self.pr_waiting = other,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn create_pull_request(&mut self, tab: u64, branch: String, target: String, title: String, body: String, draft: bool, push: bool) {
        let Some(path) = self.repo(tab).map(|r| r.path.clone()) else { return };
        let (engine, gitlab) = (self.engine.clone(), self.gitlab());
        self.busy = Some("Creating pull request…".into());
        self.spawn(move || {
            let r = (|| {
                if push {
                    let out = engine.perform(&path, Action::PushBranch { branch: branch.clone() });
                    if !out.ok {
                        return Err(out.message);
                    }
                }
                let (remote, source) = gitcraft_core::hosting::pull_request_destination(Path::new(&path), &branch, &gitlab)?;
                gitcraft_core::hosting::create_pr(&remote, &source, &target, &title, &body, draft)
            })();
            super::Msg::Apply(Box::new(move |app: &mut App| {
                app.busy = None;
                match r {
                    Ok(pr) => {
                        app.toast(true, format!("Created #{}: {}", pr.number, pr.title));
                        app.hosted_mut(tab).prs = List::default();
                        app.open_pr(tab, pr.number);
                    }
                    Err(e) => app.toast(false, e),
                }
            }))
        });
    }

    fn launchpad_load(&mut self) {
        if self.launchpad.statuses.is_none() {
            self.launchpad.statuses = Some(Loaded::Loading);
            self.spawn(|| {
                let handles: Vec<_> = super::prefs_ui::SERVICES.iter().map(|s| std::thread::spawn(move || gitcraft_core::hosting::status(s.id))).collect();
                let list: Vec<_> = handles.into_iter().filter_map(|h| h.join().ok()).collect();
                super::Msg::Apply(Box::new(move |app: &mut App| app.launchpad.statuses = Some(Loaded::Ready(list))))
            });
        }
        for svc in LAUNCHPAD_SERVICES {
            if self.launchpad_connected().iter().any(|c| c == svc) && !self.launchpad.pages.contains_key(svc) {
                self.want_launchpad(svc, false);
            }
        }
    }

    fn launchpad_connected(&self) -> Vec<String> {
        match &self.launchpad.statuses {
            Some(Loaded::Ready(s)) => s.iter().filter(|s| s.connected && s.error.is_none()).map(|s| s.id.clone()).collect(),
            _ => Vec::new(),
        }
    }

    /// Pull requests and issues across the connected services, newest first, and the errors.
    fn launchpad_items(&self) -> (Vec<Value>, Vec<Value>, Vec<String>) {
        let connected = self.launchpad_connected();
        let mut prs: Vec<Value> = Vec::new();
        let mut issues: Vec<Value> = Vec::new();
        let mut errors: Vec<String> = Vec::new();
        for (k, l) in &self.launchpad.pages {
            if !connected.contains(k) {
                continue;
            }
            for page in &l.items {
                for pr in page["pull_requests"].as_array().cloned().unwrap_or_default() {
                    if !prs.iter().any(|x| x["url"] == pr["url"] && x["number"] == pr["number"] && x["repo"] == pr["repo"]) {
                        prs.push(pr);
                    }
                }
                for i in page["issues"].as_array().cloned().unwrap_or_default() {
                    if !issues.iter().any(|x| issue_identity(x) == issue_identity(&i)) {
                        issues.push(i);
                    }
                }
            }
            if let Some(e) = &l.error {
                errors.push(e.clone());
            } else if let Some(last) = l.items.last() {
                errors.extend(last["errors"].as_array().cloned().unwrap_or_default().into_iter().filter_map(|e| e.as_str().map(str::to_owned)));
            }
        }
        prs.sort_by(|a, b| s(b, "updated").cmp(&s(a, "updated")));
        issues.sort_by(|a, b| s(b, "updated").cmp(&s(a, "updated")));
        (prs, issues, errors)
    }

    /// The Launchpad: pull requests and issues that involve you, across every connected service.
    pub(crate) fn launchpad(&mut self, ui: &mut Ui) {
        let c = p();
        self.launchpad_load();
        let statuses = match &self.launchpad.statuses {
            Some(Loaded::Ready(s)) => Some(s.clone()),
            _ => None,
        };
        let connected = self.launchpad_connected();
        let (prs, issues, errors) = self.launchpad_items();
        let pages: Vec<&List> = self.launchpad.pages.iter().filter(|(k, _)| connected.contains(k)).map(|(_, v)| v).collect();
        let loading = statuses.is_none() || pages.iter().any(|l| l.loading);
        let has_more = pages.iter().any(|l| l.cursor.is_some());
        let mut open: Option<String> = None;
        let mut refresh = false;
        let mut more = false;
        egui::ScrollArea::vertical().id_salt("launchpad").auto_shrink([false, false]).show(ui, |ui| {
            let w = ui.available_width().min(900.0);
            ui.horizontal(|ui| {
                ui.add_space(((ui.available_width() - w) / 2.0).max(0.0));
                ui.vertical(|ui| {
                    ui.set_width(w - 64.0);
                    ui.add_space(24.0);
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
                        icons::paint(ui, r, Icon::Rocket, 22.0, c.link);
                        ui.label(egui::RichText::new("Launchpad").font(super::fonts::semibold(18.0)).color(c.white(1.0)));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if widgets::link(ui, if loading { "Refreshing…" } else { "Refresh" }, 12.5).clicked() {
                                refresh = true;
                            }
                        });
                    });
                    ui.add_space(16.0);
                    if statuses.is_some() && connected.is_empty() {
                        egui::Frame::new().fill(c.panel).stroke(Stroke::new(1.0, c.white(0.1))).corner_radius(CornerRadius::same(4)).inner_margin(egui::Margin::same(16)).show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(egui::RichText::new("Connect a hosting or issue tracking account to see your pull requests and issues here.").size(13.0).color(c.fg2));
                                if widgets::link(ui, "Open Accounts", 13.0).clicked() {
                                    open = Some(String::new());
                                }
                            });
                        });
                        ui.add_space(16.0);
                    }
                    for e in &errors {
                        ui.label(egui::RichText::new(e).size(12.0).color(REMOVED));
                    }
                    let list = |ui: &mut Ui, title: &str, rows: &[Value], issue: bool, open: &mut Option<String>| {
                        ui.add_space(12.0);
                        ui.label(egui::RichText::new(format!("{title} ({})", rows.len())).font(super::fonts::semibold(12.0)).color(c.fg2));
                        ui.add_space(4.0);
                        egui::Frame::new().fill(c.panel).stroke(Stroke::new(1.0, c.white(0.08))).corner_radius(CornerRadius::same(4)).show(ui, |ui| {
                            ui.set_width(w - 64.0);
                            ui.spacing_mut().item_spacing = Vec2::ZERO;
                            if rows.is_empty() {
                                ui.add_space(10.0);
                                ui.horizontal(|ui| {
                                    ui.add_space(12.0);
                                    ui.label(egui::RichText::new(if issue { "No open issues assigned to you." } else { "Nothing needs you right now." }).size(12.5).color(c.fg3));
                                });
                                ui.add_space(10.0);
                            }
                            for row in rows {
                                let (r, resp) = ui.allocate_exact_size(vec2(w - 64.0, 34.0), Sense::click());
                                if resp.hovered() {
                                    ui.painter().rect_filled(r, CornerRadius::ZERO, c.hoverrow);
                                }
                                ui.painter().line_segment([r.left_bottom(), r.right_bottom()], Stroke::new(1.0, c.white(0.05)));
                                let draft = row["draft"].as_bool().unwrap_or(false);
                                icons::paint(ui, Rect::from_min_size(pos2(r.left() + 12.0, r.center().y - 7.0), Vec2::splat(14.0)), if issue { Icon::CircleDot } else { Icon::GitPullRequest }, 14.0, if draft { c.fg3 } else { GOOD });
                                let repo = widgets::fit(ui, &s(row, "repo"), FontId::proportional(12.0), c.fg3, 160.0);
                                ui.painter().galley(pos2(r.left() + 34.0, r.center().y - repo.size().y / 2.0), repo, c.fg3);
                                let key = s(row, "key");
                                let num = if issue && !key.is_empty() { key } else { format!("#{}", row["number"].as_u64().unwrap_or(0)) };
                                let ng = ui.painter().layout_no_wrap(num, FontId::proportional(12.0), c.fg3);
                                let nw = ng.size().x;
                                ui.painter().galley(pos2(r.left() + 200.0, r.center().y - ng.size().y / 2.0), ng, c.fg3);
                                let (age, color) = iso_age(&s(row, "updated"));
                                ui.painter().text(pos2(r.right() - 12.0, r.center().y), Align2::RIGHT_CENTER, age, FontId::proportional(12.0), color);
                                let mut right = r.right() - 52.0;
                                if !issue {
                                    let ag = ui.painter().layout_no_wrap(s(row, "author"), FontId::proportional(12.0), c.fg2);
                                    right -= ag.size().x;
                                    ui.painter().galley(pos2(right, r.center().y - ag.size().y / 2.0), ag, c.fg2);
                                    right -= 8.0;
                                }
                                let tg = widgets::fit(ui, &s(row, "title"), FontId::proportional(13.0), c.white(0.9), right - (r.left() + 208.0 + nw));
                                ui.painter().galley(pos2(r.left() + 208.0 + nw, r.center().y - tg.size().y / 2.0), tg, c.white(0.9));
                                if resp.clicked() {
                                    *open = Some(s(row, "url"));
                                }
                            }
                        });
                    };
                    list(ui, "PULL REQUESTS", &prs, false, &mut open);
                    list(ui, "ISSUES ASSIGNED TO YOU", &issues, true, &mut open);
                    if loading {
                        ui.add_space(12.0);
                        ui.label(egui::RichText::new("Loading connected services...").size(13.0).color(c.fg3));
                    }
                    if has_more {
                        ui.add_space(12.0);
                        if widgets::link(ui, "Load more results", 13.0).clicked() {
                            more = true;
                        }
                    }
                    ui.add_space(24.0);
                });
            });
        });
        match open {
            Some(u) if u.is_empty() => self.open_prefs(Some("integrations")),
            Some(u) => self.open_url(&u),
            None => {}
        }
        if refresh {
            self.reset_launchpad();
        }
        if more {
            for svc in LAUNCHPAD_SERVICES {
                if self.launchpad.pages.get(svc).is_some_and(|l| l.cursor.is_some() && !l.loading) {
                    self.want_launchpad(svc, true);
                }
            }
        }
    }

    /// Start the Launchpad over. The generation counts these resets (it is not part of the state replaced), so a page
    /// still on its way from before a reset never lands in the new lists.
    fn reset_launchpad(&mut self) {
        self.launchpad = Launchpad::default();
        self.launchpad_generation += 1;
    }

    fn want_launchpad(&mut self, service: &'static str, more: bool) {
        let generation = self.launchpad_generation;
        let list = self.launchpad.pages.entry(service.to_owned()).or_default();
        if list.loading || (list.started && !more) {
            return;
        }
        list.loading = true;
        list.started = true;
        let cursor = if more { list.cursor.clone() } else { None };
        self.spawn(move || {
            let r = gitcraft_core::hosting::launchpad_page(service, cursor.as_deref());
            super::Msg::Apply(Box::new(move |app: &mut App| {
                if app.launchpad_generation != generation {
                    return;
                }
                let list = app.launchpad.pages.entry(service.to_owned()).or_default();
                list.loading = false;
                match r {
                    Ok(page) => {
                        list.items.push(serde_json::to_value(page.items).unwrap_or_default());
                        list.cursor = page.cursor;
                        list.error = None;
                    }
                    Err(e) => list.error = Some(e),
                }
            }))
        });
    }

    /// The status bar's hint and whether it opens the Launchpad (otherwise Accounts): a nudge
    /// to connect, or the first draft pull request that involves you.
    pub(crate) fn integration_hint(&mut self) -> Option<(String, bool)> {
        self.launchpad_load();
        let Some(Loaded::Ready(statuses)) = &self.launchpad.statuses else { return None };
        let connected: Vec<String> = statuses.iter().filter(|s| s.connected).map(|s| s.id.clone()).collect();
        if connected.is_empty() {
            return Some(("Connect an integration".into(), false));
        }
        let tab = self.active;
        self.repo(tab)?;
        if self.hosted.get(&tab).and_then(|h| h.remote.as_ref()).is_none() {
            self.want_remote(tab);
        }
        let service = match self.hosted.get(&tab).and_then(|h| h.remote.as_ref()) {
            Some(Loaded::Ready(Some(r))) => s(r, "service"),
            Some(Loaded::Ready(None)) => String::new(),
            _ => return None,
        };
        if !connected.contains(&service) {
            return Some(("Select an integration".into(), false));
        }
        let pages = self.launchpad.pages.get(&service)?;
        if pages.items.is_empty() {
            return None;
        }
        let (prs, _, _) = self.launchpad_items();
        Some((prs.iter().find(|p| p["draft"].as_bool().unwrap_or(false)).map(|p| format!("{}#{} is in draft", s(p, "repo"), p["number"].as_u64().unwrap_or(0))).unwrap_or_else(|| "No Pull Requests".into()), true))
    }
}

#[cfg(test)]
mod review_tests {
    use super::review_lines;

    #[test]
    fn patch_lines_get_their_side_and_number() {
        let lines = review_lines("@@ -10,3 +20,4 @@ fn x\n keep\n-old\n+new\n+more\n\\ No newline at end of file");
        let got: Vec<(&str, u64, Option<u64>)> = lines.iter().map(|l| (l.side, l.number, l.previous)).collect();
        assert_eq!(got, [("RIGHT", 20, Some(10)), ("LEFT", 11, None), ("RIGHT", 21, None), ("RIGHT", 22, None)]);
        assert!(review_lines("").is_empty());
    }
}
