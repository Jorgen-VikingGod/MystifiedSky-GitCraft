#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod accounts;
mod actions;
mod discard_backups;
mod diagnostics;
mod jobs;
mod files;
mod git;
mod graph;
mod ai;
mod ai_context;
mod operation;
mod pagination;
mod process_tree;
mod read_jobs;
mod agents;
mod ai_cli;
mod ai_models;
mod extras;
mod external;
#[cfg(target_os = "macos")]
mod finder_services;
mod prefs;
mod hosting;
mod hosting_api;
mod publishing;
mod manage;
mod rewrite;
mod metrics;
mod sample;
mod settings_file;
mod shell;
mod shot;
mod terminal;
mod watch;
mod watch_plan;
#[cfg(test)]
mod regressions;

use actions::{Action, ActionResult, Engine, UndoInfo};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{Emitter, Manager, State};

use metrics::INITIAL_COMMITS;

struct WatchSlot {
    read_id: String,
    generation: Arc<()>,
    watcher: Option<watch::RepoWatch>,
}

struct AppState {
    engine: Arc<Engine>,
    screenshot: Option<PathBuf>,
    /// Test aid: which screen `--screenshot` pictures (`--shot-view diff|split|history|start`).
    shot_view: Option<String>,
    watchers: Arc<Mutex<HashMap<(String, String), WatchSlot>>>,
    terminals: terminal::Terminals,
    launch_paths: Mutex<HashMap<String, Vec<LaunchIntent>>>,
}

#[derive(Serialize)]
struct LaunchOptions {
    screenshot: bool,
    shot_view: Option<String>,
    version: String,
    metrics: metrics::Metrics,
}

#[derive(Serialize)]
struct RepoView {
    path: String,
    name: String,
    #[serde(flatten)]
    data: git::RepoData,
    graph: graph::GraphLayout,
    history_cursor: Option<HistoryCursor>,
    undo: UndoInfo,
}

#[derive(Clone, Deserialize, Serialize)]
struct HistoryCursor {
    revs: Vec<String>,
    offset: usize,
    layout: graph::LayoutCursor,
    #[serde(default)]
    filter: git::HistoryFilter,
}

#[derive(Serialize)]
struct HistoryPage {
    commits: Vec<git::Commit>,
    graph: graph::GraphLayout,
    has_more: bool,
    history_cursor: Option<HistoryCursor>,
}

#[tauri::command]
async fn history_page(path: String, cursor: HistoryCursor, max: Option<usize>, read_id: Option<String>) -> Result<HistoryPage, String> {
    blocking(move || read_jobs::scope(read_id, || {
        let mut cursor = cursor;
        let (commits, has_more) = git::history_page(Path::new(&path), &cursor.revs, cursor.offset, max.unwrap_or(2000).clamp(100, 5000), &cursor.filter)?;
        let graph = cursor.layout.append(&commits);
        cursor.offset += commits.len();
        Ok(HistoryPage { commits, graph, has_more, history_cursor: has_more.then_some(cursor) })
    })).await?
}

fn err<E: ToString>(e: E) -> String {
    e.to_string()
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(err)
}

async fn read_blocking<T: Send + 'static>(id: Option<String>, f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    blocking(move || read_jobs::scope(id, f)).await
}

async fn ai_blocking<T: Send + 'static>(id: Option<String>, f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    blocking(move || read_jobs::scope_for(id, std::time::Duration::from_secs(300), f)).await
}

#[tauri::command]
async fn copy_diagnostics(state: State<'_, AppState>, app: tauri::AppHandle) -> Result<String, String> {
    if state.screenshot.is_some() { return Ok("GitCraft verification fixture".into()); }
    let version = app.package_info().version.to_string();
    blocking(move || diagnostics::report(&version)).await
}

#[tauri::command]
async fn check_git_version(program: String) -> Result<(), String> {
    blocking(move || git::check_version(&program)).await?
}

#[tauri::command]
async fn history_fetch_info(path: String, read_id: Option<String>) -> Result<extras::HistoryFetchInfo, String> {
    read_blocking(read_id, move || extras::history_fetch_info(Path::new(&path))).await?
}

#[tauri::command]
async fn repo_reflog(path: String, read_id: Option<String>) -> Result<Vec<extras::ReflogEntry>, String> {
    read_blocking(read_id, move || extras::reflog(Path::new(&path))).await?
}

#[tauri::command]
async fn commit_template(path: String, read_id: Option<String>) -> Result<Option<(String, String)>, String> {
    read_blocking(read_id, move || extras::commit_template(Path::new(&path))).await?
}

#[tauri::command]
async fn commit_signature(path: String, sha: String, read_id: Option<String>) -> Result<String, String> {
    read_blocking(read_id, move || {
        let dir = Path::new(&path);
        let output = git::run(dir, &["log", "-1", "--no-show-signature", "--format=%G?", "--end-of-options", &sha, "--"]);
        if output.ok { Ok(output.stdout.trim().to_owned()) } else { Err(output.message()) }
    }).await?
}

#[tauri::command]
fn launch_options(state: State<'_, AppState>) -> LaunchOptions {
    LaunchOptions { screenshot: state.screenshot.is_some(), shot_view: state.shot_view.clone(), version: env!("CARGO_PKG_VERSION").to_string(), metrics: metrics::metrics() }
}

#[tauri::command]
async fn open_repo(path: String, max: Option<usize>, view: Option<git::RefView>, state: State<'_, AppState>, read_id: Option<String>) -> Result<RepoView, String> {
    let engine = state.engine.clone();
    blocking(move || read_jobs::scope(read_id, || {
        // Tabs saved by versions that turned /Users/x into \Users\x on macOS/Linux: restore the slashes.
        let path = if cfg!(not(windows)) && path.starts_with('\\') { path.replace('\\', "/") } else { path };
        let p = PathBuf::from(&path);
        if !p.exists() {
            return Err(format!("The folder {path} doesn't exist"));
        }
        let root = git::repo_root(&p).ok_or_else(|| format!("{path} is not a Git repository"))?;
        let operation = operation::lock_for(&root);
        if let Ok(_guard) = operation.try_lock() { rewrite::cleanup_replay_files(&root); }
        let view = view.unwrap_or_default();
        let data = git::load_view_checked(&root, max.unwrap_or(INITIAL_COMMITS).clamp(prefs::MIN_INITIAL_COMMITS, 5000), &view)?;
        let pinned_sha = view.pinned.as_ref().and_then(|p| data.refs.iter().find(|r| &r.full == p)).map(|r| r.sha.clone());
        let mut layout = graph::LayoutCursor::new(pinned_sha.as_deref());
        let graph = layout.append(&data.commits);
        let history_cursor = data.has_more.then(|| HistoryCursor { revs: data.history_revs.clone(), offset: data.commits.len(), layout, filter: view.filter.clone() });
        // Windows paths use backslashes; on macOS/Linux the path must stay as-is (a blanket replace turns /Users/x into \Users\x).
        let root_s = if cfg!(windows) { root.to_string_lossy().replace('/', "\\") } else { root.to_string_lossy().into_owned() };
        Ok(RepoView {
            name: git::repo_name(&root),
            undo: engine.undo_info(&root_s),
            path: root_s,
            data,
            graph,
            history_cursor,
        })
    }))
    .await?
}

#[tauri::command]
async fn commit_details(path: String, sha: String, read_id: Option<String>) -> Result<git::Commit, String> {
    blocking(move || read_jobs::scope(read_id, || {
        let output = git::run(Path::new(&path), &["log", "--no-walk", "-1", "--format=%H%x00%P%x00%aN%x00%aE%x00%at%x00%s%x00%b%x00", &sha, "--"]);
        if !output.ok { return Err(output.message()); }
        git::parse_commits(&output.stdout).into_iter().next().ok_or_else(|| "The selected commit is no longer available.".into())
    })).await?
}

#[tauri::command]
async fn commit_files(path: String, sha: String, read_id: Option<String>) -> Result<Vec<git::CommitFile>, String> {
    blocking(move || read_jobs::scope(read_id, || {
        let dir = PathBuf::from(&path);
        let o = git::run(&dir, &["rev-list", "--parents", "-n1", &sha]);
        let parents: Vec<String> = o.stdout.split_whitespace().skip(1).map(|s| s.to_string()).collect();
        let c = git::Commit {
            sha: sha.clone(),
            parents,
            author: String::new(),
            email: String::new(),
            time: 0,
            summary: String::new(),
            body: String::new(),
        };
        git::commit_file_entries(&dir, &c)
    }))
    .await
}

#[tauri::command]
async fn file_diff(
    path: String,
    source: git::DiffSource,
    file: String,
    untracked: bool,
    parent: Option<String>,
    ignore_ws: Option<bool>,
    read_id: Option<String>,
) -> Result<Vec<git::DiffLine>, String> {
    blocking(move || read_jobs::scope(read_id, || git::file_diff_opts(Path::new(&path), &source, &file, untracked, parent.as_deref(), ignore_ws.unwrap_or(false)))).await
}

#[tauri::command]
async fn file_versions(path: String, source: git::DiffSource, file: String, untracked: bool, parent: Option<String>, allow_large: Option<bool>, read_id: Option<String>) -> Result<files::FileVersions, String> {
    blocking(move || read_jobs::scope(read_id, || files::versions_with_limit(Path::new(&path), &source, &file, parent.as_deref(), untracked, allow_large.unwrap_or(false)))).await
}

