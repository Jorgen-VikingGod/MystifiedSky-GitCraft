//! Stage 6 local stand-ins for cloud features:
//! - Patches: share changes as a .patch file (instead of hosted cloud patches).
//! - Conflict prevention: warn early when your branch would conflict with its target
//!   (proactive conflict detection with the target branch), computed locally
//!   with `git merge-tree` — nothing leaves the computer.

use crate::git;
use crate::metrics::{DIFF_CONTEXT_LINES, DIFF_INTERHUNK_LINES, MERGE_RENAME_LIMIT, MERGE_RENAME_THRESHOLD, PATCH_START_NUMBER, PATCH_SUBJECT_PREFIX};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// A patch of uncommitted changes (`wip`) or of the given commits (oldest first, mailbox format).
pub fn make_patch(dir: &Path, shas: &[String]) -> Result<Vec<u8>, String> {
    let patch_bytes = |args: &[&str]| {
        let output = crate::read_jobs::run_bytes(git::git_command(dir, args), 64 * 1024 * 1024, None);
        if output.ok { Ok(output.stdout) }
        else { Err(git::redact_diagnostic(&String::from_utf8_lossy(&output.stderr))) }
    };
    // Keep the hunk boundaries unchanged in every export.
    let context = format!("-U{DIFF_CONTEXT_LINES}");
    let interhunk = format!("--inter-hunk-context={DIFF_INTERHUNK_LINES}");
    if shas.is_empty() {
        let baseline = if git::run(dir, &["rev-parse", "--verify", "HEAD"]).ok { "HEAD".into() } else {
            let empty = git::run(dir, &["hash-object", "-w", "-t", "tree", "--stdin"]);
            if !empty.ok { return Err(empty.message()); } empty.stdout.trim().to_owned()
        };
        let data = patch_bytes(&["diff", &baseline, "--binary", "--no-color", "--no-ext-diff", "--no-textconv", &context, &interhunk])?;
        if data.is_empty() {
            return Err("There are no uncommitted changes to put in a patch (new files must be staged first).".into());
        }
        return Ok(data);
    }
    let mut out = Vec::new();
    let total = shas.len();
    // The subject prefix is PATCH, numbering starts at 1, and there is no reroll suffix by default.
    // Override personal format.* settings before adding the series numbering ourselves.
    let prefix = format!("--subject-prefix={PATCH_SUBJECT_PREFIX}");
    let start = format!("--start-number={PATCH_START_NUMBER}");
    for (i, sha) in shas.iter().enumerate() {
        let parents = git::run(dir, &["rev-list", "--parents", "-n", "1", sha]);
        if !parents.ok { return Err(parents.message()); }
        if parents.stdout.split_whitespace().count() > 2 {
            return Err("Merge commits cannot be exported as mailbox patches. Choose the individual commits instead.".into());
        }
        let mut data = patch_bytes(&["format-patch", "-1", "--stdout", "--binary", &context, &interhunk, &prefix, "--no-numbered", "--no-cover-letter", &start, sha])?;
        // Native indices are one-based: number = i_native - 1 + S, total = N - 1 + S.
        number_subject(&mut data, i + PATCH_START_NUMBER, total - 1 + PATCH_START_NUMBER);
        if out.len() + data.len() > 64 * 1024 * 1024 {
            return Err("The patch series exceeds the 64 MB export limit. Select fewer commits.".into());
        }
        out.extend(data);
    }
    Ok(out)
}

/// The email subject: "Subject: [PATCH] …" for one patch,
/// "[PATCH n/total]" when there are several (start number 1).
fn number_subject(patch: &mut Vec<u8>, n: usize, total: usize) {
    if total <= 1 { return; }
    let prefix = format!("Subject: [{PATCH_SUBJECT_PREFIX}] ");
    let mut offset = 0;
    for line in patch.split_inclusive(|byte| *byte == b'\n') {
        if matches!(line, b"\n" | b"\r\n") { break; }
        if line.starts_with(prefix.as_bytes()) {
            let numbered = format!("Subject: [{PATCH_SUBJECT_PREFIX} {n}/{total}] ");
            patch.splice(offset..offset + prefix.len(), numbered.into_bytes());
            return;
        }
        offset += line.len();
    }
}

