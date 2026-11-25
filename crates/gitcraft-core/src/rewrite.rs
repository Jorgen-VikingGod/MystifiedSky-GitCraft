//! Stage 3: interactive rebase / cherry pick and merge-conflict information.
//!
//! An interactive rebase writes its own todo list and points
//! GIT_SEQUENCE_EDITOR at a small shell loop that copies that list over git's `git-rebase-todo`.

use crate::git::{self, Commit};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// GIT_SEQUENCE_EDITOR: copy our todo file line by line into git's todo file.
const SEQUENCE_EDITOR: &str = r#"while IFS= read -r line || [ -n "$line" ]; do printf '%s\n' "$line"; done < "$GITCRAFT_REBASE_TODO_PATH" >"#;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Step {
    Pick,
    Reword,
    Squash,
    Fixup,
    Edit,
    Exec,
    Drop,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TodoItem {
    pub action: Step,
    pub sha: String,
    /// Replacement commit message for reword, or shell command for exec.
    pub message: Option<String>,
}

fn git_dir(dir: &Path) -> PathBuf {
    let o = git::run(dir, &["rev-parse", "--absolute-git-dir"]);
    PathBuf::from(o.stdout.trim())
}


/// Commits an interactive rebase onto `base` would replay (oldest first). `base` "--root" = all.
pub fn rebase_commits(dir: &Path, base: &str) -> Result<Vec<Commit>, String> {
    let range = if base == "--root" { "HEAD".to_string() } else { format!("{base}..HEAD") };
    let o = git::run(dir, &["log", "--reverse", "--format=%H%x00%P%x00%an%x00%ae%x00%at%x00%s%x00%b%x00", &range]);
    if !o.ok {
        return Err(o.message());
    }
    let commits = git::parse_commits(&o.stdout);
    if commits.iter().any(|c| c.parents.len() > 1) {
        return Err("These commits include a merge. Interactive rebase over merge commits isn't supported.".into());
    }
    Ok(commits)
}

fn replay_active(git_dir: &Path) -> bool {
    ["rebase-merge", "rebase-apply"].iter().any(|name| git_dir.join(name).exists())
}

pub fn edit_stop(dir: &Path) -> bool {
    let gd = git_dir(dir);
    gd.is_absolute() && gd.join("rebase-merge/amend").is_file()
}

pub fn replay_original(dir: &Path) -> Option<(String, String)> {
    let gd = git_dir(dir);
    if !gd.is_absolute() { return None; }
    for item in ["rebase-merge", "rebase-apply"] {
        let folder = gd.join(item);
        let Ok(before) = std::fs::read_to_string(folder.join("orig-head")) else { continue; };
        let branch = std::fs::read_to_string(folder.join("head-name")).ok()?;
        let before = before.trim();
        if matches!(before.len(), 40 | 64) && before.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Some((before.to_owned(), branch.trim().strip_prefix("refs/heads/").map(|_| branch.trim().to_owned()).unwrap_or_default()));
        }
    }
    None
}