#[tauri::command]
async fn save_file(path: String, file: String, content: String, expected: Option<String>, utf8: Option<bool>) -> Result<(), String> {
    blocking(move || {
        let operation = operation::lock_for(Path::new(&path));
        let _guard = operation.lock().unwrap_or_else(|e| e.into_inner());
        files::save_checked_mode(Path::new(&path), &file, &content, expected.as_deref(), utf8.unwrap_or(false))
    }).await?
}

#[tauri::command]
async fn file_history(path: String, file: String, cursor: Option<files::FileHistoryCursor>, read_id: Option<String>) -> Result<files::FileHistoryPage, String> {
    read_blocking(read_id, move || files::file_history_page(Path::new(&path), &file, cursor, metrics::FILE_HISTORY_MAX)).await?
}

#[tauri::command]
async fn blame(path: String, file: String, rev: Option<String>, ignore_revs: Option<bool>, read_id: Option<String>) -> Result<Vec<files::BlameLine>, String> {
    blocking(move || read_jobs::scope(read_id, || files::blame(Path::new(&path), &file, rev.as_deref(), ignore_revs.unwrap_or(true)))).await?
}

#[tauri::command]
async fn commit_stats(path: String, shas: Vec<String>, read_id: Option<String>) -> Result<Vec<files::Stats>, String> {
    blocking(move || read_jobs::scope(read_id, || files::stats(Path::new(&path), &shas))).await?
}

#[tauri::command]
async fn range_files(path: String, from: String, to: String, read_id: Option<String>) -> Result<Vec<git::FileChange>, String> {
    blocking(move || read_jobs::scope(read_id, || files::range_files(Path::new(&path), &from, &to))).await?
}

#[tauri::command]
async fn tracked_files(path: String, read_id: Option<String>) -> Result<Vec<String>, String> {
    blocking(move || read_jobs::scope(read_id, || files::tracked_files(Path::new(&path)))).await?
}

#[tauri::command]
async fn read_ssh_public_key(path: String) -> Result<String, String> { blocking(move || accounts::read_public_key(&path)).await? }

#[tauri::command]
async fn rebase_plan(path: String, base: String, read_id: Option<String>) -> Result<serde_json::Value, String> {
    read_blocking(read_id, move || {
        let dir = Path::new(&path);
        let lock = operation::lock_for(dir); let _guard = read_jobs::lock(&lock)?;
        let expected = rewrite::plan_state(dir, &base)?;
        let commits = rewrite::rebase_commits(dir, &base)?;
        Ok(serde_json::json!({ "expected": expected, "commits": commits }))
    }).await?
}

#[tauri::command]
async fn rebase_commits(path: String, base: String, read_id: Option<String>) -> Result<Vec<git::Commit>, String> {
    read_blocking(read_id, move || rewrite::rebase_commits(Path::new(&path), &base)).await?
}

#[tauri::command]
async fn conflict_info(path: String, file: String, read_id: Option<String>) -> Result<rewrite::ConflictInfo, String> {
    read_blocking(read_id, move || rewrite::conflict_info(Path::new(&path), &file)).await
}

#[tauri::command]
async fn gitflow_config(path: String, read_id: Option<String>) -> Result<manage::GitflowConfig, String> {
    read_blocking(read_id, move || manage::gitflow_config(Path::new(&path))).await
}

#[tauri::command]
async fn cleanup_list(path: String, detect_patches: Option<bool>, read_id: Option<String>) -> Result<Vec<manage::CleanupBranch>, String> {
    read_blocking(read_id, move || manage::review_cleanup(Path::new(&path), detect_patches.unwrap_or(false))).await
}

#[tauri::command]
async fn remotes_list(path: String, read_id: Option<String>) -> Result<Vec<manage::Remote>, String> {
    read_blocking(read_id, move || manage::remotes(Path::new(&path))).await
}

#[tauri::command]
async fn submodules_list(path: String, read_id: Option<String>) -> Result<Vec<manage::Submodule>, String> {
    read_blocking(read_id, move || manage::submodules(Path::new(&path))).await
}

#[tauri::command]
async fn lfs_locks(path: String, read_id: Option<String>) -> Result<Vec<manage::LfsLock>, String> { read_blocking(read_id, move || manage::lfs_locks(Path::new(&path))).await? }

#[tauri::command]
async fn lfs_info(path: String, read_id: Option<String>) -> Result<manage::LfsInfo, String> {
    read_blocking(read_id, move || manage::lfs_info(Path::new(&path))).await
}

#[tauri::command]
async fn sparse_info(path: String, read_id: Option<String>) -> Result<manage::SparseInfo, String> {
    read_blocking(read_id, move || manage::sparse_info(Path::new(&path))).await
}

#[tauri::command]
fn gitignore_templates() -> Vec<String> {
    manage::GITIGNORE_TEMPLATES.iter().map(|(n, _)| n.to_string()).collect()
}

#[tauri::command]
async fn account_info(path: Option<String>, read_id: Option<String>) -> Result<accounts::AccountInfo, String> {
    read_blocking(read_id, move || accounts::info(Path::new(&path.unwrap_or_else(|| ".".into())))).await
}

#[tauri::command]
async fn push_review(path: String, read_id: Option<String>) -> Result<actions::PushReview, String> {
    read_blocking(read_id, move || {
        let dir=Path::new(&path);let lock=operation::lock_for(dir);let _guard=read_jobs::lock(&lock)?;
        actions::push_review(dir)
    }).await?
}

#[tauri::command]
async fn set_git_config(path: Option<String>, scope: String, key: String, value: String) -> Result<(), String> {
    blocking(move || accounts::set(Path::new(&path.unwrap_or_else(|| ".".into())), &scope, &key, &value)).await?
}

#[tauri::command]
async fn generate_ssh_key(email: String, name: String, passphrase: String, add_to_agent: bool) -> Result<accounts::GeneratedSshKey, String> {
    blocking(move || accounts::generate_ssh_key(&email, &name, &passphrase, add_to_agent)).await?
}

#[tauri::command]
fn cancel_job(id: String, state: State<'_, AppState>) -> bool {
    state.engine.jobs.cancel(&id)
}

#[tauri::command]
fn term_open(cwd: String, cols: u16, rows: u16, shell: Option<String>, agent_env: Option<HashMap<String, String>>, window: tauri::WebviewWindow, state: State<'_, AppState>) -> Result<u32, String> {
    let output_window = window.clone();
    let exit_window = window.clone();
    state.terminals.open(
        window.label(),
        shell.as_deref().unwrap_or(""),
        &cwd,
        cols,
        rows,
        agent_env.as_ref(),
        std::sync::Arc::new(move |id, data| {
            let _ = output_window.emit_to(tauri::EventTarget::webview_window(output_window.label()), "term-data", serde_json::json!({ "id": id, "data": data }));
        }),
        std::sync::Arc::new(move |id| {
            let _ = exit_window.emit_to(tauri::EventTarget::webview_window(exit_window.label()), "term-exit", id);
        }),
    )
}

#[tauri::command]
fn term_write(id: u32, data: String, window: tauri::WebviewWindow, state: State<'_, AppState>) -> Result<(), String> {
    state.terminals.write(window.label(), id, &data)
}

#[tauri::command]
fn term_ack(id: u32, bytes: usize, window: tauri::WebviewWindow, state: State<'_, AppState>) {
    state.terminals.acknowledge(window.label(), id, bytes);
}

#[tauri::command]
fn term_resize(id: u32, cols: u16, rows: u16, window: tauri::WebviewWindow, state: State<'_, AppState>) -> Result<(), String> {
    state.terminals.resize(window.label(), id, cols, rows)
}

#[tauri::command]
fn term_close(id: u32, window: tauri::WebviewWindow, state: State<'_, AppState>) {
    state.terminals.close(window.label(), id)
}

/// The hosting service of a repository's origin (or first) remote, if it's GitHub or GitLab.
fn remote_of(path: &str, gitlab_host: &str) -> Result<hosting::RemoteRepo, String> {
    let dir = Path::new(path);
    let mut url = git::run(dir, &["remote", "get-url", "origin"]).stdout.trim().to_string();
    if url.is_empty() {
        if let Some(first) = git::run(dir, &["remote"]).stdout.lines().next() {
            url = git::run(dir, &["remote", "get-url", first]).stdout.trim().to_string();
        }
    }
    hosting::parse_remote(&url, gitlab_host).ok_or_else(|| "This repository's remote isn't on a connected service (Preferences > Integrations).".to_string())
}

#[derive(Serialize)]
struct HostingStatus {
    github: Option<String>,
    gitlab: Option<String>,
    github_error: Option<String>,
    gitlab_error: Option<String>,
    ai: bool,
}

#[tauri::command]
async fn hosting_status(gitlab_host: String) -> Result<HostingStatus, String> {
    blocking(move || {
        let check = |p: hosting::Provider, name: &str| -> (Option<String>, Option<String>) {
            if hosting::secret_get(name).is_none() {
                return (None, None);
            }
            match hosting::whoami(&p, &gitlab_host) {
                Ok(u) => (Some(u), None),
                Err(e) => (None, Some(e)),
            }
        };
        let (github, github_error) = check(hosting::Provider::Github, "github-token");
        let (gitlab, gitlab_error) = check(hosting::Provider::Gitlab, "gitlab-token");
        HostingStatus { github, gitlab, github_error, gitlab_error, ai: ai::configured() }
    })
    .await
}

