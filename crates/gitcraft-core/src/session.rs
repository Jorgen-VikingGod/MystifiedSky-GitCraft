//! Reading a repository for a window, cloning and creating repositories, and the folders or
//! `gitcraft://` links a launch hands over. Both interfaces call these.

use crate::actions::{Engine, UndoInfo};
use crate::metrics::INITIAL_COMMITS;
use crate::{external, git, graph, manage, operation, prefs, read_jobs, rewrite};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A repository as the window shows it: its data, the graph lanes, and where history paging
/// continues.
#[derive(Clone, Serialize)]
pub struct RepoView {
    pub path: String,
    pub name: String,
    #[serde(flatten)]
    pub data: git::RepoData,
    pub graph: graph::GraphLayout,
    pub history_cursor: Option<HistoryCursor>,
    pub undo: UndoInfo,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HistoryCursor {
    pub revs: Vec<String>,
    pub offset: usize,
    pub layout: graph::LayoutCursor,
    #[serde(default)]
    pub filter: git::HistoryFilter,
    /// Stashes made on commits further down, placed when their page loads.
    #[serde(default)]
    pub stashes: Vec<git::Stash>,
}

#[derive(Serialize)]
pub struct HistoryPage {
    pub commits: Vec<git::Commit>,
    pub graph: graph::GraphLayout,
    pub has_more: bool,
    pub history_cursor: Option<HistoryCursor>,
}

/// Read a repository: its first `max` commits, branches, status and the graph layout.
pub fn open_repo(engine: &Engine, path: &str, max: Option<usize>, view: Option<git::RefView>) -> Result<RepoView, String> {
    // Tabs saved by versions that turned /Users/x into \Users\x on macOS/Linux: restore the slashes.
    let path = if cfg!(not(windows)) && path.starts_with('\\') { path.replace('\\', "/") } else { path.to_owned() };
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err(format!("The folder {path} doesn't exist"));
    }
    let root = git::repo_root(&p).ok_or_else(|| format!("{path} is not a Git repository"))?;
    let operation = operation::lock_for(&root);
    if let Ok(_guard) = operation.try_lock() {
        rewrite::cleanup_replay_files(&root);
    }
    let view = view.unwrap_or_default();
    let mut data = git::load_view_checked(&root, max.unwrap_or(INITIAL_COMMITS).clamp(prefs::MIN_INITIAL_COMMITS, 5000), &view)?;
    let pinned_sha = view.pinned.as_ref().and_then(|p| data.refs.iter().find(|r| &r.full == p)).map(|r| r.sha.clone());
    let mut layout = graph::LayoutCursor::new(pinned_sha.as_deref());
    // Stashes are rows of the graph too, unless the history is filtered.
    let loaded = data.commits.len();
    let pending = if view.filter.is_active() { Vec::new() } else { place_stashes(&mut data.commits, data.stashes.clone()) };
    if data.commits.len() != loaded {
        data.index_of = data.commits.iter().enumerate().map(|(i, c)| (c.sha.clone(), i)).collect();
    }
    let graph = layout.append(&data.commits);
    let history_cursor = data.has_more.then(|| HistoryCursor { revs: data.history_revs.clone(), offset: loaded, layout, filter: view.filter.clone(), stashes: pending });
    // Windows paths use backslashes; on macOS/Linux the path must stay as-is (a blanket replace turns /Users/x into \Users\x).
    let root_s = if cfg!(windows) { root.to_string_lossy().replace('/', "\\") } else { root.to_string_lossy().into_owned() };
    Ok(RepoView { name: git::repo_name(&root), undo: engine.undo_info(&root_s), path: root_s, data, graph, history_cursor })
}

/// The next page of history after `cursor`.
pub fn history_page(path: &str, mut cursor: HistoryCursor, max: Option<usize>) -> Result<HistoryPage, String> {
    let (mut commits, has_more) = git::history_page(Path::new(path), &cursor.revs, cursor.offset, max.unwrap_or(2000).clamp(100, 5000), &cursor.filter)?;
    cursor.offset += commits.len();
    cursor.stashes = place_stashes(&mut commits, std::mem::take(&mut cursor.stashes));
    let graph = cursor.layout.append(&commits);
    Ok(HistoryPage { commits, graph, has_more, history_cursor: has_more.then_some(cursor) })
}

