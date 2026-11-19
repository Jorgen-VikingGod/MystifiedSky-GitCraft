//! Keep recent file discards reachable independently of the in-memory Undo stack.
use crate::git;
use serde::Serialize;
use std::{path::Path, time::{SystemTime, UNIX_EPOCH}};

const PREFIX: &str = "refs/gitcraft/discards/";
const MAX_BACKUPS: usize = 50;
const RETENTION_SECONDS: u64 = 30 * 24 * 3600;

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

pub fn restore(dir: &Path, reference: &str) -> git::GitOut {
    if timestamp(reference).is_none() { return git::GitOut::failed("Choose a GitCraft discard backup."); }
    if !git::run(dir, &["show-ref", "--verify", "--quiet", reference]).ok {
        return git::GitOut::failed("This discard backup is no longer available.");
    }
    git::run(dir, &["stash", "apply", "--index", "-q", reference])
}
