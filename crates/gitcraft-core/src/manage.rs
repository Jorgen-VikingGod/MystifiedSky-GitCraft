//! Stage 4: repository management — remotes, worktrees, submodules, branch clean-up and Gitflow.
//! Gitflow uses these config keys:
//! `gitflow.branch.master` / `gitflow.branch.develop` / `gitflow.prefix.*`, defaults main, develop,
//! feature/, release/, hotfix/ and an empty version-tag prefix.

use crate::git::{self, GitOut};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const GITFLOW_DEFAULTS: [(&str, &str); 6] = [
    ("gitflow.branch.master", "main"),
    ("gitflow.branch.develop", "develop"),
    ("gitflow.prefix.feature", "feature/"),
    ("gitflow.prefix.release", "release/"),
    ("gitflow.prefix.hotfix", "hotfix/"),
    ("gitflow.prefix.versiontag", ""),
];

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct GitflowConfig {
    pub master: String,
    pub develop: String,
    pub feature: String,
    pub release: String,
    pub hotfix: String,
    pub versiontag: String,
    /// Gitflow has been initialised in this repository (its keys exist).
    pub initialized: bool,
}

pub fn gitflow_config(dir: &Path) -> GitflowConfig {
    let get = |k: &str| {
        let o = git::run(dir, &["config", "--get", k]);
        o.ok.then(|| o.stdout.trim_end_matches(['\r', '\n']).to_string())
    };
    let initialized = get("gitflow.branch.develop").is_some();
    let val = |k: &str| get(k).unwrap_or_else(|| GITFLOW_DEFAULTS.iter().find(|(n, _)| *n == k).unwrap().1.to_string());
    GitflowConfig {
        master: val("gitflow.branch.master"),
        develop: val("gitflow.branch.develop"),
        feature: val("gitflow.prefix.feature"),
        release: val("gitflow.prefix.release"),
        hotfix: val("gitflow.prefix.hotfix"),
        versiontag: val("gitflow.prefix.versiontag"),
        initialized,
    }
}

fn exists(dir: &Path, branch: &str) -> bool {
    git::run(dir, &["rev-parse", "--verify", "-q", &format!("refs/heads/{branch}")]).ok
}

fn err(msg: impl Into<String>) -> GitOut {
    GitOut { ok: false, stdout: String::new(), stderr: msg.into() }
}

fn ok() -> GitOut {
    GitOut { ok: true, stdout: String::new(), stderr: String::new() }
}

/// "Initialize Gitflow": store the branch names and prefixes, and create develop from main if needed.
pub fn gitflow_init(dir: &Path, cfg: &GitflowConfig, run: &dyn Fn(&[&str]) -> GitOut) -> GitOut {
    let pairs = [
        ("gitflow.branch.master", &cfg.master),
        ("gitflow.branch.develop", &cfg.develop),
        ("gitflow.prefix.feature", &cfg.feature),
        ("gitflow.prefix.release", &cfg.release),
        ("gitflow.prefix.hotfix", &cfg.hotfix),
        ("gitflow.prefix.versiontag", &cfg.versiontag),
    ];
    for (k, v) in pairs {
        let o = run(&["config", k, v.as_str()]);
        if !o.ok {
            return o;
        }
    }
    if !exists(dir, &cfg.master) {
        return err(format!("{} branch is required", cfg.master));
    }
    if !exists(dir, &cfg.develop) {
        return run(&["branch", &cfg.develop, &cfg.master]);
    }
    ok()
}