/// A zero merge limit selects diff.renamelimit; a nonpositive result selects 1000.
fn rename_limit(dir: &Path) -> u32 {
    let read = |key: &str| -> i32 {
        let o = git::run(dir, &["config", "--int", "--get", key]);
        if o.ok { o.stdout.trim().parse().unwrap_or(0) } else { 0 }
    };
    // diff.renamelimit is used only when merge.renamelimit is zero. A negative merge
    // value goes straight to the default; it must not select diff.renamelimit.
    let merge = read("merge.renamelimit");
    let limit = if merge == 0 { read("diff.renamelimit") } else { merge };
    if limit > 0 { limit as u32 } else { MERGE_RENAME_LIMIT }
}

/// Shared merge settings: rename detection is on,
/// similarity defaults to 50%, and the candidate limit follows rename_limit above.
/// Use for previews and merges so personal merge.renames settings cannot make them disagree.
pub fn merge_args(dir: &Path, command: &str, args: &[&str]) -> Vec<String> {
    let mut out = merge_config(dir);
    out.extend([command.into(), "-X".into(), format!("find-renames={MERGE_RENAME_THRESHOLD}%")]);
    out.extend(args.iter().map(|s| (*s).to_owned()));
    out
}

fn merge_config(dir: &Path) -> Vec<String> {
    vec!["-c".into(), "merge.renames=true".into(), "-c".into(), format!("merge.renameLimit={}", rename_limit(dir))]
}

#[derive(Serialize)]
pub struct HistoryFetchInfo {
    pub shallow: bool,
    pub remotes: Vec<String>,
    pub promisor_remotes: Vec<String>,
}

pub fn history_fetch_info(dir: &Path) -> Result<HistoryFetchInfo, String> {
    let shallow = git::run(dir, &["rev-parse", "--is-shallow-repository"]);
    if !shallow.ok { return Err(shallow.message()); }
    let output = git::run(dir, &["remote"]);
    if !output.ok { return Err(output.message()); }
    let mut remotes: Vec<String> = output.stdout.lines().map(str::to_owned).collect();
    let head = git::run(dir, &["symbolic-ref", "--short", "-q", "HEAD"]);
    let upstream = git::run(dir, &["config", "--get", &format!("branch.{}.remote", head.stdout.trim())]);
    if let Some(index) = remotes.iter().position(|name| name == upstream.stdout.trim())
        .or_else(|| remotes.iter().position(|name| name == "origin")) {
        remotes.swap(0, index);
    }
    let legacy = git::run(dir, &["config", "--get", "extensions.partialClone"]);
    let promisor_remotes = remotes.iter().filter(|name| {
        legacy.stdout.trim() == name.as_str()
            || git::run(dir, &["config", "--bool", "--get", &format!("remote.{name}.promisor")]).stdout.trim() == "true"
    }).cloned().collect();
    Ok(HistoryFetchInfo { shallow: shallow.stdout.trim() == "true", remotes, promisor_remotes })
}

#[derive(Serialize)]
pub struct ReflogEntry {
    sha: String,
    selector: String,
    message: String,
}

pub fn reflog(dir: &Path) -> Result<Vec<ReflogEntry>, String> {
    let output = git::run_limited(dir, &["reflog", "show", "--all", "-n", "200", "--date=iso", "--format=%H%x00%gD%x00%gs"], 1024 * 1024);
    if !output.ok { return Err(output.message()); }
    Ok(output.stdout.lines().filter_map(|line| {
        let mut columns = line.splitn(3, '\0');
        Some(ReflogEntry { sha: columns.next()?.to_owned(), selector: columns.next()?.to_owned(), message: columns.next()?.to_owned() })
    }).collect())
}