/// Store (or with an empty value, remove) a token or API key in the OS credential store.
#[tauri::command]
async fn set_secret(name: String, value: String) -> Result<(), String> {
    let allowed = hosting::SERVICES.iter().any(|id| hosting::secret_name(id) == name) || ["anthropic-key", "openai-key", "google-key", "ollama-key", "custom-ai-key"].contains(&name.as_str());
    if !allowed { return Err("unknown secret".into()); }
    blocking(move || {
        hosting::save_secret_manually(&name, value.trim())?;
        if name.ends_with("-token") { hosting::sync_git_credentials(""); }
        Ok(())
    }).await?
}

#[tauri::command]
async fn repo_remote(path: String, gitlab_host: String, read_id: Option<String>) -> Result<Option<hosting::RemoteRepo>, String> {
    read_blocking(read_id, move || remote_of(&path, &gitlab_host).ok()).await
}

#[tauri::command]
async fn commit_checks(path: String, gitlab_host: String, sha: String, number: Option<u64>, remote_name: Option<String>, read_id: Option<String>) -> Result<Vec<hosting::CommitCheck>, String> {
    blocking(move || read_jobs::scope(read_id, || {
        let remote = if let Some(name) = remote_name {
            let output = git::run(Path::new(&path), &["remote", "get-url", "--", &name]);
            if !output.ok { return Err(output.message()); }
            hosting::parse_remote(output.stdout.trim(), &gitlab_host).ok_or("This branch's remote is outside the configured hosting services.")?
        } else { remote_of(&path, &gitlab_host)? };
        hosting::commit_checks(&remote, &sha, number)
    })).await?
}

#[tauri::command]
async fn list_prs_page(path: String, gitlab_host: String, cursor: Option<String>, filter: Option<hosting::PullFilter>, read_id: Option<String>) -> Result<pagination::Page<Vec<hosting::PullRequest>>, String> {
    blocking(move || read_jobs::scope(read_id, || hosting::pull_requests_page(&remote_of(&path, &gitlab_host)?, cursor.as_deref(), filter.unwrap_or_default()))).await?
}

#[tauri::command]
async fn list_prs(path: String, gitlab_host: String, read_id: Option<String>) -> Result<Vec<hosting::PullRequest>, String> {
    read_blocking(read_id, move || hosting::pull_requests(&remote_of(&path, &gitlab_host)?)).await?
}

#[tauri::command]
async fn pr_details(path: String, gitlab_host: String, number: u64, read_id: Option<String>) -> Result<hosting::PullRequestDetails, String> {
    read_blocking(read_id, move || hosting::pull_request_details(&remote_of(&path, &gitlab_host)?, number)).await?
}

#[tauri::command]
async fn pr_checkout_target(path: String, gitlab_host: String, number: u64, read_id: Option<String>) -> Result<hosting::PrCheckout, String> {
    read_blocking(read_id, move || {
        let remote = remote_of(&path, &gitlab_host)?;
        hosting::pr_checkout(&remote, &hosting::pull_request(&remote, number)?)
    }).await?
}

/// Issues for a repository: from its own issue tracker choice (Preferences > Issue Tracker), else its host.
#[tauri::command]
async fn issue_comments_page(path: String, gitlab_host: String, number: u64, cursor: Option<String>, read_id: Option<String>) -> Result<pagination::Page<Vec<hosting::Comment>>, String> {
    blocking(move || read_jobs::scope(read_id, || {
        let remote = remote_of(&path, &gitlab_host)?;
        pagination::one_page(cursor, || hosting::issue_comments(&remote, number))
    })).await?
}

#[tauri::command]
async fn list_issues_page(path: String, gitlab_host: String, tracker: Option<hosting::Tracker>, cursor: Option<String>, read_id: Option<String>) -> Result<pagination::Page<Vec<hosting::Issue>>, String> {
    blocking(move || read_jobs::scope(read_id, || pagination::one_page(cursor, || match tracker.filter(|t| !t.kind.is_empty()) {
        Some(t) => hosting::tracker_issues(&t, false),
        None => hosting::issues(&remote_of(&path, &gitlab_host)?),
    }))).await?
}

#[tauri::command]
async fn list_issues(path: String, gitlab_host: String, tracker: Option<hosting::Tracker>, read_id: Option<String>) -> Result<Vec<hosting::Issue>, String> {
    read_blocking(read_id, move || match tracker.filter(|t| !t.kind.is_empty()) {
        Some(t) => hosting::tracker_issues(&t, false),
        None => hosting::issues(&remote_of(&path, &gitlab_host)?),
    })
    .await?
}

#[tauri::command]
async fn create_issue(path: Option<String>, gitlab_host: String, tracker: Option<hosting::Tracker>, title: String, body: String) -> Result<hosting::Issue, String> {
    blocking(move || {
        let r = path.as_deref().and_then(|p| remote_of(p, &gitlab_host).ok());
        hosting::create_issue(r.as_ref(), &tracker.unwrap_or_default(), &title, &body)
    })
    .await?
}

/// Preferences > Integrations: hosts, user names and organisations (tokens stay in the credential store).
#[tauri::command]
fn set_integration_config(config: hosting::Config) {
    hosting::set_config(config);
    std::thread::spawn(|| hosting::sync_git_credentials(""));
}

#[tauri::command]
async fn integration_statuses(ids: Vec<String>) -> Result<Vec<hosting::ServiceStatus>, String> {
    blocking(move || {
        let handles: Vec<_> = ids.into_iter().map(|id| std::thread::spawn(move || hosting::status(&id))).collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    })
    .await
}

/// "Connect": browser sign-in through Git Credential Manager, or the GitHub CLI's sign-in ("gh").
#[tauri::command]
async fn integration_sign_in(id: String, method: String) -> Result<String, String> {
    blocking(move || {
        let token = if method == "gh" { hosting::gh_cli_token()? } else { hosting::browser_sign_in(&id)? };
        hosting::save_sign_in(&id, &token, method != "gh")?;
        hosting::sync_git_credentials("");
        hosting::whoami_id(&id)
    })
    .await?
}

#[tauri::command]
async fn integration_repos_page(id: String, cursor: Option<String>, read_id: Option<String>) -> Result<pagination::Page<Vec<hosting::HostedRepo>>, String> {
    blocking(move || read_jobs::scope(read_id, || pagination::one_page(cursor, || hosting::repositories(&id)))).await?
}

#[tauri::command]
async fn integration_repos(id: String, read_id: Option<String>) -> Result<Vec<hosting::HostedRepo>, String> {
    read_blocking(read_id, move || hosting::repositories(&id)).await?
}

#[tauri::command]
async fn publish_targets(service: String) -> Result<Vec<publishing::Target>, String> {
    blocking(move || publishing::targets(&service)).await?
}

#[tauri::command]
async fn publish_repository(path: String, request: publishing::PublishRequest, state: State<'_, AppState>) -> Result<publishing::PublishResult, String> {
    let engine=state.engine.clone();
    blocking(move || publishing::publish(Path::new(&path), &request, &engine.jobs)).await?
}

#[tauri::command]
async fn resume_publish(path: String, request: publishing::PublishRequest, url: String, state: State<'_, AppState>) -> Result<publishing::PublishResult, String> {
    let engine=state.engine.clone();
    blocking(move || publishing::resume(Path::new(&path), &request, &url, &engine.jobs)).await
}

#[tauri::command]
async fn integration_add_ssh_key(id: String, title: String, key: String) -> Result<(), String> {
    blocking(move || hosting::add_ssh_key(&id, &title, &key)).await?
}

#[tauri::command]
fn integration_ssh_page(id: String) -> String {
    hosting::ssh_keys_page(&id)
}

#[tauri::command]
async fn pr_comments_page(path: String, gitlab_host: String, number: u64, cursor: Option<String>, read_id: Option<String>) -> Result<pagination::Page<Vec<hosting::Comment>>, String> {
    blocking(move || read_jobs::scope(read_id, || pagination::one_page(cursor, || hosting::comments(&remote_of(&path, &gitlab_host)?, number)))).await?
}

#[tauri::command]
async fn launchpad_page(service: String, cursor: Option<String>, read_id: Option<String>) -> Result<pagination::Page<hosting::Launchpad>, String> {
    blocking(move || read_jobs::scope(read_id, || hosting::launchpad_page(&service, cursor.as_deref()))).await?
}

#[tauri::command]
async fn pr_comments(path: String, gitlab_host: String, number: u64, read_id: Option<String>) -> Result<Vec<hosting::Comment>, String> {
    read_blocking(read_id, move || hosting::comments(&remote_of(&path, &gitlab_host)?, number)).await?
}

#[tauri::command]
async fn pr_comment(path: String, gitlab_host: String, number: u64, body: String) -> Result<(), String> {
    blocking(move || hosting::add_comment(&remote_of(&path, &gitlab_host)?, number, &body)).await?
}

#[tauri::command]
async fn create_pr(path: String, gitlab_host: String, source: String, target: String, title: String, body: String, draft: bool) -> Result<hosting::PullRequest, String> {
    blocking(move || {
        let (remote, source) = hosting::pull_request_destination(Path::new(&path), &source, &gitlab_host)?;
        hosting::create_pr(&remote, &source, &target, &title, &body, draft)
    }).await?
}