/// Put each stash into `commits` as a row of its own, at its date but above the commit it was
/// made on, with that commit as its only parent, so the graph draws it as a short branch.
/// Returns the stashes whose commit isn't among `commits` (a later page has it).
pub fn place_stashes(commits: &mut Vec<git::Commit>, stashes: Vec<git::Stash>) -> Vec<git::Stash> {
    let mut later = Vec::new();
    for s in stashes {
        let Some(base) = commits.iter().position(|c| c.sha == s.base) else {
            later.push(s);
            continue;
        };
        let at = commits[..base].iter().position(|c| c.time <= s.time).unwrap_or(base);
        commits.insert(at, git::Commit { sha: s.sha.clone(), parents: vec![s.base.clone()], author: s.author.clone(), email: s.email.clone(), time: s.time, summary: s.message.clone(), body: String::new() });
    }
    later
}

/// One commit's full message, author and parents.
pub fn commit_details(path: &str, sha: &str) -> Result<git::Commit, String> {
    let output = git::run(Path::new(path), &["log", "--no-walk", "-1", "--format=%H%x00%P%x00%aN%x00%aE%x00%at%x00%s%x00%b%x00", sha, "--"]);
    if !output.ok {
        return Err(output.message());
    }
    git::parse_commits(&output.stdout).into_iter().next().ok_or_else(|| "The selected commit is no longer available.".into())
}

/// The files a commit changed.
pub fn commit_files(path: &str, sha: &str) -> Vec<git::CommitFile> {
    let dir = PathBuf::from(path);
    let o = git::run(&dir, &["rev-list", "--parents", "-n1", sha]);
    let parents: Vec<String> = o.stdout.split_whitespace().skip(1).map(|s| s.to_string()).collect();
    let c = git::Commit { sha: sha.to_owned(), parents, author: String::new(), email: String::new(), time: 0, summary: String::new(), body: String::new() };
    git::commit_file_entries(&dir, &c)
}

/// The signature state of a commit (`%G?`: G good, B bad, N none ...).
pub fn commit_signature(path: &str, sha: &str) -> Result<String, String> {
    let output = git::run(Path::new(path), &["log", "-1", "--no-show-signature", "--format=%G?", "--end-of-options", sha, "--"]);
    if output.ok { Ok(output.stdout.trim().to_owned()) } else { Err(output.message()) }
}

/// Whether `sha` is already on the checked-out branch's upstream.
pub fn head_is_upstream(path: &str, sha: &str) -> bool {
    if !matches!(sha.len(), 40 | 64) || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return false;
    }
    let dir = Path::new(path);
    let upstream = git::run(dir, &["rev-parse", "--verify", "@{upstream}"]);
    upstream.ok && git::run(dir, &["merge-base", "--is-ancestor", sha, upstream.stdout.trim()]).ok
}

/// The hosting service of a repository's origin (or first) remote.
pub fn remote_of(path: &str, gitlab_host: &str) -> Result<crate::hosting::RemoteRepo, String> {
    let dir = Path::new(path);
    let mut url = git::run(dir, &["remote", "get-url", "origin"]).stdout.trim().to_string();
    if url.is_empty() {
        if let Some(first) = git::run(dir, &["remote"]).stdout.lines().next() {
            url = git::run(dir, &["remote", "get-url", first]).stdout.trim().to_string();
        }
    }
    crate::hosting::parse_remote(&url, gitlab_host).ok_or_else(|| "This repository's remote isn't on a connected service (Preferences > Integrations).".to_string())
}

/// What an interactive rebase onto `base` would replay, and the state it expects to find.
pub fn rebase_plan(path: &str, base: &str) -> Result<(rewrite::PlanState, Vec<git::Commit>), String> {
    let dir = Path::new(path);
    let lock = operation::lock_for(dir);
    let _guard = read_jobs::lock(&lock)?;
    let expected = rewrite::plan_state(dir, base)?;
    let commits = rewrite::rebase_commits(dir, base)?;
    Ok((expected, commits))
}