pub fn commit_template(dir: &Path) -> Result<Option<(String, String)>, String> {
    let template = git::run(dir, &["config", "--path", "--get", "commit.template"]);
    if !template.ok || template.stdout.trim_end_matches(['\r', '\n']).is_empty() { return Ok(None); }
    let path = dir.join(template.stdout.trim_end_matches(['\r', '\n']));
    let file = std::fs::File::open(path).map_err(|error| format!("Could not open the Git commit template: {error}"))?;
    use std::io::Read;
    const LIMIT: u64 = 64 * 1024;
    let mut data = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut data).map_err(|error| error.to_string())?;
    if data.len() as u64 > LIMIT || data.contains(&0) { return Err("Git commit templates must be text under 64 KB.".into()); }
    let cleaned = crate::read_jobs::run_bytes(git::git_command(dir, &["stripspace", "--strip-comments"]), LIMIT as usize, Some(data));
    if !cleaned.ok { return Err(String::from_utf8_lossy(&cleaned.stderr).trim().to_owned()); }
    let text = String::from_utf8(cleaned.stdout).map_err(|_| "The Git commit template is not UTF-8.")?;
    let (summary, description) = text.trim_end().split_once('\n').unwrap_or((text.trim_end(), ""));
    if summary.is_empty() { return Ok(None); }
    Ok(Some((summary.to_owned(), description.trim().to_owned())))
}

/// Apply a patch file: mailbox patches become commits (`git am`), plain diffs change the working folder.
pub fn apply_patch_file(dir: &Path, file: &Path, run: &dyn Fn(&[&str]) -> git::GitOut) -> git::GitOut {
    use std::io::{BufRead, BufReader};
    let source = std::fs::File::open(file).map(BufReader::new);
    let mailbox = match source.and_then(|mut source| source.fill_buf().map(|bytes| bytes.starts_with(b"From "))) {
        Ok(mailbox) => mailbox,
        Err(error) => return git::GitOut::failed(&format!("Could not read the patch file: {error}")),
    };
    let Some(f) = file.to_str() else { return git::GitOut::failed("The patch filename contains unsupported characters."); };
    if mailbox {
        for state in ["rebase-apply", "rebase-merge", "MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD"] {
            let path = git::run(dir, &["rev-parse", "--git-path", state]);
            let path = dir.join(path.stdout.trim());
            if path.exists() {
                return git::GitOut::failed("Finish or abort the current Git operation before importing a mailbox patch.");
            }
        }
        // Keep a newly conflicted session so the user can resolve or explicitly abort it.
        run(&["am", "--3way", "--", f])
    } else {
        run(&["apply", "--3way", "--", f])
    }
}

#[derive(Serialize, Default)]
pub struct RepoSummary {
    /// False when the folder is gone or isn't a repository ("Repo is unreachable and may have been deleted").
    pub exists: bool,
    pub branch: Option<String>,
    /// Changed files in the working folder (the "WIP summary").
    pub changes: usize,
    pub ahead: u32,
    pub behind: u32,
}

/// Branch, uncommitted-change count and ahead / behind for the Repository Management list.
pub fn repo_summary(dir: &Path) -> RepoSummary {
    if !dir.is_dir() {
        return RepoSummary::default();
    }
    // Use the same recursion and rename rules as the working-directory lists.
    // NUL records keep filenames containing line breaks from inflating the count.
    let options = git::status_options();
    let o = git::run(dir, &["--no-optional-locks", "status", "--porcelain=v2", "--branch", "-z", &options[0], &options[1]]);
    if !o.ok {
        return RepoSummary::default();
    }
    let mut s = RepoSummary { exists: true, ..Default::default() };
    let mut records = o.stdout.split('\0');
    while let Some(line) = records.next() {
        if let Some(b) = line.strip_prefix("# branch.head ") {
            s.branch = (b != "(detached)").then(|| b.to_string());
        } else if let Some(ab) = line.strip_prefix("# branch.ab ") {
            let mut it = ab.split_whitespace();
            s.ahead = it.next().and_then(|x| x.trim_start_matches('+').parse().ok()).unwrap_or(0);
            s.behind = it.next().and_then(|x| x.trim_start_matches('-').parse().ok()).unwrap_or(0);
        } else if line.starts_with("2 ") {
            s.changes += 1;
            records.next(); // A renamed/copied file has a second NUL record for its old path.
        } else if line.starts_with("1 ") || line.starts_with("u ") || line.starts_with("? ") {
            // Each porcelain entry represents a path, even when both index and worktree changed.
            s.changes += 1;
        }
    }
    s
}