#[tauri::command]
async fn cancel_pr_merge(path: String, gitlab_host: String, number: u64) -> Result<bool, String> {
    blocking(move || Ok(hosting_api::cancel_merge(&remote_of(&path, &gitlab_host)?, number))).await?
}

#[tauri::command]
async fn merge_pr(path: String, gitlab_host: String, number: u64, method: String, expected: Option<String>, squash_message: Option<String>) -> Result<(), String> {
    blocking(move || hosting::merge_pr(&remote_of(&path, &gitlab_host)?, number, &method, expected.as_deref(), squash_message.as_deref())).await?
}

#[tauri::command]
async fn pull_review_data(path: String, gitlab_host: String, number: u64, expected: String, read_id: Option<String>) -> Result<hosting::ReviewData, String> {
    blocking(move || read_jobs::scope(read_id, || hosting::review_data(&remote_of(&path, &gitlab_host)?, number, &expected))).await?
}

#[tauri::command]
async fn submit_pull_review(path: String, gitlab_host: String, number: u64, expected: String, action: hosting::ReviewAction) -> Result<(), String> {
    blocking(move || hosting::submit_review(&remote_of(&path, &gitlab_host)?, number, &expected, action)).await?
}

#[tauri::command]
async fn delete_merged_source(path: String, gitlab_host: String, number: u64, expected: String, remote: bool, state: State<'_, AppState>) -> Result<(), String> {
    let engine = state.engine.clone();
    blocking(move || {
        let target = remote_of(&path, &gitlab_host)?;
        let pr = hosting::merged_source(&target, number, &expected)?;
        if !remote {
            let result = engine.perform(&path, actions::Action::DeleteMergedLocal { name: pr.source, expected });
            return if result.ok { Ok(()) } else { Err(result.message) };
        }
        let source = hosting::parse_remote(&pr.source_url, &gitlab_host)
            .filter(|source| source.service == target.service && source.host == target.host)
            .ok_or("The hosting service did not return a verified HTTPS source repository.")?;
        let dir = Path::new(&path);
        let reference = format!("refs/heads/{}", pr.source);
        if !git::run(dir, &["check-ref-format", &reference]).ok { return Err("The source branch name is invalid.".into()); }
        let head = git::run(dir, &["ls-remote", "--symref", "--", &pr.source_url, "HEAD"]);
        if !head.ok { return Err(head.message()); }
        let default = head.stdout.lines().find_map(|line| line.strip_prefix("ref: ").and_then(|line| line.split_once('\t')).filter(|(_, name)| *name == "HEAD").map(|(reference, _)| reference));
        if default.is_none() || default == Some(reference.as_str()) {
            return Err(format!("Could not confirm that {} is safe to delete from {}.", pr.source, source.web));
        }
        let guard = format!("--force-with-lease={reference}:{expected}");
        let refspec = format!(":{reference}");
        let operation = operation::lock_for(dir);
        let _operation = operation.lock().unwrap_or_else(|error| error.into_inner());
        let deleted = git::run(dir, &["push", &guard, "--", &pr.source_url, &refspec]);
        if deleted.ok { Ok(()) } else { Err(deleted.message()) }
    }).await?
}

#[tauri::command]
async fn launchpad(gitlab_host: String, read_id: Option<String>) -> Result<hosting::Launchpad, String> {
    read_blocking(read_id, move || hosting::launchpad(&gitlab_host)).await
}

#[tauri::command]
fn issue_branch_name(number: u64, title: String) -> String {
    hosting::branch_for_issue(number, &title)
}

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("Only web links can be opened".into());
    }
    // Open a web link with the system: ShellExecuteExW "open".
    shell::open_external(&url)
}

#[tauri::command]
async fn ai_file_preview(path: String, kind: ai_context::PreviewKind) -> Result<ai_context::Preview, String> {
    blocking(move || ai_context::preview(Path::new(&path), kind)).await?
}

#[tauri::command]
async fn ai_commit_message(path: String, sha: Option<String>, config: ai::AiConfig, preview: Option<u32>, read_id: Option<String>) -> Result<(String, String), String> {
    ai_blocking(read_id, move || {
        let dir = Path::new(&path);
        let files = ai_context::take_preview(dir, ai_context::PreviewKind::Commit, preview)?;
        ai::with_config(config, || ai::commit_message(dir, sha.as_deref(), &files.text))
    }).await?
}

#[tauri::command]
async fn ai_stash_message(path: String, config: ai::AiConfig, preview: Option<u32>, read_id: Option<String>) -> Result<String, String> {
    ai_blocking(read_id, move || {
        let dir = Path::new(&path);
        let files = ai_context::take_preview(dir, ai_context::PreviewKind::Stash, preview)?;
        ai::with_config(config, || ai::stash_message(dir, &files.text))
    }).await?
}

#[tauri::command]
async fn ai_explain_commit(path: String, sha: String, config: ai::AiConfig, read_id: Option<String>) -> Result<String, String> {
    ai_blocking(read_id, move || ai::with_config(config, || ai::explain_commit(Path::new(&path), &sha))).await?
}

#[tauri::command]
async fn ai_explain_branch(path: String, branch: String, base: Option<String>, config: ai::AiConfig, read_id: Option<String>) -> Result<String, String> {
    ai_blocking(read_id, move || ai::with_config(config, || {
        let base = base.or_else(|| extras::default_target(Path::new(&path))).ok_or("No branch to compare with")?;
        ai::explain_branch(Path::new(&path), &branch, &base)
    }))
    .await?
}

#[tauri::command]
fn set_ai_config(config: ai::AiConfig) {
    ai::set_config(config);
}

#[tauri::command]
async fn ai_test_connection(config: ai::AiConfig, read_id: Option<String>) -> Result<String, String> {
    ai_blocking(read_id, move || ai::with_config(config, ai::test_connection)).await?
}

#[tauri::command]
async fn ai_list_models(config: ai::AiConfig) -> Result<Vec<ai_models::Model>, String> {
    blocking(move || ai_models::list(&config)).await?
}

#[tauri::command]
async fn ai_find_program(provider: String) -> Result<ai_cli::DetectedProgram, String> {
    let agent = ai_cli::Agent::from_provider(&provider).ok_or("This provider does not use a local program.")?;
    blocking(move || ai_cli::detect_program(agent)).await?
}

#[tauri::command]
async fn ai_pull_request(path: String, source: String, target: String, config: ai::AiConfig, read_id: Option<String>) -> Result<(String, String), String> {
    ai_blocking(read_id, move || ai::with_config(config, || ai::pull_request_text(Path::new(&path), &source, &target))).await?
}

#[tauri::command]
async fn ai_resolve_conflict(base: String, ours: String, theirs: String, file: String, config: ai::AiConfig, read_id: Option<String>) -> Result<String, String> {
    ai_blocking(read_id, move || ai::with_config(config, || ai::resolve_conflict(&base, &ours, &theirs, &file))).await?
}

#[tauri::command]
async fn ai_compose(path: String, config: ai::AiConfig, preview: Option<u32>, read_id: Option<String>) -> Result<Vec<ai::ComposedCommit>, String> {
    ai_blocking(read_id, move || {
        let dir = Path::new(&path);
        let files = ai_context::take_preview(dir, ai_context::PreviewKind::Compose, preview)?;
        ai::with_config(config, || ai::compose(dir, files))
    }).await?
}

/// Save a patch (uncommitted changes when `shas` is empty, else those commits) to a file the person picks.
#[tauri::command]
async fn save_patch(path: String, shas: Vec<String>, name: String) -> Result<Option<String>, String> {
    let patch = blocking(move || -> Result<_, String> {
        let text = extras::make_patch(Path::new(&path), &shas)?;
        let file = rfd::FileDialog::new().set_title("Save patch").set_file_name(&name).add_filter("Patch", &["patch", "diff"]).save_file();
        Ok(file.map(|file| (file, text)))
    })
    .await??;
    let Some((file, text)) = patch else {
        return Ok(None);
    };
    tokio::fs::write(&file, text).await.map_err(err)?;
    Ok(Some(file.to_string_lossy().into_owned()))
}

#[tauri::command]
async fn pick_patch_file() -> Result<Option<String>, String> {
    blocking(|| rfd::FileDialog::new().set_title("Apply a patch").add_filter("Patch", &["patch", "diff"]).pick_file().map(|p| p.to_string_lossy().into_owned())).await
}

#[tauri::command]
async fn conflict_check(path: String, target: Option<String>, read_id: Option<String>) -> Result<extras::ConflictCheck, String> {
    read_blocking(read_id, move || extras::conflict_check(Path::new(&path), target.as_deref())).await
}

/// Just the file lists (one `git status`), for staging and working-folder edits.
#[tauri::command]
async fn repo_status(path: String, read_id: Option<String>) -> Result<git::WorkStatus, String> {
    blocking(move || read_jobs::scope(read_id, || git::try_status(Path::new(&path)))).await?
}

/// Hand the tokens of connected services to Git (HTTPS push / pull / fetch / clone).
#[tauri::command]
async fn sync_git_credentials(gitlab_host: String) -> Result<(), String> {
    blocking(move || hosting::sync_git_credentials(&gitlab_host)).await
}