/// Create a repository in a new, empty folder `parent/name`.
pub fn init_repo(engine: &Engine, parent: &str, name: &str, options: Option<manage::InitOptions>) -> Result<manage::InitOutcome, String> {
    let name = name.trim();
    if name.is_empty() || Path::new(name).components().count() != 1 || !matches!(Path::new(name).components().next(), Some(std::path::Component::Normal(_))) {
        return Err("Enter a folder name, without parent directories.".into());
    }
    let dir = PathBuf::from(parent).join(name);
    let nonempty = match std::fs::read_dir(&dir) {
        Ok(mut entries) => entries.next().transpose().map_err(|e| e.to_string())?.is_some(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.to_string()),
    };
    if nonempty {
        return Err("Choose an empty folder for a new repository. Existing files were not changed.".into());
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let opts = options.unwrap_or_default();
    let branch = if opts.branch.trim().is_empty() { "main".to_string() } else { opts.branch.trim().to_string() };
    let o = engine.git(&dir, &["init", "-b", &branch]);
    if !o.ok {
        return Err(o.message());
    }
    let f = manage::init_files(&dir, name, &opts, &|a: &[&str]| engine.git(&dir, a));
    if !f.ok {
        return Err(f.message());
    }
    Ok(manage::initialize_lfs(&dir, opts.lfs, &|args| engine.git(&dir, args)))
}

/// What the Clone dialog can ask for besides the address and folder.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct CloneOptions {
    /// Only the newest this-many commits (--depth).
    pub depth: Option<u32>,
    /// Only commits newer than this date, YYYY-MM-DD (--shallow-since).
    pub since: Option<String>,
    /// Clone just this branch (--single-branch --branch).
    pub branch: Option<String>,
    /// Extra flags parsed as quoted argument fields without shell expansion.
    pub custom_flags: Option<String>,
    /// Start with a sparse checkout (--sparse) and check out only `sparse_paths`.
    pub sparse: Option<bool>,
    pub sparse_paths: Option<Vec<String>>,
    /// Leave file contents out of the first download (--filter blob:none).
    pub blobless: Option<bool>,
    pub folder_name: Option<String>,
    pub initialize_submodules: Option<bool>,
}