#[derive(Serialize)]
pub struct PullRequestTemplate {
    pub name: String,
    pub body: String,
}

pub fn pr_templates(dir: &Path) -> Vec<PullRequestTemplate> {
    use std::io::Read;
    let Ok(root) = dir.canonicalize() else { return Vec::new(); };
    let mut candidates: Vec<PathBuf> = [
        ".github/pull_request_template.md",
        ".github/PULL_REQUEST_TEMPLATE.md",
        "pull_request_template.md",
        "PULL_REQUEST_TEMPLATE.md",
        "docs/pull_request_template.md",
        "docs/PULL_REQUEST_TEMPLATE.md",
        ".gitlab/merge_request_templates/Default.md",
    ].iter().map(|path| root.join(path)).collect();
    for folder in [".github/PULL_REQUEST_TEMPLATE", ".gitlab/merge_request_templates"] {
        if let Ok(directory) = std::fs::read_dir(root.join(folder)) {
            let mut more: Vec<_> = directory.flatten().map(|item| item.path())
                .filter(|path| path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("md")))
                .collect();
            more.sort();
            candidates.extend(more.into_iter().take(32));
        }
    }
    let mut seen = std::collections::HashSet::new();
    candidates.into_iter().filter_map(|candidate| {
        let path = candidate.canonicalize().ok()?;
        if !path.starts_with(&root) || !path.is_file() || !seen.insert(path.clone()) { return None; }
        const LIMIT: u64 = 32 * 1024;
        let mut text = String::new();
        std::fs::File::open(path).ok()?.take(LIMIT + 1).read_to_string(&mut text).ok()?;
        if text.len() as u64 > LIMIT || text.trim().is_empty() { return None; }
        let name = candidate.strip_prefix(&root).ok()?.to_string_lossy().replace('\\', "/");
        Some(PullRequestTemplate { name, body: text.trim().to_owned() })
    }).take(32).collect()
}

pub fn range_diff(dir: &Path, before: &str, after: &str) -> Result<String, String> {
    let resolve = |range: &str| -> Result<String, String> {
        let (base, tip) = range.trim().split_once("..")
            .filter(|(base, tip)| !base.trim().is_empty() && !tip.trim().is_empty() && !tip.starts_with('.') && !tip.contains(".."))
            .ok_or("Enter each range as base..tip, using two commit IDs, branches, or tags.")?;
        let commit = |reference: &str| -> Result<String, String> {
            let expression = format!("{}^{{commit}}", reference.trim());
            let result = git::run(dir, &["rev-parse", "--verify", "-q", "--end-of-options", &expression]);
            if result.ok { Ok(result.stdout.trim().to_owned()) }
            else { Err(format!("Git could not resolve {} to a commit.", reference.trim())) }
        };
        Ok(format!("{}..{}", commit(base)?, commit(tip)?))
    };
    let before = resolve(before)?;
    let after = resolve(after)?;
    let result = git::run_limited(dir, &["range-diff", "--no-color", "--no-ext-diff", "--no-textconv", &before, &after], 2 * 1024 * 1024);
    if result.ok { Ok(result.stdout) } else { Err(result.message()) }
}

#[derive(Serialize, Default)]
pub struct ConflictCheck {
    pub target: String,
    pub files: Vec<String>,
    /// The check could not run (no target branch, or Git too old).
    pub note: Option<String>,
}