#[tauri::command]
async fn remote_auth(path: String, gitlab_host: String, read_id: Option<String>) -> Result<hosting::RemoteAuth, String> {
    read_blocking(read_id, move || hosting::remote_auth(Path::new(&path), &gitlab_host)).await
}

/// Preferences that change how Git runs: executable, SSH key / agent, extended logging.
#[tauri::command]
fn set_runtime_settings(settings: prefs::RuntimeSettings) {
    prefs::set_runtime(settings);
}

#[tauri::command]
async fn tool_versions(gpg_program: String) -> Result<prefs::Versions, String> {
    blocking(move || prefs::versions(&gpg_program)).await
}

#[tauri::command]
async fn git_config_get(path: Option<String>, scope: String, keys: Vec<String>, read_id: Option<String>) -> Result<HashMap<String, String>, String> {
    read_blocking(read_id, move || prefs::config_get(path.as_deref().map(Path::new), &scope, &keys)).await
}

#[tauri::command]
async fn gpg_generate_key(program: String, name: String, email: String, passphrase: String) -> Result<String, String> {
    blocking(move || prefs::gpg_generate(&program, &name, &email, &passphrase)).await?
}

#[tauri::command]
async fn gpg_export_key(program: String, key: String) -> Result<String, String> {
    blocking(move || prefs::gpg_export(&program, &key)).await?
}

#[tauri::command]
async fn forget_credentials(hosts: Vec<String>) -> Result<usize, String> {
    blocking(move || prefs::forget_credentials(&hosts)).await
}

#[tauri::command]
async fn orig_cleanup_candidates(path: String) -> Result<Vec<prefs::OrigCandidate>, String> { blocking(move || prefs::orig_cleanup_candidates(Path::new(&path))).await }

#[tauri::command]
async fn delete_orig_files(path: String, candidates: Vec<prefs::OrigCandidate>) -> Result<Vec<String>, String> {
    blocking(move || {
        let dir = Path::new(&path); let lock = operation::lock_for(dir); let _guard = lock.lock().unwrap_or_else(|error| error.into_inner());
        prefs::delete_orig_selected(dir, &candidates)
    }).await?
}

#[tauri::command]
async fn pick_file(title: String) -> Result<Option<String>, String> {
    blocking(move || rfd::FileDialog::new().set_title(&title).pick_file().map(|p| p.to_string_lossy().into_owned())).await
}

#[tauri::command]
async fn repo_summary(path: String) -> Result<extras::RepoSummary, String> {
    blocking(move || extras::repo_summary(Path::new(&path))).await
}

#[tauri::command]
async fn pr_default_target(path: String, branch: String, read_id: Option<String>) -> Result<Option<String>, String> {
    read_blocking(read_id, move || extras::default_branch_for(Path::new(&path), &branch).map(|(_, branch)| branch)).await
}

#[tauri::command]
async fn pr_templates(path: String, read_id: Option<String>) -> Result<Vec<extras::PullRequestTemplate>, String> {
    read_blocking(read_id, move || extras::pr_templates(Path::new(&path))).await
}

#[tauri::command]
async fn range_diff(path: String, before: String, after: String, read_id: Option<String>) -> Result<String, String> {
    read_blocking(read_id, move || extras::range_diff(Path::new(&path), &before, &after)).await?
}

#[tauri::command]
async fn agent_sessions(path: String, paths: Option<Vec<String>>, claude_dir: Option<String>, codex_dir: Option<String>) -> Result<Vec<agents::AgentSession>, String> {
    blocking(move || {
        let mut roots = paths.unwrap_or_default();
        if !roots.contains(&path) { roots.push(path); }
        agents::sessions(&roots, claude_dir.as_deref(), codex_dir.as_deref())
    }).await
}

/// Whether automatic updates are set up in this build (a public key is configured), and where they come from.
#[tauri::command]
fn update_config(app: tauri::AppHandle) -> serde_json::Value {
    let u = app.config().plugins.0.get("updater").cloned().unwrap_or_default();
    let configured = u["pubkey"].as_str().map(|k| !k.trim().is_empty()).unwrap_or(false);
    serde_json::json!({ "configured": configured, "endpoint": u["endpoints"][0] })
}

/// File > Open Repo in <editor> (Preferences > External Tools > Editor); with `file`, open that file.
#[tauri::command]
fn open_in_editor(path: String, file: Option<String>, tool: Option<String>, exe: Option<String>, file_args: Option<String>, repo_args: Option<String>) -> Result<(), String> {
    external::open_editor(&path, file.as_deref(), tool.as_deref().unwrap_or("vscode"), exe.as_deref().unwrap_or(""), file_args.as_deref().unwrap_or(""), repo_args.as_deref().unwrap_or(""))
}

#[tauri::command]
async fn run_action(path: String, action: Action, state: State<'_, AppState>) -> Result<ActionResult, String> {
    let engine = state.engine.clone();
    blocking(move || engine.perform(&path, action)).await
}

#[tauri::command]
fn activity_log_snapshot(version: Option<u64>, state: State<'_, AppState>) -> Option<(u64, Vec<git::LogEntry>)> { state.engine.log_snapshot(version) }

#[tauri::command]
fn activity_log(state: State<'_, AppState>) -> Vec<git::LogEntry> {
    state.engine.log_entries()
}

#[tauri::command]
async fn sample_repo() -> Result<String, String> {
    blocking(|| sample::ensure_sample().map(|p| p.to_string_lossy().into_owned())).await?
}

#[tauri::command]
async fn conflict_sample() -> Result<String, String> {
    blocking(|| sample::ensure_conflict_sample().map(|p| p.to_string_lossy().into_owned())).await?
}

#[tauri::command]
async fn discarded_changes(path: String, read_id: Option<String>) -> Result<Vec<discard_backups::DiscardBackup>, String> {
    read_blocking(read_id, move || discard_backups::list(Path::new(&path))).await?
}

#[tauri::command]
async fn validate_ref_name(path: String, kind: String, name: String, read_id: Option<String>) -> Result<(), String> {
    read_blocking(read_id, move || actions::validate_ref_name(Path::new(&path), &kind, &name)).await?
}

#[tauri::command]
async fn head_is_upstream(path: String, sha: String, read_id: Option<String>) -> Result<bool, String> {
    read_blocking(read_id, move || {
        if !matches!(sha.len(), 40 | 64) || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) { return Ok(false); }
        let dir = Path::new(&path);
        let upstream = git::run(dir, &["rev-parse", "--verify", "@{upstream}"]);
        if !upstream.ok { return Ok(false); }
        Ok(git::run(dir, &["merge-base", "--is-ancestor", &sha, upstream.stdout.trim()]).ok)
    }).await?
}

#[tauri::command]
async fn last_commit_message(path: String, read_id: Option<String>) -> Result<(String, String), String> {
    read_blocking(read_id, move || git::last_commit_message(Path::new(&path))).await
}

#[tauri::command]
async fn pick_folder(title: String) -> Result<Option<String>, String> {
    blocking(move || rfd::FileDialog::new().set_title(&title).pick_folder().map(|p| p.to_string_lossy().into_owned())).await
}

#[tauri::command]
async fn init_repo(parent: String, name: String, options: Option<manage::InitOptions>, state: State<'_, AppState>) -> Result<manage::InitOutcome, String> {
    let engine = state.engine.clone();
    let name = name.trim();
    if name.is_empty() || Path::new(name).components().count() != 1 || !matches!(Path::new(name).components().next(), Some(std::path::Component::Normal(_))) {
        return Err("Enter a folder name, without parent directories.".into());
    }
    let dir = PathBuf::from(parent).join(name);
    let nonempty = match tokio::fs::read_dir(&dir).await {
        Ok(mut entries) => entries.next_entry().await.map_err(err)?.is_some(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(err(error)),
    };
    if nonempty {
        return Err("Choose an empty folder for a new repository. Existing files were not changed.".into());
    }
    let name = name.to_owned();
    tokio::fs::create_dir_all(&dir).await.map_err(err)?;
    blocking(move || {
        let opts = options.unwrap_or_default();
        let branch = if opts.branch.trim().is_empty() { "main".to_string() } else { opts.branch.trim().to_string() };
        let o = engine.git(&dir, &["init", "-b", &branch]);
        if o.ok {
            let f = manage::init_files(&dir, name.trim(), &opts, &|a: &[&str]| engine.git(&dir, a));
            if !f.ok {
                return Err(f.message());
            }
            Ok(manage::initialize_lfs(&dir, opts.lfs, &|args| engine.git(&dir, args)))
        } else {
            Err(o.message())
        }
    })
    .await?
}

/// What the Clone dialog can ask for besides the address and folder.
#[derive(serde::Deserialize, Default)]
struct CloneOptions {
    /// Only the newest this-many commits (--depth).
    depth: Option<u32>,
    /// Only commits newer than this date, YYYY-MM-DD (--shallow-since).
    since: Option<String>,
    /// Clone just this branch (--single-branch --branch).
    branch: Option<String>,
    /// Extra flags parsed as quoted argument fields without shell expansion.
    custom_flags: Option<String>,
    /// Start with a sparse checkout (--sparse) and check out only `sparse_paths`.
    sparse: Option<bool>,
    sparse_paths: Option<Vec<String>>,
    /// Leave file contents out of the first download (--filter blob:none).
    blobless: Option<bool>,
    folder_name: Option<String>,
    initialize_submodules: Option<bool>,
}

/// The `git clone` arguments in a fixed order: branch, depth, progress, since, custom flags, sparse, filter.
fn clone_args(o: &CloneOptions) -> Result<Vec<String>, String> {
    let mut a: Vec<String> = Vec::new();
    if let Some(b) = o.branch.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
        a.extend(["--single-branch".to_string(), "--branch".to_string(), b.to_string()]);
    }
    if let Some(d) = o.depth.filter(|d| *d > 0) {
        a.extend(["--depth".to_string(), d.to_string()]);
    }
    a.push("--progress".into());
    if let Some(since) = o.since.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        a.extend(["--shallow-since".to_string(), since.to_string()]);
    }
    if o.initialize_submodules == Some(true) { a.push("--recurse-submodules".into()); }
    if let Some(flags) = o.custom_flags.as_deref() {
        a.extend(external::argument_fields(flags)?);
    }
    if o.sparse == Some(true) {
        a.push("--sparse".into());
    }
    if o.blobless == Some(true) {
        a.extend(["--filter".to_string(), "blob:none".to_string()]);
    }
    Ok(a)
}