/// The `git clone` arguments in a fixed order: branch, depth, progress, since, custom flags, sparse, filter.
pub fn clone_args(o: &CloneOptions) -> Result<Vec<String>, String> {
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
    if o.initialize_submodules == Some(true) {
        a.push("--recurse-submodules".into());
    }
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

pub fn valid_clone_folder(name: &str) -> bool {
    !name.is_empty() && !matches!(name, "." | "..") && !name.ends_with([' ', '.']) &&
        !name.chars().any(|character| character.is_control() || matches!(character, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')) &&
        (!cfg!(windows) || {
            let base = name.split('.').next().unwrap_or_default().to_ascii_uppercase();
            !matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL") &&
                !["COM", "LPT"].iter().any(|prefix| base.strip_prefix(prefix).is_some_and(|suffix| matches!(suffix, "1"|"2"|"3"|"4"|"5"|"6"|"7"|"8"|"9"|"¹"|"²"|"³")))
        })
}

pub fn clone_folder_base(url: &str) -> String {
    let parsed = url.contains("://").then(|| url::Url::parse(url).ok()).flatten();
    let path = parsed.as_ref().map(|url| url.path()).unwrap_or(url);
    let part = path.trim_end_matches(['/', '\\']).rsplit(['/', '\\', ':']).next().unwrap_or_default();
    let name = part.strip_suffix(".git").unwrap_or(part);
    if valid_clone_folder(name) { name.into() } else { "repository".into() }
}

pub fn available_clone_folder(parent: &Path, base: &str) -> Result<String, String> {
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

/// A free folder name in `parent` for a clone of `url`.
pub fn suggest_clone_folder(url: &str, parent: &str) -> Result<String, String> {
    available_clone_folder(Path::new(parent), &clone_folder_base(url.trim()))
}

/// Clone `url` into a new folder in `parent`, with progress reported as the "clone" job.
pub fn clone_repo(engine: &Engine, url: &str, parent: &str, options: Option<CloneOptions>) -> Result<String, String> {
    let url = url.trim().to_owned();
    if url.is_empty() || url.starts_with('-') || url.contains(['\r', '\n', '\0']) {
        return Err("Enter a repository URL or path, without command options.".into());
    }
    // Absolute, so Git (which runs in the parent folder) and the steps after it use one place.
    let parent = std::path::absolute(parent).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
    let opts = options.unwrap_or_default();
    let base = clone_folder_base(&url);
    let name = match opts.folder_name.as_deref().map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) if !valid_clone_folder(name) => return Err("Use a single valid folder name inside the selected parent folder.".into()),
        Some(name) => {
            let available = available_clone_folder(&parent, name)?;
            if available != name {
                return Err(format!("Folder {name} already exists. Try {available}."));
            }
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
    if defer_checkout {
        args.push("--no-checkout".into());
    }
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
            if !out.ok {
                return Err(out.message());
            }
        }
        let requested: Vec<_> = clone_options.iter().filter_map(|arg| arg.strip_prefix("--recurse-submodules=")).collect();
        if opts.initialize_submodules == Some(true) || !requested.is_empty() || clone_options.iter().any(|arg| arg == "--recurse-submodules" || arg == "--recursive") {
            let mut update = vec!["submodule", "update", "--init", "--recursive"];
            if clone_options.iter().any(|arg| arg == "--shallow-submodules") {
                update.push("--depth=1");
            }
            if clone_options.iter().any(|arg| arg == "--remote-submodules") {
                update.push("--remote");
            }
            if !requested.is_empty() {
                update.push("--");
                update.extend(requested);
            }
            let out = engine.job("clone", "submodules", &dest, &update);
            if !out.ok {
                return Err(format!("The repository was cloned, but its submodules could not be initialized: {}", out.message()));
            }
        }
    }
    if opts.sparse == Some(true) && !no_worktree {
        // Only the chosen paths are checked out; with none, only the files in the top folder.
        let mut set: Vec<String> = vec!["sparse-checkout".into(), "set".into(), "--no-cone".into()];
        let paths: Vec<String> = opts.sparse_paths.unwrap_or_default().into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect();
        if !paths.is_empty() {
            set.push("--".into());
            set.extend(paths);
        } else {
            set.extend(["/*".into(), "!/*/".into()]);
        }
        let refs: Vec<&str> = set.iter().map(String::as_str).collect();
        let s = engine.git(&dest, &refs);
        if !s.ok {
            return Err(format!("The repository was cloned, but the sparse checkout could not be set: {}", s.message()));
        }
    }
    Ok(dest.to_string_lossy().into_owned())
}

/// Something a launch asks the window to open.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum LaunchIntent {
    Repository { path: String },
    Clone { url: String },
}

/// A `gitcraft://clone?url=` link, when it names a plain http(s) repository address.
pub fn clone_launch(value: &str) -> Option<LaunchIntent> {
    if value.len() > 8192 || value.chars().any(char::is_control) {
        return None;
    }
    let launch = url::Url::parse(value).ok()?;
    if launch.scheme() != "gitcraft" || launch.host_str() != Some("clone") || !matches!(launch.path(), "" | "/") ||
        launch.port().is_some() || !launch.username().is_empty() || launch.password().is_some() || launch.fragment().is_some() {
        return None;
    }
    let mut fields = launch.query_pairs();
    let (name, value) = fields.next()?;
    if name != "url" || fields.next().is_some() || value.chars().any(char::is_control) {
        return None;
    }
    let url = url::Url::parse(&value).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() || !url.username().is_empty() ||
        url.password().is_some() || url.query().is_some() || url.fragment().is_some() || matches!(url.path(), "" | "/") {
        return None;
    }
    Some(LaunchIntent::Clone { url: url.into() })
}

/// What the program's arguments ask to open: one folder, or a `gitcraft://` link. `args[0]` is
/// the program.
pub fn startup_paths(args: &[String], cwd: &Path) -> Vec<LaunchIntent> {
    let args = args.get(1..).unwrap_or_default();
    let args = if args.first().is_some_and(|arg| arg == "--") { &args[1..] } else { args };
    let Some(argument) = args.first().filter(|arg| !arg.starts_with('-')) else { return Vec::new() };
    if argument.starts_with("gitcraft:") {
        return clone_launch(argument).into_iter().collect();
    }
    let argument = explorer_argument(argument);
    let path = cwd.join(argument.as_ref());
    if path.is_dir() { vec![LaunchIntent::Repository { path: path.to_string_lossy().into_owned() }] } else { Vec::new() }
}

/// Explorer passes the folder as `"%1"`. For a drive root that is `"C:\"`, and Windows argument
/// parsing reads the `\"` as an escaped quote, so the program receives `C:"`. Restore the separator.
pub fn explorer_argument(argument: &str) -> std::borrow::Cow<'_, str> {
    match argument.strip_suffix('"') {
        Some(folder) if cfg!(windows) => std::borrow::Cow::Owned(format!("{folder}\\")),
        _ => std::borrow::Cow::Borrowed(argument),
    }
}