pub fn cleanup_replay_files(dir: &Path) {
    let gd = git_dir(dir);
    if !gd.is_absolute() || replay_active(&gd) { return; }
    let Ok(entries) = std::fs::read_dir(&gd) else { return; };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Ok(kind) = entry.file_type() else { continue; };
        if name.starts_with("gitcraft-replay-") && kind.is_dir() {
            let _ = std::fs::remove_dir_all(entry.path());
        } else if name == "gitcraft-rebase-todo" || name.strip_prefix("gitcraft-msg-").is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn run_replay(dir: &Path, args: &[&str], items: &[TodoItem], skip_hooks: bool) -> git::GitOut {
    let gd = git_dir(dir);
    if !gd.is_absolute() || !gd.is_dir() { return git::GitOut::failed("Could not locate the repository's Git directory."); }
    if replay_active(&gd) || gd.join("CHERRY_PICK_HEAD").exists() {
        return git::GitOut::failed("Finish or abort the current replay before starting another plan.");
    }
    cleanup_replay_files(dir);
    let files = match tempfile::Builder::new().prefix("gitcraft-replay-").tempdir_in(&gd) {
        Ok(files) => files,
        Err(error) => return git::GitOut::failed(&error.to_string()),
    };
    let todo = match write_todo(files.path(), items, skip_hooks) {
        Ok(todo) => todo,
        Err(error) => return git::GitOut::failed(&error),
    };
    let path = shell_path(&todo);
    let result = git::run_env(dir, args, &[("GIT_SEQUENCE_EDITOR", SEQUENCE_EDITOR), ("GITCRAFT_REBASE_TODO_PATH", &path)]);
    // Git can resume later and still needs the message files referenced by its todo list.
    if replay_active(&gd) { let _ = files.keep(); }
    result
}

/// Runs `git rebase -i` with our own todo list. Uncommitted changes are stashed and restored
/// around it ("Auto stash before rebase").
pub fn interactive_rebase(dir: &Path, base: &str, items: &[TodoItem], skip_hooks: bool) -> git::GitOut {
    run_replay(dir, &["-c", "core.editor=true", "rebase", "-i", "--autostash", base], items, skip_hooks)
}

fn shell_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) { text.replace('\\', "/") } else { text.into_owned() }
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn write_todo(folder: &Path, items: &[TodoItem], skip_hooks: bool) -> Result<PathBuf, String> {
    let todo = folder.join("todo");
    let git = shell_quote(&shell_path(Path::new(&crate::prefs::git_program())));
    let mut text = String::new();
    let mut picked_any = false;
    for (i, it) in items.iter().enumerate() {
        if !matches!(it.sha.len(), 40 | 64) || !it.sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("Each replay step must identify a commit by its full hash.".into());
        }
        match it.action {
            Step::Drop => text.push_str(&format!("drop {}\n", it.sha)),
            Step::Squash if picked_any => text.push_str(&format!("squash {}\n", it.sha)),
            Step::Fixup if picked_any => text.push_str(&format!("fixup {}\n", it.sha)),
            Step::Edit => {
                text.push_str(&format!("edit {}\n", it.sha));
                picked_any = true;
            }
            Step::Exec => {
                let command = it.message.as_deref().unwrap_or_default().trim();
                if command.is_empty() || command.len() > 8192 || command.contains(['\r', '\n', '\0']) {
                    return Err("A replay command must be one nonempty line of at most 8192 bytes.".into());
                }
                text.push_str(&format!("exec {command}\n"));
            }
            _ => {
                text.push_str(&format!("pick {}\n", it.sha));
                picked_any = true;
            }
        }
        if it.action == Step::Reword {
            let msg = folder.join(format!("message-{i}"));
            std::fs::write(&msg, it.message.as_deref().unwrap_or_default()).map_err(|error| error.to_string())?;
            let message = shell_quote(&shell_path(&msg));
            let flag = if skip_hooks { " --no-verify" } else { "" };
            text.push_str(&format!("exec {git} commit --amend --only --allow-empty{flag} -F {message}\n"));
        }
    }
    std::fs::write(&todo, text).map_err(|e| e.to_string())?;
    Ok(todo)
}

/// Interactive cherry pick: replay the listed commits onto HEAD in order.
pub fn interactive_cherry_pick(dir: &Path, items: &[TodoItem], skip_hooks: bool) -> git::GitOut {
    if items.iter().all(|item| item.action == Step::Drop) { return git::GitOut { ok: true, stdout: String::new(), stderr: String::new() }; }
    let head = git::run(dir, &["rev-parse", "--verify", "HEAD"]);
    if !head.ok { return head; }
    let head = head.stdout.trim();
    // Git generates an empty plan at HEAD. The sequence editor substitutes only the selected commits.
    run_replay(dir, &["-c", "core.editor=true", "rebase", "-i", "--autostash", "--onto", head, head], items, skip_hooks)
}


#[derive(Serialize, Default, Clone)]
pub struct ConflictInfo {
    pub working_token: String,
    pub decoding_loss: bool,
    /// The working-folder file with conflict markers.
    pub working: Option<String>,
    pub base: Option<String>,
    pub ours: Option<String>,
    pub theirs: Option<String>,
    /// Labels for A (current) and B (incoming), e.g. "master" / "feature/x".
    pub ours_label: String,
    pub theirs_label: String,
    pub binary: bool,
    /// How long the file's conflict markers are (its `conflict-marker-size` attribute, else 7).
    pub marker_size: usize,
}

/// The `conflict-marker-size` attribute of `file`, or Git's default of 7.
pub fn conflict_marker_size(dir: &Path, file: &str) -> usize {
    let out = crate::git::run(dir, &["check-attr", "conflict-marker-size", "--", file]);
    out.stdout.lines().next().and_then(|l| l.rsplit(": ").next()).and_then(|v| v.trim().parse::<usize>().ok()).filter(|n| *n > 0).unwrap_or(7)
}

/// The marker length to use for `text`: the attribute's, unless the attribute changed after the conflict was
/// written and the file holds only complete blocks (`<<<`, `===` and `>>>` of one equal length) of another length.
/// Incomplete marker sequences are never taken as blocks.
fn marker_size_for(attribute: usize, text: &str) -> usize {
    let run = |l: &str, ch: char| {
        let n = l.chars().take_while(|c| *c == ch).count();
        (n >= 3 && (l.len() == n || l[n..].starts_with(' '))).then_some(n)
    };
    let mut found = Vec::new();
    let mut open: Option<(usize, bool)> = None;
    for l in text.lines().map(|l| l.trim_end_matches('\r')) {
        if let Some(n) = run(l, '<') {
            open = Some((n, false));
        } else if let Some((n, _)) = open {
            if l.len() == n && l.chars().all(|c| c == '=') {
                open = Some((n, true));
            } else if let (Some(m), true) = (run(l, '>'), open.is_some_and(|o| o.1)) {
                if m == n {
                    found.push(n);
                }
                open = None;
            }
        }
    }
    if found.contains(&attribute) { attribute } else { found.first().copied().unwrap_or(attribute) }
}