fn valid_clone_folder(name: &str) -> bool {
    !name.is_empty() && !matches!(name, "." | "..") && !name.ends_with([' ', '.']) &&
        !name.chars().any(|character| character.is_control() || matches!(character, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')) &&
        (!cfg!(windows) || {
            let base = name.split('.').next().unwrap_or_default().to_ascii_uppercase();
            !matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL") &&
                !["COM", "LPT"].iter().any(|prefix| base.strip_prefix(prefix).is_some_and(|suffix| matches!(suffix, "1"|"2"|"3"|"4"|"5"|"6"|"7"|"8"|"9"|"¹"|"²"|"³")))
        })
}

fn clone_folder_base(url: &str) -> String {
    let parsed = url.contains("://").then(|| tauri::Url::parse(url).ok()).flatten();
    let path = parsed.as_ref().map(|url| url.path()).unwrap_or(url);
    let part = path.trim_end_matches(['/', '\\']).rsplit(['/', '\\', ':']).next().unwrap_or_default();
    let name = part.strip_suffix(".git").unwrap_or(part);
    if valid_clone_folder(name) { name.into() } else { "repository".into() }
}

fn available_clone_folder(parent: &Path, base: &str) -> Result<String, String> {
    for n in 1..=1000 {
        let name = if n == 1 { base.to_owned() } else { format!("{base}-{n}") };
        match std::fs::symlink_metadata(parent.join(&name)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(name),
            Err(error) => return Err(error.to_string()),
            Ok(_) => {}
        }
    }
    Err("No available clone folder name was found. Choose a different name or parent folder.".into())
}

#[tauri::command]
async fn suggest_clone_folder(url: String, parent: String) -> Result<String, String> {
    blocking(move || available_clone_folder(Path::new(&parent), &clone_folder_base(url.trim()))).await?
}

#[tauri::command]
async fn clone_repo(url: String, parent: String, options: Option<CloneOptions>, state: State<'_, AppState>) -> Result<String, String> {
    let url = url.trim().to_owned();
    if url.is_empty() || url.starts_with('-') || url.contains(['\r', '\n', '\0']) {
        return Err("Enter a repository URL or path, without command options.".into());
    }
    let engine = state.engine.clone();
    let parent = PathBuf::from(parent);
    tokio::fs::create_dir_all(&parent).await.map_err(err)?;
    blocking(move || {
        let opts = options.unwrap_or_default();
        let base = clone_folder_base(&url);
        let name = match opts.folder_name.as_deref().map(str::trim).filter(|name| !name.is_empty()) {
            Some(name) if !valid_clone_folder(name) => return Err("Use a single valid folder name inside the selected parent folder.".into()),
            Some(name) => {
                let available = available_clone_folder(&parent, name)?;
                if available != name { return Err(format!("Folder {name} already exists. Try {available}.")); }
                name.to_owned()
            }
            None => available_clone_folder(&parent, &base)?,
        };
        let dest = parent.join(&name);
        let dest_s = dest.to_string_lossy().into_owned();
        let mut args: Vec<String> = vec!["clone".into()];
        let clone_options = clone_args(&opts)?;
        let no_worktree = clone_options.iter().any(|flag| matches!(flag.as_str(), "--bare" | "--mirror" | "--no-checkout" | "-n"));
        let defer_checkout = !no_worktree && !manage::git_lfs_available(&parent);
        args.extend(clone_options.iter().cloned());
        if defer_checkout { args.push("--no-checkout".into()); }
        args.extend(["--".into(), url]);
        args.push(dest_s);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let o = engine.job("clone", "clone", &parent, &refs);
        if !o.ok {
            return Err(o.message());
        }
        if defer_checkout {
            if manage::tree_uses_lfs(&dest, "HEAD")? {
                return Err(format!("{} The Git history was downloaded to {}.", manage::LFS_REQUIRED, dest.display()));
            }
            let head = git::run(&dest, &["rev-parse", "--verify", "HEAD"]);
            if head.ok {
                let out = engine.git(&dest, &["checkout", "-q", "HEAD"]);
                if !out.ok { return Err(out.message()); }
            }
            let requested: Vec<_> = clone_options.iter().filter_map(|arg| arg.strip_prefix("--recurse-submodules=")).collect();
            if opts.initialize_submodules == Some(true) || !requested.is_empty() || clone_options.iter().any(|arg| arg == "--recurse-submodules" || arg == "--recursive") {
                let mut update = vec!["submodule", "update", "--init", "--recursive"];
                if clone_options.iter().any(|arg| arg == "--shallow-submodules") { update.push("--depth=1"); }
                if clone_options.iter().any(|arg| arg == "--remote-submodules") { update.push("--remote"); }
                if !requested.is_empty() { update.push("--"); update.extend(requested); }
                let out = engine.job("clone", "submodules", &dest, &update);
                if !out.ok { return Err(format!("The repository was cloned, but its submodules could not be initialized: {}", out.message())); }
            }
        }
        if opts.sparse == Some(true) && !no_worktree {
            // Only the chosen paths are checked out; with none, only the files in the top folder.
            let mut set: Vec<String> = vec!["sparse-checkout".into(), "set".into(), "--no-cone".into()];
            let paths: Vec<String> = opts.sparse_paths.unwrap_or_default().into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect();
            if !paths.is_empty() {
                set.push("--".into());
                set.extend(paths);
            } else { set.extend(["/*".into(), "!/*/".into()]); }
            let refs: Vec<&str> = set.iter().map(String::as_str).collect();
            let s = engine.git(&dest, &refs);
            if !s.ok {
                return Err(format!("The repository was cloned, but the sparse checkout could not be set: {}", s.message()));
            }
        }
        Ok(dest.to_string_lossy().into_owned())
    })
    .await?
}

/// File > Open External Terminal (Preferences > External Tools > Terminal).
#[tauri::command]
async fn open_terminal(path: String, tool: Option<String>, exe: Option<String>, args: Option<String>) -> Result<(), String> {
    blocking(move || external::open_terminal(&path, tool.as_deref().unwrap_or("default"), exe.as_deref().unwrap_or(""), args.as_deref().unwrap_or(""))).await?
}

#[tauri::command]
fn open_file_manager(path: String) -> Result<(), String> {
    // A folder opens with the "explore" verb.
    shell::open_path(Path::new(&path))
}

/// File menu "Open file in default program": the system opens it with its usual app.
#[tauri::command]
fn open_default(path: String, file: String) -> Result<(), String> {
    let p = files::repo_file(Path::new(&path), &file)?;
    // Default action; "Failed to open path" on failure.
    if !p.exists() {
        return Err(shell::OPEN_PATH_FAILED.into());
    }
    shell::open_path(&p)
}

/// File menu "Show in File Manager": opens the folder with the file selected (its folder when the file is gone).
#[tauri::command]
async fn reveal_file(path: String, file: String) -> Result<(), String> {
    blocking(move || {
        let p = files::repo_file(Path::new(&path), &file)?;
        if !p.exists() { return shell::open_path(p.parent().unwrap_or(Path::new(&path))); }
        shell::show_item_in_folder(&p)
    }).await?
}

/// File menu "Copy file contents to clipboard": the working copy, or the file as it was in `rev`.
#[tauri::command]
async fn file_text(path: String, file: String, rev: Option<String>, read_id: Option<String>) -> Result<String, String> {
    read_blocking(read_id, move || files::file_text(Path::new(&path), &file, rev.as_deref())).await?
}

fn settings_file() -> PathBuf {
    sample::data_dir().join("settings.json")
}

fn window_settings_file(label: &str) -> PathBuf {
    if label == "main" { settings_file() }
    else { sample::data_dir().join("windows").join(format!("{label}.json")) }
}

#[tauri::command]
fn load_settings(state: State<'_, AppState>, window: tauri::WebviewWindow) -> Result<serde_json::Value, String> {
    if state.screenshot.is_some() { return Ok(serde_json::Value::Null); }
    let file = window_settings_file(window.label());
    if window.label() != "main" && !file.exists() && !file.with_extension("json.bak").exists() {
        let desktop = settings_file::load(&settings_file())?;
        return Ok(serde_json::json!({ "settings": desktop["settings"], "profiles": desktop["profiles"], "activeProfile": desktop["activeProfile"] }));
    }
    settings_file::load(&file)
}

#[tauri::command]
fn save_settings(settings: serde_json::Value, state: State<'_, AppState>, window: tauri::WebviewWindow) -> Result<(), String> {
    if state.screenshot.is_some() { return Ok(()); }
    settings_file::save(&window_settings_file(window.label()), &settings)
}

#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum LaunchIntent {
    Repository { path: String },
    Clone { url: String },
}

fn clone_launch(value: &str) -> Option<LaunchIntent> {
    if value.len() > 8192 || value.chars().any(char::is_control) { return None; }
    let launch = tauri::Url::parse(value).ok()?;
    if launch.scheme() != "gitcraft" || launch.host_str() != Some("clone") || !matches!(launch.path(), "" | "/") ||
        launch.port().is_some() || !launch.username().is_empty() || launch.password().is_some() || launch.fragment().is_some() {
        return None;
    }
    let mut fields = launch.query_pairs();
    let (name, value) = fields.next()?;
    if name != "url" || fields.next().is_some() || value.chars().any(char::is_control) { return None; }
    let url = tauri::Url::parse(&value).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() || !url.username().is_empty() ||
        url.password().is_some() || url.query().is_some() || url.fragment().is_some() || matches!(url.path(), "" | "/") {
        return None;
    }
    Some(LaunchIntent::Clone { url: url.into() })
}

fn startup_paths(args: &[String], cwd: &Path) -> Vec<LaunchIntent> {
    let args = args.get(1..).unwrap_or_default();
    let args = if args.first().is_some_and(|arg| arg == "--") { &args[1..] } else { args };
    let Some(argument) = args.first().filter(|arg| !arg.starts_with('-')) else { return Vec::new(); };
    if argument.starts_with("gitcraft:") { return clone_launch(argument).into_iter().collect(); }
    let argument = explorer_argument(argument);
    let path = cwd.join(argument.as_ref());
    if path.is_dir() { vec![LaunchIntent::Repository { path: path.to_string_lossy().into_owned() }] } else { Vec::new() }
}

/// Explorer passes the folder as `"%1"`. For a drive root that is `"C:\"`, and Windows argument
/// parsing reads the `\"` as an escaped quote, so the program receives `C:"`. Restore the separator.
fn explorer_argument(argument: &str) -> std::borrow::Cow<'_, str> {
    match argument.strip_suffix('"') {
        Some(folder) if cfg!(windows) => std::borrow::Cow::Owned(format!("{folder}\\")),
        _ => std::borrow::Cow::Borrowed(argument),
    }
}