/// Prefer the default branch advertised by the current branch's upstream remote.
pub fn default_target(dir: &Path) -> Option<String> {
    let head = git::run(dir, &["symbolic-ref", "--short", "-q", "HEAD"]);
    default_branch_for(dir, head.stdout.trim()).map(|(tracking, _)| tracking)
}

pub fn default_branch_for(dir: &Path, branch: &str) -> Option<(String, String)> {
    let upstream = git::run(dir, &["config", "--get", &format!("branch.{branch}.remote")]);
    let remotes = git::run(dir, &["remote"]);
    let valid = |reference: &str| git::run(dir, &["rev-parse", "--verify", "-q", "--end-of-options", &format!("{reference}^{{commit}}")]).ok;
    let preferred = upstream.stdout.trim();
    let mut names = Vec::new();
    if !preferred.is_empty() && preferred != "." { names.push(preferred); }
    if !names.contains(&"origin") { names.push("origin"); }
    for remote in remotes.stdout.lines() {
        if !names.contains(&remote) { names.push(remote); }
    }
    for remote in names.iter() {
        let head = git::run(dir, &["symbolic-ref", "-q", &format!("refs/remotes/{remote}/HEAD")]);
        if let Some(branch) = head.stdout.trim().strip_prefix(&format!("refs/remotes/{remote}/")).filter(|_| head.ok && valid(head.stdout.trim())) {
            return Some((format!("{remote}/{branch}"), branch.to_owned()));
        }
    }
    for remote in names {
        for branch in ["main", "master"] {
            let target = format!("{remote}/{branch}");
            if valid(&format!("refs/remotes/{target}")) { return Some((target, branch.to_owned())); }
        }
    }
    ["main", "master"].into_iter().find(|branch| valid(&format!("refs/heads/{branch}"))).map(|branch| (branch.to_owned(), branch.to_owned()))
}