/// Run `f` as a cancellable read named `id` (see [`read_jobs`]).
pub fn read<T>(id: Option<String>, f: impl FnOnce() -> T) -> T {
    read_jobs::scope(id, f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_clone_launch_requires_a_reviewable_web_url() {
        assert!(matches!(clone_launch("gitcraft://clone?url=https%3A%2F%2Fexample.invalid%2Fteam%2Fproject.git"), Some(LaunchIntent::Clone { .. })));
        for invalid in [
            "gitcraft://clone?url=file%3A%2F%2F%2Fproject",
            "gitcraft://clone?url=https%3A%2F%2Fname%40example.invalid%2Fproject",
            "gitcraft://clone?url=https%3A%2F%2Fexample.invalid%2Fproject%3Ftoken%3Dsecret",
            "gitcraft://clone?url=https%3A%2F%2Fexample.invalid%2Fproject&url=other",
        ] {
            assert!(clone_launch(invalid).is_none());
        }
    }

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

    #[test]
    fn stashes_sit_at_their_date_above_the_commit_they_were_made_on() {
        let commit = |sha: &str, time: i64, parents: &[&str]| git::Commit { sha: sha.into(), parents: parents.iter().map(|p| p.to_string()).collect(), author: String::new(), email: String::new(), time, summary: String::new(), body: String::new() };
        let stash = |sha: &str, base: &str, time: i64| git::Stash { index: 0, sha: sha.into(), message: format!("On main: {sha}"), base: base.into(), author: "A".into(), email: "a@example.invalid".into(), time };
        let mut commits = vec![commit("c3", 30, &["c2"]), commit("c2", 20, &["c1"]), commit("c1", 10, &[])];
        // s1 is newer than everything but was made on c2; s0 was made on c1 between c1 and c2;
        // s9's commit is on a later page.
        let later = place_stashes(&mut commits, vec![stash("s1", "c2", 40), stash("s0", "c1", 15), stash("s9", "c0", 5)]);
        let order: Vec<&str> = commits.iter().map(|c| c.sha.as_str()).collect();
        assert_eq!(order, ["s1", "c3", "c2", "s0", "c1"]);
        assert_eq!(commits[0].parents, ["c2"], "a stash row's only parent is its commit");
        assert_eq!(commits[0].summary, "On main: s1");
        assert_eq!(later.len(), 1);
        assert_eq!(later[0].sha, "s9");
    }

    #[test]
    fn clone_folders_come_from_the_address() {
        assert_eq!(clone_folder_base("https://example.invalid/team/project.git"), "project");
        assert_eq!(clone_folder_base("git@example.invalid:team/tool.git"), "tool");
        assert_eq!(clone_folder_base("https://example.invalid/"), "repository");
    }
}