fn deliver_launch(app: &tauri::AppHandle, paths: Vec<LaunchIntent>) {
    let target = app.get_webview_window("main").or_else(|| app.webview_windows().into_values().next());
    let Some(window) = target else { return; };
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
    if paths.is_empty() { return; }
    if let Some(state) = app.try_state::<AppState>() {
        state.launch_paths.lock().unwrap_or_else(|error| error.into_inner())
            .entry(window.label().to_owned()).or_default().extend(paths);
        let _ = window.emit_to(tauri::EventTarget::webview_window(window.label()), "open-launch-paths", ());
    }
}

#[cfg(test)]
#[test]
fn feature_clone_launch_requires_a_reviewable_web_url() {
    assert!(matches!(clone_launch("gitcraft://clone?url=https%3A%2F%2Fexample.invalid%2Fteam%2Fproject.git"), Some(LaunchIntent::Clone { .. })));
    for invalid in [
        "gitcraft://clone?url=file%3A%2F%2F%2Fproject",
        "gitcraft://clone?url=https%3A%2F%2Fname%40example.invalid%2Fproject",
        "gitcraft://clone?url=https%3A%2F%2Fexample.invalid%2Fproject%3Ftoken%3Dsecret",
        "gitcraft://clone?url=https%3A%2F%2Fexample.invalid%2Fproject&url=other",
    ] { assert!(clone_launch(invalid).is_none()); }
}

#[cfg(test)]
#[test]
fn explorer_drive_roots_keep_their_separator() {
    if cfg!(windows) {
        assert_eq!(explorer_argument("C:\""), "C:\\");
        assert_eq!(explorer_argument("D:\\repo\""), "D:\\repo\\");
    } else {
        assert_eq!(explorer_argument("folder\""), "folder\"");
    }
    assert_eq!(explorer_argument(r"C:\repo"), r"C:\repo");
}

#[tauri::command]
async fn open_repository_window(path: String, state: State<'_, AppState>, app: tauri::AppHandle) -> Result<(), String> {
    if state.screenshot.is_some() { return Err("Repository windows are unavailable in verification mode.".into()); }
    let root = blocking(move || git::repo_root(Path::new(&path))).await?
        .ok_or("Choose the folder of an existing Git repository.")?;
    let path = root.to_string_lossy().into_owned();
    let label = format!("repository-{}", files::content_token(Some(path.as_bytes())));
    if let Some(window) = app.get_webview_window(&label) {
        let _ = window.unminimize();
        let _ = window.show();
        return window.set_focus().map_err(err);
    }
    state.launch_paths.lock().unwrap_or_else(|error| error.into_inner())
        .insert(label.clone(), vec![LaunchIntent::Repository { path }]);
    let result = tauri::WebviewWindowBuilder::new(&app, &label, tauri::WebviewUrl::App("index.html".into()))
        .title(format!("{} – GitCraft", git::repo_name(&root)))
        .inner_size(930.0, 669.0).min_inner_size(760.0, 520.0).center().build();
    if result.is_err() {
        state.launch_paths.lock().unwrap_or_else(|error| error.into_inner()).remove(&label);
    }
    result.map(|_| ()).map_err(err)
}

#[tauri::command]
fn take_launch_paths(state: State<'_, AppState>, window: tauri::WebviewWindow) -> Vec<LaunchIntent> {
    state.launch_paths.lock().unwrap_or_else(|error| error.into_inner()).remove(window.label()).unwrap_or_default()
}

#[tauri::command]
fn desktop_validation_state(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    if state.screenshot.is_none() || state.shot_view.as_deref() != Some("validation") { return Err("Desktop validation mode is not active".into()); }
    let watchers = state.watchers.lock().unwrap();
    Ok(serde_json::json!({"read_slots": read_jobs::active_count(), "watchers": watchers.len(), "pending_watches": watchers.values().filter(|s| s.watcher.is_none()).count(), "terminals": state.terminals.pending_output()}))
}

#[tauri::command]
fn desktop_validation_accounts(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    if state.screenshot.is_none() || state.shot_view.as_deref() != Some("validation") { return Err("Desktop validation mode is not active".into()); }
    Ok(serde_json::json!({"gitlab": hosting::secret_get("gitlab-token").is_some(), "gitlab_self": hosting::secret_get("gitlab-self-token").is_some()}))
}