/// Files that would conflict if HEAD were merged with `target` (no working-folder changes).
pub fn conflict_check(dir: &Path, target: Option<&str>) -> ConflictCheck {
    let Some(target) = target.map(|s| s.to_string()).or_else(|| default_target(dir)) else {
        return ConflictCheck { note: Some("No target branch to compare with.".into()), ..Default::default() };
    };
    let mut args = merge_config(dir);
    // Git 2.38 supports merge-tree. Its default similarity threshold is
    // already 50%, matching the threshold passed explicitly to git merge.
    args.extend(["merge-tree", "--write-tree", "--name-only", "--no-messages", "-z", "--", "HEAD", &target].map(str::to_owned));
    let o = git::run(dir, &args.iter().map(String::as_str).collect::<Vec<_>>());
    // Exit 0 means clean. On conflict, Git emits the tree ID followed by NUL-delimited paths.
    if o.ok {
        return ConflictCheck { target, ..Default::default() };
    }
    if let Some((tree, paths)) = o.stdout.split_once('\0') {
        if matches!(tree.len(), 40 | 64) && tree.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            let files: Vec<String> = paths.split_terminator('\0').filter(|path| !path.is_empty()).map(str::to_owned).collect();
            let note = files.is_empty().then(|| "Git reported a merge conflict without naming a file.".into());
            return ConflictCheck { target, files, note };
        }
    }
    ConflictCheck { target, files: vec![], note: Some(o.message()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn patches_and_conflict_check() {
        let d = std::env::temp_dir().join(format!("gitcraft-extras-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        let g = |a: &[&str]| assert!(git::run(&d, a).ok, "{a:?}");
        g(&["init", "-q", "-b", "main"]);
        g(&["config", "user.name", "T"]);
        g(&["config", "user.email", "t@e.x"]);
        g(&["config", "commit.gpgsign", "false"]);
        fs::write(d.join(" café.txt"), "base\n").unwrap();
        g(&["add", "-A"]);
        g(&["commit", "-q", "-m", "base"]);
        g(&["checkout", "-q", "-b", "side"]);
        fs::write(d.join(" café.txt"), "side\n").unwrap();
        g(&["commit", "-q", "-am", "side change"]);
        let side = git::run(&d, &["rev-parse", "HEAD"]).stdout.trim().to_string();
        g(&["checkout", "-q", "main"]);
        // A clean comparison, then a conflicting one.
        assert!(conflict_check(&d, Some("side")).files.is_empty());
        fs::write(d.join(" café.txt"), "main\n").unwrap();
        g(&["commit", "-q", "-am", "main change"]);
        assert_eq!(conflict_check(&d, Some("side")).files, vec![" café.txt".to_string()]);

        // Personal email settings must not override the default subject rules.
        g(&["config", "format.subjectPrefix", "CUSTOM"]);
        g(&["config", "format.numbered", "true"]);
        g(&["config", "format.coverLetter", "true"]);
        // Patch of a commit, applied as a commit on another branch.
        let patch = String::from_utf8(make_patch(&d, std::slice::from_ref(&side)).unwrap()).unwrap();
        assert!(patch.starts_with("From "));
        assert!(patch.lines().any(|l| l == "Subject: [PATCH] side change"), "{patch}");
        assert_eq!(patch.lines().filter(|l| l.starts_with("Subject: ")).count(), 1);
        let series = String::from_utf8(make_patch(&d, &[side.clone(), side.clone()]).unwrap()).unwrap();
        assert!(series.lines().any(|l| l == "Subject: [PATCH 1/2] side change"), "{series}");
        assert!(series.lines().any(|l| l == "Subject: [PATCH 2/2] side change"), "{series}");
        assert_eq!(series.lines().filter(|l| l.starts_with("Subject: ")).count(), 2);
        let pf = d.join("..").join(format!("gitcraft-extras-{}.patch", std::process::id()));
        fs::write(&pf, patch).unwrap();
        g(&["checkout", "-q", "-b", "other", "main~1"]);
        assert!(apply_patch_file(&d, &pf, &|a: &[&str]| git::run(&d, a)).ok);
        assert_eq!(git::run(&d, &["log", "-1", "--format=%s"]).stdout.trim(), "side change");
        // Patch of uncommitted work.
        fs::write(d.join(" café.txt"), "wip\n").unwrap();
        assert!(String::from_utf8(make_patch(&d, &[]).unwrap()).unwrap().contains("+wip"));
        let _ = fs::remove_file(&pf);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn patch_exports_keep_hunk_boundaries() {
        let d = std::env::temp_dir().join(format!("gitcraft-patch-context-{}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        let g = |a: &[&str]| assert!(git::run(&d, a).ok, "{a:?}");
        g(&["init", "-q", "-b", "main"]);
        g(&["config", "user.name", "T"]);
        g(&["config", "user.email", "t@e.x"]);
        g(&["config", "commit.gpgsign", "false"]);
        let base: String = (1..=20).map(|i| format!("line {i}\n")).collect();
        fs::write(d.join("context.txt"), &base).unwrap();
        g(&["add", "-A"]);
        g(&["commit", "-q", "-m", "base"]);
        // These settings would remove context and join both changes without explicit overrides.
        g(&["config", "diff.context", "0"]);
        g(&["config", "diff.interHunkContext", "100"]);
        let changed = base.replace("line 5\n", "changed 5\n").replace("line 15\n", "changed 15\n");
        fs::write(d.join("context.txt"), changed).unwrap();
        let check = |patch: Vec<u8>| {
            let patch = String::from_utf8(patch).unwrap();
            assert_eq!(patch.lines().filter(|l| l.starts_with("@@ ")).count(), 2, "{patch}");
            assert!(patch.contains("@@ -2,7 +2,7 @@"), "{patch}");
            assert!(patch.contains("@@ -12,7 +12,7 @@"), "{patch}");
        };
        check(make_patch(&d, &[]).unwrap());
        g(&["commit", "-q", "-am", "two changes"]);
        let sha = git::run(&d, &["rev-parse", "HEAD"]).stdout.trim().to_string();
        check(make_patch(&d, &[sha]).unwrap());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn rename_limit_uses_zero_and_negative_rules() {
        let d = std::env::temp_dir().join(format!("gitcraft-merge-limits-{}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        let g = |a: &[&str]| assert!(git::run(&d, a).ok, "{a:?}");
        g(&["init", "-q", "-b", "main"]);
        // Set both keys so inherited user configuration cannot affect the cases.
        g(&["config", "merge.renamelimit", "0"]);
        g(&["config", "diff.renamelimit", "77"]);
        assert_eq!(rename_limit(&d), 77);
        g(&["config", "merge.renamelimit", "-1"]);
        assert_eq!(rename_limit(&d), 1000);
        g(&["config", "merge.renamelimit", "2k"]);
        assert_eq!(rename_limit(&d), 2048);
        g(&["config", "merge.renamelimit", "0"]);
        g(&["config", "diff.renamelimit", "-1"]);
        assert_eq!(rename_limit(&d), 1000);
        g(&["config", "diff.renamelimit", "0"]);
        assert_eq!(rename_limit(&d), 1000);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn patch_subjects_are_numbered_like_libgit2() {
        let p = "From abc Mon Sep 17 00:00:00 2001\nSubject: [PATCH] Fix it\n";
        let mut bytes = p.as_bytes().to_vec();
        number_subject(&mut bytes, 1, 1);
        assert_eq!(bytes, p.as_bytes());
        number_subject(&mut bytes, 2, 3);
        assert!(String::from_utf8(bytes).unwrap().contains("Subject: [PATCH 2/3] Fix it"));
    }

    #[test]
    fn summary_of_a_missing_folder_and_a_repo() {
        assert!(!repo_summary(Path::new("Z:/surely/not/here")).exists);
        let d = std::env::temp_dir().join(format!("gitcraft-summary-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        assert!(git::run(&d, &["init", "-q", "-b", "main"]).ok);
        fs::write(d.join("x.txt"), "x").unwrap();
        let s = repo_summary(&d);
        assert!(s.exists && s.changes == 1 && s.branch.as_deref() == Some("main"));
        // Personal settings must not hide files or collapse a new directory into one change.
        assert!(git::run(&d, &["config", "status.showUntrackedFiles", "no"]).ok);
        fs::create_dir_all(d.join("nested")).unwrap();
        fs::write(d.join("nested/a.txt"), "a").unwrap();
        fs::write(d.join("nested/b.txt"), "b").unwrap();
        assert_eq!(repo_summary(&d).changes, 3);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn status_finds_staged_renames_even_when_config_turns_them_off() {
        let d = std::env::temp_dir().join(format!("gitcraft-renames-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        let g = |a: &[&str]| assert!(git::run(&d, a).ok, "{a:?}");
        g(&["init", "-q", "-b", "main"]);
        g(&["config", "user.name", "T"]);
        g(&["config", "user.email", "t@e.x"]);
        g(&["config", "commit.gpgsign", "false"]);
        g(&["config", "status.renames", "false"]);
        fs::write(d.join("old.txt"), "one\ntwo\nthree\nfour\n").unwrap();
        g(&["add", "-A"]);
        g(&["commit", "-q", "-m", "base"]);
        g(&["mv", "old.txt", "new.txt"]);
        let s = git::status(&d);
        assert_eq!(s.staged.len(), 1, "{:?}", s.staged);
        assert_eq!((s.staged[0].path.as_str(), s.staged[0].status), ("new.txt", 'R'));
        // The repository summary must consume the old-path record and count this rename once.
        assert_eq!(repo_summary(&d).changes, 1);
        fs::write(d.join("new.txt"), "edited after staging\n").unwrap();
        assert_eq!(repo_summary(&d).changes, 1);
        let _ = fs::remove_dir_all(&d);
    }

}
