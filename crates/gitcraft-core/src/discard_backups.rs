//! Keep recent file discards reachable independently of the in-memory Undo stack.
use crate::git;
use serde::Serialize;
use std::{path::Path, time::{SystemTime, UNIX_EPOCH}};

const PREFIX: &str = "refs/gitcraft/discards/";
const MAX_BACKUPS: usize = 50;
const RETENTION_SECONDS: u64 = 30 * 24 * 3600;
const FILE_RESTORE_SUBJECT: &str = "Auto stash before restore files";

#[derive(Serialize)]
pub struct DiscardBackup {
    pub reference: String,
    pub time: u64,
    pub subject: String,
}

fn timestamp(reference: &str) -> Option<u64> {
    let (time, sha) = reference.strip_prefix(PREFIX)?.split_once('-')?;
    if !matches!(sha.len(), 40 | 64) || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) { return None; }
    time.parse().ok()
}

pub fn list(dir: &Path) -> Result<Vec<DiscardBackup>, String> {
    let result = git::run(dir, &["for-each-ref", "--sort=-refname", "--format=%(refname)%00%(subject)", PREFIX]);
    if !result.ok { return Err(result.message()); }
    Ok(result.stdout.lines().filter_map(|line| {
        let (reference, subject) = line.split_once('\0')?;
        Some(DiscardBackup { time: timestamp(reference)?, reference: reference.into(), subject: subject.into() })
    }).collect())
}

pub fn save(dir: &Path, oid: &str) -> git::GitOut {
    let time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let reference = format!("{PREFIX}{time:012}-{oid}");
    if timestamp(&reference).is_none() { return git::GitOut::failed("Could not identify the discarded changes."); }
    let result = git::run(dir, &["update-ref", &reference, oid]);
    if result.ok {
        for (index, entry) in list(dir).unwrap_or_default().into_iter().enumerate() {
            if index >= MAX_BACKUPS || time.saturating_sub(entry.time) > RETENTION_SECONDS {
                let _ = git::run(dir, &["update-ref", "-d", &entry.reference]);
            }
        }
    }
    result
}

/// Keep the stash and the checkout source together. A normal stash applies against HEAD,
/// but this backup needs to be recovered over the staged historical checkout.
pub fn save_before_file_restore(dir: &Path, stash: &str, source: &str, paths: &[&str]) -> git::GitOut {
    let files = match serde_json::to_string(paths) {
        Ok(files) => files,
        Err(error) => return git::GitOut::failed(&error.to_string()),
    };
    let tree = format!("{stash}^{{tree}}");
    let commit = git::run(dir, &["commit-tree", "--no-gpg-sign", &tree, "-p", stash, "-p", source, "-m", FILE_RESTORE_SUBJECT, "-m", &files]);
    if !commit.ok { return commit; }
    save(dir, commit.stdout.trim())
}

pub fn restore(dir: &Path, reference: &str) -> git::GitOut {
    if timestamp(reference).is_none() { return git::GitOut::failed("Choose a GitCraft discard backup."); }
    if !git::run(dir, &["show-ref", "--verify", "--quiet", reference]).ok {
        return git::GitOut::failed("This discard backup is no longer available.");
    }
    let metadata = git::run(dir, &["show", "-s", "--format=%s%n%b", reference]);
    if !metadata.ok { return metadata; }
    if let Some((FILE_RESTORE_SUBJECT, files)) = metadata.stdout.split_once('\n') {
        return restore_files(dir, reference, files);
    }
    git::run(dir, &["stash", "apply", "--index", "-q", reference])
}

fn restore_files(dir: &Path, reference: &str, files: &str) -> git::GitOut {
    let Ok(paths) = serde_json::from_str::<Vec<String>>(files) else {
        return git::GitOut::failed("Could not read the files in this restore backup.");
    };
    if paths.is_empty() {
        return git::GitOut::failed("There are no files in this restore backup.");
    }
    let run = |args: &[&str]| {
        let args: Vec<_> = std::iter::once("--literal-pathspecs").chain(args.iter().copied())
            .chain(std::iter::once("--")).chain(paths.iter().map(String::as_str)).collect();
        git::run(dir, &args)
    };
    let source = format!("{reference}^2");
    let stash = format!("{reference}^1");
    let base = format!("{stash}^1");
    let index = format!("{stash}^2");
    let untracked = format!("{stash}^3");
    let index_matches = |tree: &str| run(&["diff", "--cached", "--quiet", "--no-ext-diff", tree]).ok;
    let baseline = if index_matches(&source) { Some(&source) } else if index_matches(&base) { Some(&base) } else { None };
    let Some(baseline) = baseline.filter(|_| run(&["diff", "--quiet", "--no-ext-diff"]).ok) else {
        return git::GitOut::failed("The files have changed since the restore. Save those changes before recovering this backup.");
    };
    // Untracked files must not be in the recovered index. Read their paths before changing it.
    let others = run(&["ls-tree", "-r", "-z", "--name-only", &untracked]);
    if !others.ok { return others; }
    let untracked_paths: Vec<_> = others.stdout.split('\0').filter(|path| !path.is_empty()).collect();
    // If the historical checkout has already been undone, a new untracked file could be
    // where the backup needs to write. Refuse before updating any of the files.
    let tracked = run(&["ls-files", "-z"]);
    if !tracked.ok { return tracked; }
    let tracked: std::collections::HashSet<_> = tracked.stdout.split('\0').collect();
    if paths.iter().any(|path| !tracked.contains(path.as_str()) && dir.join(path).symlink_metadata().is_ok()) {
        return git::GitOut::failed("An untracked file would be overwritten. Move it before recovering this backup.");
    }
    let recover = || {
        // Restore data first while the checkout's index entries still identify deleted paths.
        let work = run(&["restore", "--worktree", "--source", &stash]);
        if !work.ok { return work; }
        if !untracked_paths.is_empty() {
            let mut args = vec!["--literal-pathspecs", "restore", "--worktree", "--source", &untracked, "--"];
            args.extend(untracked_paths);
            let work = git::run(dir, &args);
            if !work.ok { return work; }
        }
        run(&["restore", "--staged", "--source", &index])
    };
    let result = recover();
    if !result.ok {
        // A failed index write leaves the index intact. Put the working files back too so
        // the operation can be retried from the same checkout.
        let rollback = run(&["restore", "--worktree", "--source", baseline]);
        if !rollback.ok {
            return git::GitOut::failed(&format!("{}\nCould not reset the working files after recovery failed: {}\nThe backup is still available.", result.message(), rollback.message()));
        }
    }
    result
}