#[tauri::command]
fn acknowledge_job(id: String, state: State<'_, AppState>) { state.engine.jobs.acknowledge(&id); }

#[tauri::command]
fn cancel_read(id: String) { read_jobs::cancel(&id); }

/// Watch a repository and tell the window when something changed (rules in src/watch.rs).
#[tauri::command]
async fn watch_repo(path: String, window: tauri::WebviewWindow, state: State<'_, AppState>) -> Result<(), String> {
    let generation = Arc::new(());
    let key = (window.label().to_owned(), path.clone());
    static NEXT_WATCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let read_id = format!("watch-{}", NEXT_WATCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let watchers = state.watchers.clone();
    {
        let mut map = watchers.lock().unwrap();
        if map.contains_key(&key) { return Ok(()); }
        map.insert(key.clone(), WatchSlot { read_id: read_id.clone(), generation: generation.clone(), watcher: None });
    }
    blocking(move || {
        let payload_path = path.clone();
        let result = read_jobs::scope(Some(read_id), || watch::start(Path::new(&path), move |full| {
            let _ = window.emit_to(tauri::EventTarget::webview_window(window.label()), "repo-changed", serde_json::json!({ "path": payload_path, "full": full }));
        }));
        let mut map = watchers.lock().unwrap();
        if !map.get(&key).is_some_and(|slot| Arc::ptr_eq(&slot.generation, &generation)) {
            drop(map);
            drop(result); // stop a superseded watcher outside the map lock
            return Ok(());
        }
        match result {
            Ok(watcher) => { map.get_mut(&key).unwrap().watcher = Some(watcher); Ok(()) }
            Err(error) => { map.remove(&key); Err(error) }
        }
    }).await?
}

#[tauri::command]
async fn unwatch_repo(path: String, window: tauri::WebviewWindow, state: State<'_, AppState>) -> Result<(), String> {
    let removed = state.watchers.lock().unwrap_or_else(|error| error.into_inner()).remove(&(window.label().to_owned(), path));
    if let Some(slot) = &removed { if slot.watcher.is_none() { read_jobs::cancel(&slot.read_id); } }
    blocking(move || drop(removed)).await
}

/// Called by the window once it has drawn about 120 frames in --screenshot mode.
#[tauri::command]
fn screenshot_ready(window: tauri::WebviewWindow, state: State<'_, AppState>, app: tauri::AppHandle) -> Result<(), String> {
    let Some(out) = state.screenshot.clone() else { return Ok(()) };
    take_and_exit(&window, &out, &app);
    Ok(())
}

#[cfg_attr(not(windows), allow(unused_variables))]
fn take_and_exit(window: &tauri::WebviewWindow, out: &Path, app: &tauri::AppHandle) {
    #[cfg(windows)]
    let res = match window.hwnd() {
        Ok(h) => shot::capture_window(h.0 as isize, out),
        Err(e) => Err(e.to_string()),
    };

    #[cfg(not(windows))]
    let res = shot::capture_window(0, out);
    match res {
        Ok(()) => println!("saved {}", out.display()),
        Err(e) => eprintln!("screenshot failed: {e}"),
    }
    app.exit(0);
}

fn main() {
    if accounts::handle_ssh_askpass() { return; }
    #[cfg(target_os = "macos")]
    external::add_macos_tool_paths();
    let args: Vec<String> = std::env::args().collect();
    let verification = cfg!(any(debug_assertions, feature = "verification"));
    let shot_view = args.iter().position(|a| a == "--shot-view").filter(|_| verification).and_then(|i| args.get(i + 1)).cloned();
    let screenshot = args.iter().position(|a| a == "--screenshot").filter(|_| verification).and_then(|i| args.get(i + 1)).map(|p| {
        let p = PathBuf::from(p);
        if p.is_absolute() {
            p
        } else {
            std::env::current_dir().unwrap_or_default().join(p)
        }
    });

    if screenshot.is_none() { diagnostics::install(); }
    let builder = tauri::Builder::default();
    let builder = if screenshot.is_none() {
        builder.plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            deliver_launch(app, startup_paths(&args, Path::new(&cwd)));
        }))
    } else { builder };
    let builder = if screenshot.is_none() {
        use tauri_plugin_window_state::StateFlags;
        builder.plugin(tauri_plugin_window_state::Builder::default()
            .with_state_flags(StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED).build())
    } else { builder };
    let launch_paths = startup_paths(&args, &std::env::current_dir().unwrap_or_default());
    #[cfg(any(target_os = "macos", windows, target_os = "linux"))]
    let builder = if screenshot.is_none() { builder.plugin(tauri_plugin_deep_link::init()) } else { builder };
    builder
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(AppState { engine: Arc::new(Engine::default()), screenshot: screenshot.clone(), shot_view, watchers: Arc::new(Mutex::new(HashMap::new())), terminals: terminal::Terminals::default(), launch_paths: Mutex::new(HashMap::from([("main".into(), launch_paths)])) })
        .on_window_event(|window, event| {
            if !matches!(event, tauri::WindowEvent::Destroyed) { return; }
            let Some(state) = window.try_state::<AppState>() else { return; };
            state.terminals.close_window(window.label());
            let mut watchers = state.watchers.lock().unwrap_or_else(|error| error.into_inner());
            let stale: Vec<_> = watchers.keys().filter(|(label, _)| label == window.label()).cloned().collect();
            let removed: Vec<_> = stale.into_iter().filter_map(|key| watchers.remove(&key)).collect();
            drop(watchers);
            for slot in &removed {
                if slot.watcher.is_none() { read_jobs::cancel(&slot.read_id); }
            }
            tauri::async_runtime::spawn_blocking(move || drop(removed));
        })
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            if screenshot.is_none() {
                use tauri_plugin_deep_link::DeepLinkExt;
                finder_services::register(app.handle().clone());
                let app_handle = app.handle().clone();
                if let Ok(Some(urls)) = app.deep_link().get_current() {
                    deliver_launch(&app_handle, urls.iter().filter_map(|url| clone_launch(url.as_str())).collect());
                }
                app.deep_link().on_open_url(move |event| {
                    deliver_launch(&app_handle, event.urls().iter().filter_map(|url| clone_launch(url.as_str())).collect());
                });
            }
            // Long jobs report progress to the window ("job-progress" events).
            let handle = app.handle().clone();
            let engine = app.state::<AppState>().engine.clone();
            engine.jobs.set_listener(std::sync::Arc::new(move |id: &str, text: &str, pct: Option<u32>| {
                let _ = handle.emit("job-progress", serde_json::json!({ "id": id, "text": text, "percent": pct }));
            }));
            // Test aid: `--shot-size 1920x1040` opens the window at that size (to check wide layouts).
            let args: Vec<String> = std::env::args().collect();
            if let Some((w, h)) = args
                .iter()
                .position(|a| a == "--shot-size").filter(|_| verification)
                .and_then(|i| args.get(i + 1))
                .and_then(|s| s.split_once('x'))
                .and_then(|(w, h)| Some((w.parse::<f64>().ok()?, h.parse::<f64>().ok()?)))
            {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.set_size(tauri::LogicalSize::new(w, h));
                    let _ = win.center();
                }
            }
            if let Some(out) = screenshot.clone().filter(|_| !args.windows(2).any(|a| a[0] == "--shot-view" && a[1] == "validation")) {
                // Safety net: take the picture anyway if the page never reports ready.
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(40));
                    if let Some(w) = handle.get_webview_window("main") {
                        let h2 = handle.clone();
                        let _ = handle.run_on_main_thread(move || take_and_exit(&w, &out, &h2));
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            launch_options,
            check_git_version,
            copy_diagnostics,
            commit_signature,
            repo_reflog,
            history_fetch_info,
            commit_template,
            open_repo,
            history_page,
            commit_files,
            commit_details,
            file_diff,
            file_versions,
            save_file,
            file_history,
            blame,
            commit_stats,
            range_files,
            tracked_files,
            open_in_editor,
            rebase_commits,
            rebase_plan,
            read_ssh_public_key,
            gitflow_config,
            update_config,
            ai_file_preview,
            ai_commit_message,
            ai_stash_message,
            ai_explain_commit,
            ai_explain_branch,
            ai_compose,
            set_ai_config,
            ai_test_connection,
            ai_list_models,
            ai_find_program,
            ai_pull_request,
            ai_resolve_conflict,
            save_patch,
            pick_patch_file,
            conflict_check,
            agent_sessions,
            pr_templates,
            range_diff,
            pr_default_target,
            repo_summary,
            set_runtime_settings,
            tool_versions,
            git_config_get,
            gpg_generate_key,
            gpg_export_key,
            forget_credentials,
            delete_orig_files,
            orig_cleanup_candidates,
            pick_file,
            sync_git_credentials,
            remote_auth,
            repo_status,
            hosting_status,
            set_secret,
            repo_remote,
            list_prs_page,
            commit_checks,
            list_prs,
            pr_details,
            pr_checkout_target,
            list_issues_page,
            issue_comments_page,
            list_issues,
            pr_comments_page,
            launchpad_page,
            pr_comments,
            pr_comment,
            create_pr,
            merge_pr,
            delete_merged_source,
            pull_review_data,
            submit_pull_review,
            cancel_pr_merge,
            launchpad,
            create_issue,
            set_integration_config,
            integration_statuses,
            integration_sign_in,
            integration_repos_page,
            integration_repos,
            publish_targets,
            publish_repository,
            resume_publish,
            integration_add_ssh_key,
            integration_ssh_page,
            issue_branch_name,
            open_url,
            term_open,
            term_write,
            term_ack,
            term_resize,
            term_close,
            account_info,
            set_git_config,
            push_review,
            generate_ssh_key,
            cancel_job,
            lfs_info,
            lfs_locks,
            sparse_info,
            gitignore_templates,
            cleanup_list,
            remotes_list,
            submodules_list,
            conflict_sample,
            conflict_info,
            run_action,
            activity_log,
            activity_log_snapshot,
            sample_repo,
            last_commit_message,
            discarded_changes,
            head_is_upstream,
            validate_ref_name,
            pick_folder,
            init_repo,
            clone_repo,
            suggest_clone_folder,
            open_terminal,
            open_file_manager,
            open_default,
            reveal_file,
            file_text,
            load_settings,
            take_launch_paths,
            open_repository_window,
            save_settings,
            desktop_validation_state,
            desktop_validation_accounts,
            acknowledge_job,
            cancel_read,
            watch_repo,
            unwatch_repo,
            screenshot_ready
        ])
        .build(tauri::generate_context!())
        .expect("error while building GitCraft")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                process_tree::shutdown();
                app.state::<AppState>().terminals.close_all();
            }
        });
}

#[cfg(test)]
mod clone_tests {
    use super::*;

    #[test]
    fn clone_arguments_keep_a_fixed_order() {
        let all = CloneOptions {
            depth: Some(5),
            since: Some("2025-01-31".into()),
            branch: Some("dev".into()),
            custom_flags: Some("--no-tags   --quiet".into()),
            sparse: Some(true),
            sparse_paths: None,
            blobless: Some(true),
            ..Default::default()
        };
        assert_eq!(
            clone_args(&all).unwrap(),
            ["--single-branch", "--branch", "dev", "--depth", "5", "--progress", "--shallow-since", "2025-01-31", "--no-tags", "--quiet", "--sparse", "--filter", "blob:none"]
        );
        assert_eq!(clone_args(&CloneOptions::default()).unwrap(), ["--progress"]);
        let blank = CloneOptions { branch: Some("  ".into()), depth: Some(0), ..Default::default() };
        assert_eq!(clone_args(&blank).unwrap(), ["--progress"]);
    }
}