/// Start a feature (from develop), release (from develop) or hotfix (from main) and check it out.
pub fn gitflow_start(dir: &Path, kind: &str, name: &str, run: &dyn Fn(&[&str]) -> GitOut) -> GitOut {
    let cfg = gitflow_config(dir);
    let name = name.trim().replace(' ', "-");
    if name.is_empty() {
        return err(format!("{} name is required", capital(kind)));
    }
    let (prefix, from) = match kind {
        "feature" => (&cfg.feature, &cfg.develop),
        "release" => (&cfg.release, &cfg.develop),
        "hotfix" => (&cfg.hotfix, &cfg.master),
        _ => return err("unknown Gitflow branch type"),
    };
    if !exists(dir, from) {
        return err(format!("{from} branch is required to start {kind}"));
    }
    run(&["checkout", "-q", "-b", &format!("{prefix}{name}"), from])
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// Finish: a feature merges into develop; a release or hotfix merges into main (tagged with the
/// version-tag prefix + name), then into develop. The branch is deleted unless `keep`.
pub fn gitflow_finish(dir: &Path, kind: &str, name: &str, keep: bool, tag_message: &str, run: &dyn Fn(&[&str]) -> GitOut) -> GitOut {
    let cfg = gitflow_config(dir);
    let prefix = match kind {
        "feature" => &cfg.feature,
        "release" => &cfg.release,
        "hotfix" => &cfg.hotfix,
        _ => return err("unknown Gitflow branch type"),
    };
    let branch = format!("{prefix}{name}");
    if !exists(dir, &branch) {
        return err(format!("No {kind} to finish"));
    }
    let merge_into = |target: &str| -> GitOut {
        let o = run(&["checkout", "-q", target]);
        if !o.ok {
            return o;
        }
        let args = crate::extras::merge_args(dir, "merge", &["--no-ff", "--no-edit", &branch]);
        run(&args.iter().map(String::as_str).collect::<Vec<_>>())
    };
    if kind == "feature" {
        if !exists(dir, &cfg.develop) {
            return err(format!("{} branch is required to finish feature", cfg.develop));
        }
        let o = merge_into(&cfg.develop);
        if !o.ok {
            return o;
        }
    } else {
        for b in [&cfg.master, &cfg.develop] {
            if !exists(dir, b) {
                return err(format!("{b} branch is required to finish {kind}"));
            }
        }
        let tag = format!("{}{name}", cfg.versiontag);
        let checkpoint = git::run(dir, &["rev-parse", "--git-path", "gitcraft-gitflow-finish.json"]).stdout.trim().to_owned();
        let checkpoint = dir.join(checkpoint);
        let existing = git::run(dir, &["rev-parse", "--verify", &format!("refs/tags/{tag}^{{commit}}")]);
        if existing.ok {
            let saved: serde_json::Value = std::fs::read_to_string(&checkpoint).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
            let branch_sha = git::run(dir, &["rev-parse", &branch]).stdout.trim().to_owned();
            if saved["branch"] != branch || saved["source"] != branch_sha || saved["tag"] != tag || saved["commit"] != existing.stdout.trim() {
                return err(format!("Tag {tag} already exists and does not match this unfinished operation."));
            }
        } else {
            let o = merge_into(&cfg.master);
            if !o.ok { return o; }
            let commit = git::run(dir, &["rev-parse", "HEAD"]).stdout.trim().to_owned();
            let source = git::run(dir, &["rev-parse", &branch]).stdout.trim().to_owned();
            let saved = serde_json::json!({ "branch": branch, "source": source, "tag": tag, "commit": commit });
            if let Err(error) = std::fs::write(&checkpoint, saved.to_string()) { return err(error.to_string()); }
            let msg = if tag_message.trim().is_empty() { tag.clone() } else { tag_message.trim().to_string() };
            let o = run(&["tag", "-a", &tag, "-m", &msg]);
            if !o.ok { return o; }
        }
        let o = merge_into(&cfg.develop);
        if !o.ok {
            return o;
        }
    }
    if !keep {
        return run(&["branch", "-d", &branch]);
    }
    ok()
}

#[derive(Serialize, Clone, Debug)]
pub struct CleanupBranch {
    pub name: String,
    pub time: i64,
    pub upstream: String,
    /// Fully merged into the repository's default branch.
    pub merged: bool,
    pub patch_equivalent: bool,
    pub upstream_gone: bool,
    #[serde(skip)]
    pub remote: String,
    #[serde(skip)]
    pub remote_ref: String,
}

pub(crate) fn default_branch(dir: &Path) -> Option<(String, String)> {
    let checked = |reference: &str| git::run(dir, &["show-ref", "--verify", "--quiet", reference]).ok;
    let names = git::run(dir, &["remote"]).stdout;
    for name in std::iter::once("origin").chain(names.lines().filter(|name| *name != "origin")) {
        let prefix = format!("refs/remotes/{name}/");
        let result = git::run(dir, &["symbolic-ref", "-q", &format!("{prefix}HEAD")]);
        if let Some(branch) = result.stdout.trim().strip_prefix(&prefix).filter(|_| result.ok && checked(result.stdout.trim())) {
            return Some((result.stdout.trim().to_owned(), branch.to_owned()));
        }
    }
    let preferred = git::run(dir, &["config", "--get", "init.defaultBranch"]).stdout;
    for branch in [preferred.trim(), "main", "master"] {
        for prefix in ["refs/heads/", "refs/remotes/origin/"] {
            let reference = format!("{prefix}{branch}");
            if !branch.is_empty() && checked(&reference) { return Some((reference, branch.to_owned())); }
        }
    }
    None
}

/// Omit the default branch and all branches attached to a worktree.
pub fn cleanup_list(dir: &Path) -> Vec<CleanupBranch> {
    review_cleanup(dir, false)
}

pub fn review_cleanup(dir: &Path, detect_patches: bool) -> Vec<CleanupBranch> {
    let Some((target, default)) = default_branch(dir) else { return vec![]; };
    let merged = git::run(dir, &["for-each-ref", "--merged", &target, "--format=%(refname)", "refs/heads/"]);
    let merged: std::collections::HashSet<_> = merged.stdout.lines().collect();
    let refs = git::run(dir, &["for-each-ref", "--format=%(refname)%00%(committerdate:unix)%00%(upstream:remotename)%00%(upstream:remoteref)%00%(worktreepath)%00%(upstream:track)", "refs/heads/"]);
    let target_patches = if detect_patches {
        read_patch_ids(dir, &["log", "--first-parent", "--no-merges", "-n512", "--format=commit %H", "-p", "--no-ext-diff", "--no-textconv", &target])
    } else { Default::default() };
    refs.stdout.lines().filter_map(|line| {
        let [reference, time, remote, remote_ref, worktree, track] = line.split('\0').collect::<Vec<_>>().try_into().ok()?;
        let name = reference.strip_prefix("refs/heads/")?;
        if name == default || !worktree.is_empty() { return None; }
        let upstream = match remote_ref.strip_prefix("refs/heads/") {
            Some(branch) if branch != default && !remote.is_empty() && remote != "." => format!("{remote}/{branch}"),
            _ => String::new(),
        };
        let merged = merged.contains(reference);
        let patch_equivalent = detect_patches && !merged && crate::read_jobs::check().is_ok()
            && patch_equivalent(dir, &target, reference, &target_patches);
        Some(CleanupBranch { name: name.into(), time: time.parse().unwrap_or_default(), upstream, merged,
            patch_equivalent, upstream_gone: track == "[gone]", remote: remote.into(), remote_ref: remote_ref.into() })
    }).collect()
}

fn read_patch_ids(dir: &Path, args: &[&str]) -> std::collections::HashSet<String> {
    let source = crate::read_jobs::run_bytes(git::git_command(dir, args), 8 * 1024 * 1024, None);
    if !source.ok || source.stdout.is_empty() { return Default::default(); }
    let ids = crate::read_jobs::run(git::git_command(dir, &["patch-id", "--stable"]), 256 * 1024, Some(source.stdout));
    if !ids.ok { return Default::default(); }
    ids.stdout.lines().filter_map(|line| line.split_whitespace().next().map(str::to_owned)).collect()
}

fn patch_equivalent(dir: &Path, target: &str, branch: &str, target_patches: &std::collections::HashSet<String>) -> bool {
    let range = format!("{target}..{branch}");
    let merges = git::run(dir, &["rev-list", "--merges", "-n1", &range]);
    if !merges.ok || !merges.stdout.trim().is_empty() { return false; }
    let cherry = git::run_limited(dir, &["cherry", target, branch], 128 * 1024);
    if cherry.ok && !cherry.stdout.trim().is_empty() && cherry.stdout.lines().all(|line| line.starts_with("- ")) {
        return true;
    }
    if target_patches.is_empty() { return false; }
    let range = format!("{target}...{branch}");
    read_patch_ids(dir, &["diff", "--no-ext-diff", "--no-textconv", &range, "--"])
        .iter().any(|patch| target_patches.contains(patch))
}

#[derive(Serialize, Clone, Debug)]
pub struct Remote {
    pub name: String,
    pub fetch_url: String,
    pub push_url: String,
}

pub fn remotes(dir: &Path) -> Vec<Remote> {
    git::run(dir, &["remote"]).stdout.lines().map(|name| Remote {
        name: name.into(),
        fetch_url: git::run(dir, &["remote", "get-url", name]).stdout.trim_end_matches(['\r', '\n']).into(),
        push_url: git::run(dir, &["remote", "get-url", "--push", name]).stdout.trim_end_matches(['\r', '\n']).into(),
    }).collect()
}

#[derive(Serialize, Clone, Debug)]
pub struct Submodule {
    pub path: String,
    pub sha: String,
    /// in-sync, out-of-sync, uninitialized or conflict (from `git submodule status`).
    pub state: String,
    pub url: String,
}

pub fn submodules(dir: &Path) -> Vec<Submodule> {
    let config = git::run(dir, &["config", "--file", ".gitmodules", "-z", "--get-regexp", "^submodule\\..*\\.(path|url)$"]);
    let mut modules: std::collections::BTreeMap<String, (String, String)> = Default::default();
    for record in config.stdout.split('\0') {
        let Some((key, value)) = record.split_once('\n') else { continue };
        if let Some(name) = key.strip_prefix("submodule.").and_then(|k| k.strip_suffix(".path")) { modules.entry(name.into()).or_default().0 = value.into(); }
        if let Some(name) = key.strip_prefix("submodule.").and_then(|k| k.strip_suffix(".url")) { modules.entry(name.into()).or_default().1 = value.into(); }
    }
    let status = git::run(dir, &["-c", "core.quotePath=false", "submodule", "status"]);
    modules.into_values().filter(|(path, _)| !path.is_empty()).map(|(path, url)| {
        let found = status.stdout.lines().find(|line| line.len() > 2 && line[1..].split_once(' ').is_some_and(|(_, rest)| rest == path || rest.strip_prefix(&path).is_some_and(|suffix| suffix.starts_with(" ("))));
        let sha = found.and_then(|line| line[1..].split_once(' ')).map(|(sha, _)| sha.into()).unwrap_or_default();
        let state = match found.and_then(|line| line.chars().next()) { Some('-') | None => "uninitialized", Some('+') => "out-of-sync", Some('U') => "conflict", _ => "in-sync" };
        Submodule { path, url, sha, state: state.into() }
    }).collect()
}


/// Git LFS: whether it's installed, the tracked patterns and the current locks.
#[derive(Serialize, Default)]
pub struct LfsInfo {
    pub available: bool,
    pub version: String,
    /// LFS hooks are installed in this repository.
    pub enabled: bool,
    pub patterns: Vec<String>,
    pub locks: Vec<LfsLock>,
}

#[derive(Serialize, Clone)]
pub struct LfsLock {
    pub id: String,
    pub path: String,
    pub owner: String,
    pub ours: bool,
}

pub const LFS_REQUIRED: &str = "This revision uses Git LFS. Install Git LFS, run git lfs install, then retry checkout.";

pub fn git_lfs_available(dir: &Path) -> bool {
    crate::read_jobs::run_bounded(git::git_command(dir, &["lfs", "version"]), None,
        std::time::Duration::from_secs(5), "Git LFS version check timed out.").ok
}

pub fn tree_uses_lfs(dir: &Path, revision: &str) -> Result<bool, String> {
    let expression = format!("{revision}^{{tree}}");
    let resolved = git::run(dir, &["rev-parse", "--verify", "-q", "--end-of-options", &expression]);
    if !resolved.ok { return Ok(false); } // An unborn branch or a target Git will diagnose itself.
    let tree = resolved.stdout.trim();
    let paths = crate::read_jobs::run_bytes(git::git_command(dir, &["ls-tree", "-r", "-z", "--name-only", tree]), 16 * 1024 * 1024, None);
    if !paths.ok { return Err("Could not check the target tree for Git LFS attributes.".into()); }
    if paths.stdout.is_empty() { return Ok(false); }
    let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
    let index = temp.path().join("index");
    let mut command = git::git_command(dir, &["read-tree", tree]);
    command.env("GIT_INDEX_FILE", &index);
    let load = crate::read_jobs::run(command, 128 * 1024, None);
    if !load.ok { return Err(load.message()); }
    let mut command = git::git_command(dir, &["check-attr", "--cached", "--stdin", "-z", "filter"]);
    command.env("GIT_INDEX_FILE", &index);
    let attributes = crate::read_jobs::run_bytes(command, 32 * 1024 * 1024, Some(paths.stdout));
    if !attributes.ok { return Err("Could not read the target tree's Git LFS attributes.".into()); }
    let fields: Vec<_> = attributes.stdout.split(|byte| *byte == 0).collect();
    Ok(fields.chunks_exact(3).any(|row| row[2] == b"lfs"))
}

pub fn require_lfs_for_checkout(dir: &Path, revision: &str) -> Result<(), String> {
    if !git_lfs_available(dir) && tree_uses_lfs(dir, revision)? {
        return Err(LFS_REQUIRED.into());
    }
    Ok(())
}

pub fn lfs_info(dir: &Path) -> LfsInfo {
    let v = git::run(dir, &["lfs", "version"]);
    if !v.ok {
        return LfsInfo::default();
    }
    let hook = git::run(dir, &["rev-parse", "--git-path", "hooks/pre-push"]).stdout.trim().to_string();
    let enabled = std::fs::read_to_string(dir.join(hook)).map(|s| s.contains("git lfs")).unwrap_or(false);
    let tracked = git::run(dir, &["lfs", "track"]);
    let patterns = if tracked.ok {
        tracked.stdout.lines()
            .skip_while(|line| *line != "Listing tracked patterns").skip(1)
            .take_while(|line| *line != "Listing excluded patterns")
            .filter_map(|line| {
                let (pattern, _) = line.strip_prefix("    ")?.strip_suffix(')')?.rsplit_once(" (")?;
                Some(pattern.to_owned())
            })
            .collect()
    } else { Vec::new() };
    LfsInfo { available: true, version: v.stdout.trim().to_string(), enabled, patterns, locks: vec![] }
}

pub fn lfs_lockable(dir: &Path, path: &str) -> Result<bool, String> {
    let result = git::run_limited(dir, &["check-attr", "-z", "filter", "lockable", "--", path], 64 * 1024);
    if !result.ok { return Err(result.message()); }
    let fields: Vec<_> = result.stdout.split_terminator('\0').collect();
    if fields.len() != 6 { return Err("Git returned incomplete lockable attributes.".into()); }
    Ok(fields[2] == "lfs" && fields[5] == "set")
}

pub fn lfs_locks(dir: &Path) -> Result<Vec<LfsLock>, String> {
    let mut command = git::git_command(dir, &["lfs", "locks", "--verify", "--json"]);
    command.env("GIT_TERMINAL_PROMPT", "0").env("GCM_INTERACTIVE", "never");
    let locks = crate::read_jobs::run_bounded(command, None, std::time::Duration::from_secs(20), "The Git LFS lock server did not respond.");
    if !locks.ok || !locks.stderr.trim().is_empty() { return Err(locks.message()); }
    parse_lfs_locks(&locks.stdout)
}

pub(crate) fn parse_lfs_locks(json: &str) -> Result<Vec<LfsLock>, String> {
    let value: serde_json::Value = serde_json::from_str(json.trim()).map_err(|e| e.to_string())?;
    let mut locks = Vec::new();
    for (key, ours) in [("ours", true), ("theirs", false)] {
        let rows = value.get(key).filter(|v| v.is_array() || v.is_null())
            .ok_or("The Git LFS server did not return verified lock ownership.")?;
        for row in rows.as_array().into_iter().flatten() {
            locks.push(LfsLock {
                id: row["id"].as_str().ok_or("Missing Git LFS lock ID.")?.to_owned(),
                path: row["path"].as_str().ok_or("Missing Git LFS lock path.")?.to_owned(),
                owner: row["owner"]["name"].as_str().unwrap_or_default().to_owned(),
                ours,
            });
        }
    }
    Ok(locks)
}

/// Add a pattern to .gitignore unless it's already there.
pub fn ignore(dir: &Path, pattern: &str) -> Result<bool, String> {
    use std::io::Write;
    if pattern.contains(['\r','\n','\0']) {return Err("An ignore rule must be a single text line.".into());}
    let p=dir.join(".gitignore");
    let cur=match std::fs::read(&p) {
        Ok(bytes)=>bytes,
        Err(error) if error.kind()==std::io::ErrorKind::NotFound=>Vec::new(),
        Err(error)=>return Err(format!("Could not read .gitignore; its contents were kept: {error}")),
    };
    if cur.contains(&0) {return Err("This .gitignore contains binary or unsupported encoded data. Its contents were kept.".into());}
    if cur.split(|byte|*byte==b'\n').enumerate().any(|(index,line)| {
        let line=line.strip_suffix(b"\r").unwrap_or(line);
        let line=if index==0 {line.strip_prefix(&[0xef,0xbb,0xbf]).unwrap_or(line)} else {line};
        line==pattern.as_bytes()
    }) {return Ok(false);}
    let newline=if cur.windows(2).any(|pair|pair==b"\r\n") {b"\r\n".as_slice()} else {b"\n".as_slice()};
    let mut appended=Vec::new();
    if !cur.is_empty() {appended.extend_from_slice(newline);}
    appended.extend_from_slice(pattern.as_bytes());appended.extend_from_slice(newline);
    // Append original bytes instead of decoding/replacing existing rules.
    std::fs::OpenOptions::new().create(true).append(true).open(&p).and_then(|mut file|file.write_all(&appended)).map_err(|error|error.to_string())?;
    Ok(true)
}

#[derive(Serialize, Default)]
pub struct SparseInfo {
    pub enabled: bool,
    pub patterns: Vec<String>,
}

pub fn sparse_info(dir: &Path) -> SparseInfo {
    let on = git::run(dir, &["config", "--bool", "core.sparseCheckout"]).stdout.trim() == "true";
    let patterns = if on { git::run(dir, &["sparse-checkout", "list"]).stdout.lines().map(|s| s.to_string()).collect() } else { Vec::new() };
    SparseInfo { enabled: on, patterns }
}

/// .gitignore templates offered when creating a repository (GitCraft's own short versions).
pub const GITIGNORE_TEMPLATES: [(&str, &str); 8] = [
    ("Node", "node_modules/\ndist/\nbuild/\ncoverage/\n.env\n.env.*\nnpm-debug.log*\nyarn-error.log*\n.DS_Store\n"),
    ("Rust", "/target/\n**/*.rs.bk\n*.pdb\n"),
    ("Python", "__pycache__/\n*.py[cod]\n.venv/\nvenv/\n.env\n*.egg-info/\ndist/\nbuild/\n.pytest_cache/\n"),
    ("Java", "*.class\n*.jar\n*.war\ntarget/\nbuild/\n.gradle/\nout/\n.idea/\n*.iml\n"),
    ("VisualStudio", "bin/\nobj/\n.vs/\n*.user\n*.suo\n*.cache\npackages/\nTestResults/\n"),
    ("Go", "/bin/\n*.exe\n*.test\n*.out\nvendor/\n"),
    ("Unity", "/[Ll]ibrary/\n/[Tt]emp/\n/[Oo]bj/\n/[Bb]uild/\n/[Bb]uilds/\n/[Ll]ogs/\n/[Uu]ser[Ss]ettings/\n*.csproj\n*.sln\n"),
    ("macOS / Windows", ".DS_Store\nThumbs.db\nDesktop.ini\n"),
];

/// The MIT licence, filled in with the year and name.
pub fn mit_license(year: i32, name: &str) -> String {
    format!(
        "MIT License\n\nCopyright (c) {year} {name}\n\nPermission is hereby granted, free of charge, to any person obtaining a copy\nof this software and associated documentation files (the \"Software\"), to deal\nin the Software without restriction, including without limitation the rights\nto use, copy, modify, merge, publish, distribute, sublicense, and/or sell\ncopies of the Software, and to permit persons to whom the Software is\nfurnished to do so, subject to the following conditions:\n\nThe above copyright notice and this permission notice shall be included in all\ncopies or substantial portions of the Software.\n\nTHE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR\nIMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,\nFITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE\nAUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER\nLIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,\nOUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE\nSOFTWARE.\n"
    )
}

#[derive(Deserialize, Default)]
pub struct InitOptions {
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub gitignore: Vec<String>,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub readme: bool,
    /// Set up Git LFS for the new repository (`git lfs install --local`).
    #[serde(default)]
    pub lfs: bool,
}

/// Write the starting files and make the first commit if there are any.
pub fn init_files(dir: &Path, name: &str, opts: &InitOptions, run: &dyn Fn(&[&str]) -> GitOut) -> GitOut {
    if git::run(dir, &["rev-parse", "--verify", "HEAD"]).ok || std::fs::read_dir(dir).map(|items| items.filter_map(Result::ok).any(|e| e.file_name() != ".git")).unwrap_or(true) {
        return err("Choose an empty folder for a new repository. Existing files were not changed.");
    }
    let mut generated = Vec::new();
    let write_new = |name: &str, contents: String| -> Result<(), std::io::Error> {
        use std::io::Write;
        std::fs::OpenOptions::new().write(true).create_new(true).open(dir.join(name))?.write_all(contents.as_bytes())
    };
    let mut wrote = false;
    let mut ignore = String::new();
    for t in &opts.gitignore {
        if let Some((n, body)) = GITIGNORE_TEMPLATES.iter().find(|(n, _)| n == t) {
            ignore.push_str(&format!("# {n}\n{body}\n"));
        }
    }
    if !ignore.is_empty() {
        if let Err(e) = write_new(".gitignore", ignore) {
            return err(e.to_string());
        }
        generated.push(".gitignore");
        wrote = true;
    }
    if opts.license == "MIT" {
        let who = git::run(dir, &["config", "user.name"]).stdout.trim().to_string();
        let year: i32 = chrono::Local::now().format("%Y").to_string().parse().unwrap_or(2026);
        if let Err(e) = write_new("LICENSE", mit_license(year, if who.is_empty() { "the authors" } else { &who })) {
            return err(e.to_string());
        }
        generated.push("LICENSE");
        wrote = true;
    }
    if opts.readme {
        if let Err(e) = write_new("README.md", format!("# {name}\n")) {
            return err(e.to_string());
        }
        generated.push("README.md");
        wrote = true;
    }
    if !wrote {
        // The first commit is made even with nothing to add; when Git has no author name yet this is skipped quietly.
        let _ = run(&["commit", "-q", "--allow-empty", "-m", "Initial commit"]);
        return ok();
    }
    let mut args = vec!["add", "--"];
    args.extend(generated);
    let o = run(&args);
    if !o.ok {
        return o;
    }
    run(&["commit", "-q", "-m", "Initial commit"])
}

#[derive(Serialize)]
pub struct InitOutcome { pub path: String, pub warning: Option<String> }
pub fn initialize_lfs(dir: &Path, requested: bool, run: &dyn Fn(&[&str]) -> GitOut) -> InitOutcome {
    let warning = if requested {
        let result = run(&["lfs", "install", "--local"]);
        (!result.ok).then(|| format!("Repository created, but Git LFS setup did not finish: {}. Check Git LFS installation and the existing pre-push hook, then retry Initialize LFS in repository preferences.", result.message()))
    } else { None };
    InitOutcome { path: dir.to_string_lossy().into_owned(), warning }
}