/// The names shown for "current" and "incoming": during a merge HEAD's branch and MERGE_HEAD's
/// name; during a rebase the branch being rebased onto and the commit being replayed.
pub fn conflict_labels(dir: &Path) -> (String, String) {
    let gd = git_dir(dir);
    let head = git::run(dir, &["symbolic-ref", "--short", "-q", "HEAD"]).stdout.trim().to_string();
    if gd.join("MERGE_HEAD").exists() {
        let msg = std::fs::read_to_string(gd.join("MERGE_MSG")).unwrap_or_default();
        let incoming = msg
            .lines()
            .next()
            .and_then(|l| l.split('\'').nth(1))
            .map(|s| s.to_string())
            .unwrap_or_else(|| git::run(dir, &["rev-parse", "--short", "MERGE_HEAD"]).stdout.trim().to_string());
        return (if head.is_empty() { "HEAD".into() } else { head }, incoming);
    }
    for d in ["rebase-merge", "rebase-apply"] {
        let p = gd.join(d);
        if p.exists() {
            let onto = std::fs::read_to_string(p.join("onto")).unwrap_or_default();
            let onto_name = git::run(dir, &["name-rev", "--name-only", "--no-undefined", onto.trim()]).stdout.trim().to_string();
            let head_name = std::fs::read_to_string(p.join("head-name")).unwrap_or_default().trim().trim_start_matches("refs/heads/").to_string();
            return (if onto_name.is_empty() { onto.trim().chars().take(7).collect() } else { onto_name }, head_name);
        }
    }
    if gd.join("CHERRY_PICK_HEAD").exists() {
        let s = git::run(dir, &["rev-parse", "--short", "CHERRY_PICK_HEAD"]).stdout.trim().to_string();
        return (if head.is_empty() { "HEAD".into() } else { head }, s);
    }
    (if head.is_empty() { "HEAD".into() } else { head }, "incoming".into())
}

pub fn conflict_info(dir: &Path, file: &str) -> ConflictInfo {
    let working = std::fs::read(dir.join(file)).ok();
    let encoding = crate::prefs::encoding_for(dir);
    let utf16 = encoding.is_some_and(|e| e == encoding_rs::UTF_16LE || e == encoding_rs::UTF_16BE);
    let binary = !utf16 && working.as_deref().map(crate::files::is_binary).unwrap_or(false);
    let (ours_label, theirs_label) = conflict_labels(dir);
    let working_token=crate::files::content_token(working.as_deref());
    let marker_size = marker_size_for(conflict_marker_size(dir, file), working.as_deref().map(String::from_utf8_lossy).as_deref().unwrap_or(""));
    let mut decoding_loss=false;
    let mut decoded=[working,crate::files::contents(dir,":1",file),crate::files::contents(dir,":2",file),crate::files::contents(dir,":3",file)].into_iter().map(|bytes|bytes.map(|bytes| {
        let (text,loss)=crate::prefs::decode_preview(&bytes,encoding);decoding_loss|=loss;text
    })).collect::<Vec<_>>().into_iter();
    ConflictInfo {
        working_token,decoding_loss,
        working:decoded.next().unwrap(),base:decoded.next().unwrap(),ours:decoded.next().unwrap(),theirs:decoded.next().unwrap(),
        ours_label,theirs_label,binary,
        marker_size,
    }
}

/// The message git prepared for a merge commit (MERGE_MSG without comment lines).
pub fn merge_message(dir: &Path) -> Option<String> {
    let gd = git_dir(dir);
    let s = std::fs::read_to_string(gd.join("MERGE_MSG")).ok()?;
    let msg: Vec<&str> = s.lines().filter(|l| !l.starts_with('#')).collect();
    Some(msg.join("\n").trim().to_string()).filter(|m| !m.is_empty())
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PlanState { pub head: String, pub branch: String, pub base: String }
pub fn plan_state(dir: &Path, base: &str) -> Result<PlanState, String> {
    let head = git::run(dir, &["rev-parse", "--verify", "HEAD"]);
    if !head.ok { return Err(head.message()); }
    let branch = git::run(dir, &["symbolic-ref", "-q", "HEAD"]).stdout.trim().to_owned();
    let base = if base == "--root" { base.into() } else {
        let resolved = git::run(dir, &["rev-parse", "--verify", "--end-of-options", &format!("{base}^{{commit}}")]);
        if !resolved.ok { return Err(resolved.message()); } resolved.stdout.trim().to_owned()
    };
    Ok(PlanState { head: head.stdout.trim().into(), branch, base })
}
