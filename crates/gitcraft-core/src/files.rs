//! Stage 2: file contents at any point, partial (hunk / line) staging, file history,
//! blame, per-commit change counts and comparisons between two points.

use crate::git::{self, run, Commit, DiffSource, FileChange, WORKDIR};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Runs git and returns raw stdout bytes (file contents may not be UTF-8).
fn run_bytes(dir: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = crate::read_jobs::run_bytes(git::git_command(dir, args), 64 * 1024 * 1024, None);
    output.ok.then_some(output.stdout)
}

/// Contents of `file` at `rev`: a commit sha, ":" for the index (staging area), or WORKDIR.
pub fn contents(dir: &Path, rev: &str, file: &str) -> Option<Vec<u8>> {
    if rev == WORKDIR {
        return std::fs::read(dir.join(file)).ok();
    }
    let spec = if rev == ":" { format!(":{file}") } else { format!("{rev}:{file}") };
    run_bytes(dir, &["show", &spec])
}

/// Automatic previews are bounded before bytes cross the desktop bridge.
pub const PREVIEW_MAX: usize = 2 * 1024 * 1024;
pub const LARGE_PREVIEW_MAX: usize = 20 * 1024 * 1024;

fn preview_contents(dir: &Path, rev: &str, file: &str, limit: usize) -> Result<Option<Vec<u8>>, ()> {
    use std::io::Read;
    if rev == WORKDIR {
        let Ok(input) = std::fs::File::open(dir.join(file)) else { return Ok(None) };
        if input.metadata().map(|m| m.len() > limit as u64).unwrap_or(false) { return Err(()) }
        let mut bytes = Vec::new();
        if input.take(limit as u64 + 1).read_to_end(&mut bytes).is_err() { return Ok(None) }
        return if bytes.len() > limit { Err(()) } else { Ok(Some(bytes)) };
    }
    let spec = if rev == ":" { format!(":{file}") } else { format!("{rev}:{file}") };
    let size = run(dir, &["cat-file", "-s", &spec]);
    if !size.ok { return Ok(None) }
    if size.stdout.trim().parse::<u64>().map(|n| n > limit as u64).unwrap_or(true) { return Err(()) }
    let mut command = git::git_cmd(dir); command.args(["show", &spec]);
    let output = crate::read_jobs::run_bytes(command, limit, None);
    if output.ok { Ok(Some(output.stdout)) }
    else if String::from_utf8_lossy(&output.stderr).contains("budget") { Err(()) }
    else { Ok(None) }
}

#[derive(Serialize, Default)]
pub struct FileVersions {
    pub new_token: Option<String>,
    pub too_large: bool,
    pub preview_limit: usize,
    pub old: Option<String>,
    pub new: Option<String>,
    pub binary: bool,
    /// Image previews as data URLs (before / after) when the file is a picture.
    pub old_image: Option<String>,
    pub new_image: Option<String>,
    pub encoding: String,
    /// The working-folder copy can be edited and saved.
    pub editable: bool,
    pub lockable: bool,
    pub decoding_loss: bool,
}

const IMAGE_TYPES: [(&str, &str); 8] = [
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("bmp", "image/bmp"),
    ("webp", "image/webp"),
    ("ico", "image/x-icon"),
    ("svg", "image/svg+xml"),
];

/// A file is binary by libgit2's rules:
/// look at the first 8000 bytes; a UTF-16 / UTF-32 byte-order mark means binary; a UTF-8 one is
/// skipped; a NUL byte means binary; otherwise binary when (printable >> 7) < non-printable.
/// TAB, LF, VT and CR count as neither; BS, FF and ESC count as printable.
pub fn is_binary(data: &[u8]) -> bool {
    use crate::metrics::{BINARY_CHECK_BYTES, BINARY_PRINTABLE_SHIFT};
    let data = &data[..data.len().min(BINARY_CHECK_BYTES)];
    let mut start = 0;
    if data.len() > 1 {
        match data {
            [0xEF, 0xBB, 0xBF, ..] => start = 3,                    // UTF-8: still text
            [0xFE, 0xFF, ..] | [0xFF, 0xFE, ..] => return true,      // UTF-16 (and UTF-32 LE)
            [0x00, 0x00, 0xFE, 0xFF, ..] => return true,             // UTF-32 BE
            _ => {}
        }
    }
    let (mut printable, mut nonprintable) = (0u32, 0u32);
    for &b in &data[start..] {
        match b {
            0 => return true,
            8 | 12 | 27 => printable += 1,
            9 | 10 | 11 | 13 => {} // TAB, LF, VT and CR are neutral.
            0x7f => nonprintable += 1,
            b if b < 0x20 => nonprintable += 1,
            _ => printable += 1,
        }
    }
    (printable >> BINARY_PRINTABLE_SHIFT) < nonprintable
}

fn mime_of(file: &str) -> Option<&'static str> {
    let ext = file.rsplit('.').next()?.to_ascii_lowercase();
    IMAGE_TYPES.iter().find(|(e, _)| *e == ext).map(|(_, m)| *m)
}

/// The two sides ("before" and "after") of a file for a diff source.
pub fn sides(src: &DiffSource, parent: Option<&str>) -> (Option<String>, String) {
    match src {
        DiffSource::Unstaged => (Some(":".into()), WORKDIR.into()),
        DiffSource::Staged => (Some("HEAD".into()), ":".into()),
        DiffSource::Commit(sha) => (parent.map(|p| p.to_string()), sha.clone()),
        DiffSource::Range(a, b) => (Some(a.clone()), b.clone()),
    }
}

#[cfg(test)]
pub fn versions(dir: &Path, src: &DiffSource, file: &str, parent: Option<&str>, untracked: bool) -> FileVersions {
    versions_with_limit(dir, src, file, parent, untracked, false)
}

pub fn versions_with_limit(dir: &Path, src: &DiffSource, file: &str, parent: Option<&str>, untracked: bool, allow_large: bool) -> FileVersions {
    let limit = if allow_large { LARGE_PREVIEW_MAX } else { PREVIEW_MAX };
    let (old_rev, new_rev) = sides(src, parent);
    let original = if untracked { file.to_owned() } else { git::rename_source(dir, src, file, parent) };
    let old = if untracked { Ok(None) } else { old_rev.as_deref().map(|r| preview_contents(dir, r, &original, limit)).unwrap_or(Ok(None)) };
    let new = preview_contents(dir, &new_rev, file, limit);
    let lockable = new_rev == WORKDIR && crate::manage::lfs_lockable(dir, file).unwrap_or(false);
    let (Ok(old), Ok(new)) = (old, new) else {
        return FileVersions { too_large: true, preview_limit: limit, lockable, ..Default::default() };
    };
    let mut v = FileVersions { encoding: "UTF-8".into(), editable: new_rev == WORKDIR, lockable, ..Default::default() };
    if new_rev == WORKDIR { v.new_token = Some(content_token(new.as_deref())); }
    if let Some(mime) = mime_of(file) {
        let enc = |b: &Vec<u8>| format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(b));
        v.old_image = old.as_ref().map(enc);
        v.new_image = new.as_ref().map(enc);
        v.binary = true;
        v.editable = false;
        return v;
    }
    // Repo-Specific > Encoding (UTF-16 files contain zero bytes, so they aren't treated as binary then).
    let enc = crate::prefs::encoding_for(dir);
    v.encoding=new.as_deref().and_then(encoding_rs::Encoding::for_bom).map(|(encoding,_)|encoding).or(enc).unwrap_or(encoding_rs::UTF_8).name().to_owned();
    let utf16 = enc.map(|e| e == encoding_rs::UTF_16LE || e == encoding_rs::UTF_16BE).unwrap_or(false);
    let is_bin = |b: &Option<Vec<u8>>| !utf16 && b.as_ref().map(|x| is_binary(x)).unwrap_or(false);
    if is_bin(&old) || is_bin(&new) {
        v.binary = true;
        v.editable = false;
        return v;
    }
    let decode = |bytes: Option<Vec<u8>>| bytes.map(|bytes|crate::prefs::decode_preview(&bytes,enc));
    let old=decode(old);let new=decode(new);
    v.decoding_loss=old.as_ref().is_some_and(|(_,loss)|*loss) || new.as_ref().is_some_and(|(_,loss)|*loss);
    v.editable &= !new.as_ref().is_some_and(|(_,loss)|*loss);
    v.old=old.map(|(text,_)|text);v.new=new.map(|(text,_)|text);
    v
}

/// One hunk of a unified diff: its header line and body lines (each starting with ' ', '+', '-' or '\').
struct Hunk {
    header: String,
    lines: Vec<String>,
}

fn split_diff(text: &str) -> (String, Vec<Hunk>) {
    let mut head = String::new();
    let mut hunks: Vec<Hunk> = Vec::new();
    for line in text.split_inclusive('\n') {
        let l = line.strip_suffix('\n').unwrap_or(line);
        if l.starts_with("@@") {
            hunks.push(Hunk { header: l.to_string(), lines: Vec::new() });
        } else if let Some(h) = hunks.last_mut() {
            h.lines.push(l.to_string());
        } else {
            head.push_str(l);
            head.push('\n');
        }
    }
    (head, hunks)
}

/// Builds a patch with one hunk, optionally keeping only some of its changed lines.
/// `lines` are indexes into the hunk body. For a reverse patch (unstage / discard) the
/// roles of '+' and '-' swap, as in git's own line staging.
pub fn partial_patch(diff: &str, hunk: usize, lines: Option<&[usize]>, reverse: bool) -> Option<String> {
    if !git::diff_within_budget(diff) { return None; }
    if diff.lines().filter(|line| line.starts_with("diff --git ")).count() > 1 { return None; }
    let (head, hunks) = split_diff(diff);
    let h = hunks.get(hunk)?;
    let mut out = head;
    out.push_str(&h.header);
    out.push('\n');
    let mut previous_written = false;
    for (i, l) in h.lines.iter().enumerate() {
        // A no-newline marker belongs to the immediately preceding original line.
        // Dropped additions/deletions must not transfer their marker onto context.
        if l.starts_with('\\') {
            if previous_written { out.push_str(l); out.push('\n'); }
            continue;
        }
        let chosen = lines.map(|ls| ls.contains(&i)).unwrap_or(true);
        let (keep_sign, ctx_sign) = if reverse { ('-', '+') } else { ('+', '-') };
        if chosen || l.starts_with(' ') {
            out.push_str(l);
        } else if l.starts_with(ctx_sign) {
            // Not chosen: the line stays as it is on the side the patch is applied to.
            out.push(' ');
            out.push_str(&l[1..]);
        } else if l.starts_with(keep_sign) {
            previous_written = false;
            continue; // not chosen: leave this addition out
        } else {
            out.push_str(l);
        }
        out.push('\n');
        previous_written = true;
    }
    Some(out)
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PatchOp {
    Stage,
    Unstage,
    Discard,
}

/// Stage, unstage or discard one hunk (or chosen lines of it). Returns the patch that was
/// applied and how, so it can be undone.
pub fn apply_part_checked(dir: &Path, file: &str, untracked: bool, op: PatchOp, hunk: usize, lines: Option<&[usize]>, expected: &[git::DiffLine]) -> Result<(String, bool, bool), String> {
    let src = if op == PatchOp::Unstage { DiffSource::Staged } else { DiffSource::Unstaged };
    if git::file_diff_opts(dir, &src, file, untracked, None, false) != expected {
        return Err("The displayed diff has changed. Refresh it and select the change again.".into());
    }
    apply_part(dir, file, untracked, op, hunk, lines)
}

pub fn apply_part(dir: &Path, file: &str, untracked: bool, op: PatchOp, hunk: usize, lines: Option<&[usize]>) -> Result<(String, bool, bool), String> {
    if untracked && std::fs::read(dir.join(file)).is_ok_and(|bytes| std::str::from_utf8(&bytes).is_err()) {
        return Err("Partial staging requires UTF-8 text. Stage the whole file to preserve its original encoding.".into());
    }
    if untracked {
        // Make the new file visible to `git diff` without staging its contents.
        let o = run(dir, &["--literal-pathspecs", "add", "--intent-to-add", "--", file]);
        if !o.ok {
            return Err(o.message());
        }
    }
    let src = if op == PatchOp::Unstage { DiffSource::Staged } else { DiffSource::Unstaged };
    let diff = git::diff_text_checked(dir, &src, file, None, false)?;
    let reverse = op != PatchOp::Stage;
    let patch = partial_patch(&diff, hunk, lines, reverse).ok_or("That change is no longer there. The file changed; try again.")?;
    let cached = op != PatchOp::Discard;
    apply_patch(dir, &patch, cached, reverse)?;
    Ok((patch, cached, reverse))
}

/// Hunks are checked exactly:
/// match the complete preimage bytes at the hunk position, then splice the postimage
/// before checking the next hunk. Preserve CR and the presence/absence of final LF.
/// Reverse patches exchange old/new ranges and '+'/'-' lines.
pub fn check_hunks_exact(text: &str, patch: &str, reverse: bool) -> Result<(), String> {
    fn range(value: &str) -> Option<(i64, usize)> {
        let (start, count) = value.split_once(',').unwrap_or((value, "1"));
        let start = start.parse::<i64>().ok()?;
        let count = count.parse::<usize>().ok()?;
        (start >= 0).then_some((start, count))
    }
    fn remove_final_lf(lines: &mut [String]) {
        if let Some(last) = lines.last_mut() {
            if last.ends_with('\n') {
                last.pop();
            }
        }
    }

    let mut file: Vec<String> = text.split_inclusive('\n').map(str::to_owned).collect();
    let mut shift: i64 = 0;
    let mut lines = patch.split_inclusive('\n').peekable();
    while let Some(line) = lines.next() {
        let Some(rest) = line.strip_prefix("@@ -") else { continue };
        let (ranges, _) = rest.split_once(" @@").ok_or("Invalid hunk header")?;
        let (old, new) = ranges.split_once(" +").ok_or("Invalid hunk header")?;
        let old = range(old).ok_or("Invalid old hunk range")?;
        let new = range(new).ok_or("Invalid new hunk range")?;
        let (start, count) = if reverse { new } else { old };
        let (keep, gone) = if reverse { (b'+', b'-') } else { (b'-', b'+') };
        let mut pre: Vec<String> = Vec::new();
        let mut post: Vec<String> = Vec::new();
        let mut previous = b' ';
        while let Some(line) = lines.peek() {
            let Some(&sign) = line.as_bytes().first() else { break };
            if !matches!(sign, b' ' | b'+' | b'-' | b'\\') {
                break;
            }
            let line = lines.next().unwrap();
            if sign == b'\\' {
                // "\ No newline at end of file" belongs to the preceding side.
                if previous == b' ' || previous == keep {
                    remove_final_lf(&mut pre);
                }
                if previous == b' ' || previous == gone {
                    remove_final_lf(&mut post);
                }
                continue;
            }
            if sign == b' ' || sign == keep {
                pre.push(line[1..].to_owned());
            }
            if sign == b' ' || sign == gone {
                post.push(line[1..].to_owned());
            }
            previous = sign;
        }
        // A zero-length range inserts AFTER start; otherwise start is one-based.
        let position = if count == 0 { start } else { (start - 1).max(0) } + shift;
        if position < 0 {
            return Err(format!("hunk at line {start} did not apply"));
        }
        let at = (position as usize).min(file.len());
        let end = at.checked_add(pre.len()).ok_or("Hunk range overflow")?;
        if file.get(at..end) != Some(pre.as_slice()) {
            return Err(format!("hunk at line {start} did not apply"));
        }
        let delta = post.len() as i64 - pre.len() as i64;
        file.splice(at..end, post);
        shift += delta;
    }
    Ok(())
}

pub fn apply_patch(dir: &Path, patch: &str, cached: bool, reverse: bool) -> Result<(), String> {
    let mut args = vec!["apply", "--recount", "--whitespace=nowarn"];
    if cached {
        args.push("--cached");
    }
    if reverse {
        args.push("--reverse");
    }
    args.push("-");
    let output = crate::read_jobs::run_mutation(git::git_command(dir, &args), Some(patch.as_bytes().to_vec()));
    if output.ok { Ok(()) } else { Err(output.message()) }
}

fn with_head_index(dir: &Path, action: impl FnOnce(&Path, &str) -> git::GitOut) -> git::GitOut {
    let folder = match tempfile::tempdir() {
        Ok(folder) => folder,
        Err(error) => return git::GitOut::failed(&error.to_string()),
    };
    let index = folder.path().join("index");
    let Some(index) = index.to_str() else { return git::GitOut::failed("The temporary index path is not valid UTF-8."); };
    let result = git::run_env(dir, &["read-tree", "HEAD"], &[("GIT_INDEX_FILE", index)]);
    if result.ok { action(folder.path(), index) } else { result }
}

pub fn stash_untracked(dir: &Path, message: &str) -> git::GitOut {
    let untracked = git::run_limited(dir, &["ls-files", "--others", "--exclude-standard", "-z"], 8 * 1024 * 1024);
    if !untracked.ok { return untracked; }
    if untracked.stdout.is_empty() { return git::GitOut::failed("There are no untracked files to stash."); }
    if untracked.stdout.contains('\u{fffd}') || !untracked.stdout.ends_with('\0') {
        return git::GitOut::failed("Could not read all untracked paths safely.");
    }
    let deleted = git::run_limited(dir, &["diff", "--cached", "--no-renames", "--name-only", "--diff-filter=D", "-z"], 8 * 1024 * 1024);
    if !deleted.ok { return deleted; }
    if !deleted.stdout.is_empty() {
        let paths = untracked.stdout.split_terminator('\0').collect::<std::collections::HashSet<_>>();
        let ancestors = paths.iter().flat_map(|path| path.match_indices('/').map(|(i, _)| &path[..i])).collect::<std::collections::HashSet<_>>();
        for path in deleted.stdout.split_terminator('\0') {
            if paths.contains(path) || ancestors.contains(path) || path.match_indices('/').any(|(i, _)| paths.contains(&path[..i])) {
                return git::GitOut::failed("An untracked file overlaps a staged deletion. Commit or unstage that deletion first.");
            }
        }
    }
    with_head_index(dir, |folder, index| {
        let spec = folder.join("paths");
        if let Err(error) = std::fs::write(&spec, untracked.stdout.as_bytes()) { return git::GitOut::failed(&error.to_string()); }
        let pathspec = format!("--pathspec-from-file={}", spec.display());
        git::run_env(dir, &["--literal-pathspecs", "stash", "push", "--include-untracked", "-m", message, &pathspec, "--pathspec-file-nul"], &[("GIT_INDEX_FILE", index)])
    })
}

pub fn stash_hunk(dir: &Path, file: &str, hunk: usize, lines: Option<&[usize]>, expected: &[git::DiffLine]) -> git::GitOut {
    if git::file_diff_opts(dir, &DiffSource::Unstaged, file, false, None, false) != expected {
        return git::GitOut::failed("The displayed diff changed. Refresh it and select the change again.");
    }
    let diff = match git::diff_text_checked(dir, &DiffSource::Unstaged, file, None, false) {
        Ok(diff) => diff,
        Err(error) => return git::GitOut::failed(&error),
    };
    let (Some(forward), Some(reverse)) = (partial_patch(&diff, hunk, lines, false), partial_patch(&diff, hunk, lines, true)) else {
        return git::GitOut::failed("The selected hunk is unavailable.");
    };
    let run_input = |args: &[&str], index: Option<&str>, patch: &str| {
        let mut command = git::git_command(dir, args);
        if let Some(index) = index { command.env("GIT_INDEX_FILE", index); }
        crate::read_jobs::run_mutation(command, Some(patch.as_bytes().to_vec()))
    };
    let checked = run_input(&["apply", "--check", "--recount", "--reverse", "-"], None, &reverse);
    if !checked.ok { return checked; }
    let head = git::run(dir, &["rev-parse", "--verify", "HEAD"]);
    if !head.ok { return head; }
    let head = head.stdout.trim();
    let snapshot = with_head_index(dir, |_, index| {
        let base = git::run_env(dir, &["write-tree"], &[("GIT_INDEX_FILE", index)]);
        if !base.ok { return base; }
        let patched = run_input(&["apply", "--cached", "--recount", "--whitespace=nowarn", "-"], Some(index), &forward);
        if !patched.ok {
            return git::GitOut::failed("The selected change overlaps staged changes. Stash the file or unstage the overlapping changes first.");
        }
        let tree = git::run_env(dir, &["write-tree"], &[("GIT_INDEX_FILE", index)]);
        if !tree.ok { return tree; }
        if tree.stdout == base.stdout { return git::GitOut::failed("No changed lines were selected."); }
        let index_commit = git::run(dir, &["commit-tree", base.stdout.trim(), "-p", head, "-m", "index for selected changes"]);
        if !index_commit.ok { return index_commit; }
        git::run(dir, &["commit-tree", tree.stdout.trim(), "-p", head, "-p", index_commit.stdout.trim(), "-m", "Selected changes"])
    });
    if !snapshot.ok { return snapshot; }
    if git::file_diff_opts(dir, &DiffSource::Unstaged, file, false, None, false) != expected {
        return git::GitOut::failed("The displayed diff changed. Refresh it and select the change again.");
    }
    let message = format!("Selected changes in {file}");
    let saved = git::run(dir, &["stash", "store", "-m", &message, snapshot.stdout.trim()]);
    if !saved.ok { return saved; }
    match apply_patch(dir, &reverse, false, true) {
        Ok(()) => saved,
        Err(error) => git::GitOut::failed(&format!("The selection was saved to the stash. Git could not remove it from the working folder: {error}")),
    }
}

/// Optimistic concurrency token for the exact bytes displayed by an editor.
pub fn content_token(bytes: Option<&[u8]>) -> String {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hash);
    format!("{:016x}", hash.finish())
}

pub fn save_checked_mode(dir: &Path, file: &str, content: &str, expected: Option<&str>, utf8: bool) -> Result<(), String> {
    save_checked_token(dir, file, content, expected, utf8).map(|_| ())
}

/// [`save_checked_mode`], returning the content token of the exact bytes it wrote, so a later save can expect them
/// without reading the file again (which could pick up an edit made meanwhile by another program).
pub fn save_checked_token(dir: &Path, file: &str, content: &str, expected: Option<&str>, utf8: bool) -> Result<String, String> {
    let root = dir.canonicalize().map_err(|error| error.to_string())?;
    let path = repo_file(&root, file)?;
    let destination = match std::fs::symlink_metadata(&path) {
        Ok(_) => path.canonicalize(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path.parent().ok_or("File has no parent directory")?;
            let name = path.file_name().ok_or("File has no name")?;
            parent.canonicalize().map(|parent| parent.join(name))
        }
        Err(error) => Err(error),
    }.map_err(|error| error.to_string())?;
    if !destination.starts_with(&root) || destination == root {
        return Err("The file resolves outside this repository and cannot be saved here.".into());
    }
    // A tracked symlink can also point into Git's own metadata (config, hooks). Never write there.
    let git_dir = git::run(&root, &["rev-parse", "--absolute-git-dir"]);
    let metadata = if git_dir.ok { Path::new(git_dir.stdout.trim()).canonicalize().ok() } else { None };
    let inside_metadata = metadata.as_ref().is_some_and(|metadata| destination.starts_with(metadata))
        || destination.strip_prefix(&root).is_ok_and(|relative| relative.components().any(|part| part.as_os_str().eq_ignore_ascii_case(".git")));
    if inside_metadata {
        return Err("The file resolves into the repository's Git metadata and cannot be saved here.".into());
    }
    if let Some(expected) = expected {
        let current = match std::fs::read(&destination) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.to_string()),
        };
        if content_token(current.as_deref()) != expected {
            return Err("The file changed outside this editor. Your draft was kept; reopen the file and compare before saving.".into());
        }
    }
    let destination_file = destination.strip_prefix(&root).map_err(|error| error.to_string())?.to_string_lossy();
    let destination_file = if cfg!(windows) { destination_file.replace('\\', "/") } else { destination_file.into_owned() };
    let checked_file = if crate::manage::lfs_lockable(dir, &destination_file)? { Some(destination_file.as_str()) }
        else if destination_file != file && crate::manage::lfs_lockable(dir, file)? { Some(file) } else { None };
    if let Some(file) = checked_file {
        let locks = crate::manage::lfs_locks(dir).map_err(|error| format!("Could not verify the Git LFS lock for {file}: {error}"))?;
        if !locks.iter().any(|lock| lock.ours && lock.path == file) {
            return Err(format!("Lock {file} with Git LFS before editing it."));
        }
    }
    save_mode(dir, &destination, content, utf8)
}

fn save_mode(dir: &Path, p: &Path, content: &str, utf8: bool) -> Result<String, String> {
    // Keep the file's existing line endings.
    let before = match std::fs::read(p) {
        Ok(bytes)=>bytes,Err(error) if error.kind()==std::io::ErrorKind::NotFound=>Vec::new(),Err(error)=>return Err(error.to_string()),
    };
    let enc = encoding_rs::Encoding::for_bom(&before).map(|(encoding, _)| encoding).or_else(|| crate::prefs::encoding_for(dir));
    let (old_text,decoding_loss)=crate::prefs::decode_preview(&before,enc);
    if decoding_loss && !utf8 {return Err("ENCODING_DECODE_LOSS: Existing bytes cannot be decoded with this encoding. Choose the correct repository encoding or explicitly convert the reviewed text to UTF-8; the original file was kept.".into());}
    let crlf = old_text.contains("\r\n");
    let text = if crlf { content.replace("\r\n", "\n").replace('\n', "\r\n") } else { content.to_string() };
    let bytes = if utf8 || before.starts_with(&[0xEF, 0xBB, 0xBF]) {
        [vec![0xEF, 0xBB, 0xBF], text.as_bytes().to_vec()].concat()
    } else { match enc {
        Some(e) => crate::prefs::encode(&text, e, before.starts_with(&[0xFF, 0xFE]) || before.starts_with(&[0xFE, 0xFF]))?,
        None => text.into_bytes(),
    } };
    replace_file_at(p, |file| std::io::Write::write_all(file, &bytes)).map_err(|e| e.to_string())?;
    Ok(content_token(Some(&bytes)))
}

/// Follow an existing symlink while keeping the link itself intact. A same-directory
/// rename publishes only a complete file; a failed write/replace leaves the old file.
#[cfg(test)]
fn replace_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    replace_file_with(path, |file| file.write_all(bytes))
}
#[cfg(test)]
fn replace_file_with(path: &Path, write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>) -> std::io::Result<()> {
    let destination = match std::fs::symlink_metadata(path) { Ok(_) => std::fs::canonicalize(path)?, Err(error) if error.kind() == std::io::ErrorKind::NotFound => path.to_path_buf(), Err(error) => return Err(error) };
    replace_file_at(&destination, write)
}

/// Write `bytes` to `path` through a new temporary file beside it (created exclusively, under a
/// random name), so an existing file is replaced only once the new one is completely written.
pub fn write_replacing(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    replace_file_at(path, |file| std::io::Write::write_all(file, bytes))
}

fn replace_file_at(destination: &Path, write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>) -> std::io::Result<()> {
    let parent = destination.parent().ok_or_else(|| std::io::Error::other("File has no parent directory"))?;
    let mut pending = tempfile::NamedTempFile::new_in(parent)?;
    write(pending.as_file_mut())?;
    if let Ok(metadata) = std::fs::metadata(destination) {
        pending.as_file().set_permissions(metadata.permissions())?;
    }
    pending.as_file().sync_all()?;
    let pending = pending.into_temp_path();
    #[cfg(windows)]
    if destination.exists() {
        use std::os::windows::ffi::OsStrExt;
        let target: Vec<u16> = destination.as_os_str().encode_wide().chain(Some(0)).collect();
        let source: Vec<u16> = pending.as_os_str().encode_wide().chain(Some(0)).collect();
        // ReplaceFile preserves the destination ACL and attributes on Windows.
        // SAFETY: both buffers are NUL-terminated UTF-16 paths that outlive the call; the optional
        // backup name, exclusion and reserved pointers are null as the API permits.
        let replaced = unsafe { windows_sys::Win32::Storage::FileSystem::ReplaceFileW(target.as_ptr(), source.as_ptr(), std::ptr::null(), 0, std::ptr::null(), std::ptr::null()) };
        if replaced == 0 { return Err(std::io::Error::last_os_error()); }
        return Ok(());
    }
    pending.persist(destination).map_err(|e| e.error)?;
    Ok(())
}

#[derive(Serialize)]
pub struct HistoryEntry {
    #[serde(flatten)]
    pub commit: Commit,
    /// The file's path in that commit (it may have been renamed since).
    pub path: String,
    pub status: char,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct FileHistoryCursor {
    pub tip: String,
    pub offset: usize,
}

#[derive(Serialize)]
pub struct FileHistoryPage {
    pub entries: Vec<HistoryEntry>,
    pub cursor: Option<FileHistoryCursor>,
}

pub fn file_history_page(dir: &Path, file: &str, cursor: Option<FileHistoryCursor>, page_size: usize) -> Result<FileHistoryPage, String> {
    let mut cursor = match cursor {
        Some(cursor) if matches!(cursor.tip.len(), 40 | 64) && cursor.tip.bytes().all(|byte| byte.is_ascii_hexdigit()) => cursor,
        Some(_) => return Err("Reload the file history to continue.".into()),
        None => {
            let head = git::run(dir, &["rev-parse", "--verify", "HEAD"]);
            if !head.ok { return Err(head.message()); }
            FileHistoryCursor { tip: head.stdout.trim().to_owned(), offset: 0 }
        }
    };
    // Git can lose the path of an older rename when --skip passes the rename itself.
    // Walk the fixed starting commit with --follow and advance the offset after parsing.
    let max = cursor.offset.checked_add(page_size + 1).ok_or("This history position is invalid.")?;
    let page = read_file_history(dir, file, &cursor.tip, max)?;
    let more = page.len() > cursor.offset + page_size;
    let entries: Vec<_> = page.into_iter().skip(cursor.offset).take(page_size).collect();
    cursor.offset = cursor.offset.saturating_add(entries.len());
    Ok(FileHistoryPage { entries, cursor: more.then_some(cursor) })
}

#[cfg(test)]
pub fn history(dir: &Path, file: &str, max: usize) -> Result<Vec<HistoryEntry>, String> {
    read_file_history(dir, file, "HEAD", max)
}

fn read_file_history(dir: &Path, file: &str, tip: &str, max: usize) -> Result<Vec<HistoryEntry>, String> {
    let count = format!("-n{max}");
    let output = run(dir, &["--literal-pathspecs", "log", "--follow", "-M", &count, "--name-status", "-z",
        "--format=%H%x00%P%x00%aN%x00%aE%x00%at%x00%s%x00%b%x00", tip, "--", file]);
    if !output.ok { return Err(output.message()); }
    let fields: Vec<_> = output.stdout.split('\0').collect();
    let mut position = 0; let mut entries = Vec::new();
    while position < fields.len() {
        while position < fields.len() && fields[position].trim_matches(['\r', '\n']).is_empty() { position += 1; }
        if position + 7 > fields.len() { break; }
        let Some(commit) = git::parse_commit_fields(&fields[position..position + 7]) else { break; };
        position += 7;
        let mut path = file.to_owned(); let mut status = 'M'; let mut found = false;
        while position < fields.len() {
            let token = fields[position].trim_start_matches(['\r', '\n']);
            if token.is_empty() { position += 1; continue; }
            let kind = token.chars().next().unwrap();
            if !matches!(kind, 'A' | 'M' | 'D' | 'R' | 'C' | 'T' | 'U' | 'X' | 'B') || !token[1..].bytes().all(|byte| byte.is_ascii_digit()) { break; }
            position += 1;
            let names = if matches!(kind, 'R' | 'C') { 2 } else { 1 };
            if position + names > fields.len() { break; }
            if !found { path = fields[position + names - 1].into(); status = kind; found = true; }
            position += names;
        }
        entries.push(HistoryEntry { commit, path, status });
    }
    Ok(entries)
}

#[derive(Serialize, Clone, Debug)]
pub struct BlameLine {
    pub sha: String,
    pub author: String,
    pub time: i64,
    pub summary: String,
    pub line: usize,
    pub text: String,
}

/// Who last changed each line (`git blame --porcelain`). `rev` None = working folder.
pub fn blame(dir: &Path, file: &str, rev: Option<&str>, ignore_revs: bool) -> Result<Vec<BlameLine>, String> {
    // `git blame` does not accept --end-of-options, so a revision must never look like an option.
    if rev.is_some_and(|rev| rev.is_empty() || rev.starts_with('-')) { return Err("Choose a commit to blame.".into()); }
    let bytes = preview_contents(dir, rev.unwrap_or(WORKDIR), file, PREVIEW_MAX)
        .map_err(|_| "This file exceeds the 2 MB blame limit. Open it in an external editor.".to_string())?
        .ok_or("Could not read this file for blame")?;
    let line_count = bytes.iter().filter(|&&b| b == b'\n').count() + usize::from(!bytes.is_empty() && !bytes.ends_with(b"\n"));
    if line_count > 100_000 { return Err("This file exceeds the 100,000 line blame limit.".into()); }
    let repository_list = ignore_revs && std::fs::symlink_metadata(dir.join(".git-blame-ignore-revs")).is_ok_and(|file| file.is_file());
    let run_blame = |ignore: Option<&str>| {
        let mut args = vec!["--literal-pathspecs", "blame", "--porcelain"];
        if let Some(list) = ignore { args.extend(["--ignore-revs-file", list]); }
        if let Some(r) = rev { args.push(r); }
        args.extend(["--", file]);
        git::run_limited(dir, &args, 16 * 1024 * 1024)
    };
    let mut o = run_blame(if repository_list { Some(".git-blame-ignore-revs") } else if ignore_revs { None } else { Some("") });
    if !o.ok && ignore_revs {
        // A malformed or abbreviated entry in an ignore-revs list makes Git refuse the whole blame.
        // Fall back to an ordinary blame so one bad line in the repository cannot disable the view.
        let plain = run_blame(Some(""));
        if plain.ok { o = plain; }
    }
    if !o.ok {
        return Err(o.message());
    }
    let mut info: std::collections::HashMap<String, (String, i64, String)> = Default::default();
    let mut out = Vec::new();
    let mut cur_sha = String::new();
    let mut cur_line = 0usize;
    let mut author = String::new();
    let mut time = 0i64;
    let mut summary = String::new();
    for l in o.stdout.lines() {
        if let Some(t) = l.strip_prefix('\t') {
            let (a, tm, s) = info
                .entry(cur_sha.clone())
                .or_insert_with(|| (author.clone(), time, summary.clone()))
                .clone();
            out.push(BlameLine { sha: cur_sha.clone(), author: a, time: tm, summary: s, line: cur_line, text: t.to_string() });
        } else if let Some(v) = l.strip_prefix("author ") {
            author = v.to_string();
        } else if let Some(v) = l.strip_prefix("author-time ") {
            time = v.parse().unwrap_or(0);
        } else if let Some(v) = l.strip_prefix("summary ") {
            summary = v.to_string();
        } else {
            let mut it = l.split_whitespace();
            if let (Some(sha), Some(_orig), Some(fin)) = (it.next(), it.next(), it.next()) {
                if matches!(sha.len(), 40 | 64) && sha.chars().all(|c| c.is_ascii_hexdigit()) {
                    cur_sha = sha.to_string();
                    cur_line = fin.parse().unwrap_or(0);
                    if let Some((a, tm, s)) = info.get(&cur_sha) {
                        author = a.clone();
                        time = *tm;
                        summary = s.clone();
                    }
                }
            }
        }
    }
    Ok(out)
}

#[derive(Serialize)]
pub struct Stats {
    pub sha: String,
    pub files: u32,
    pub added: u32,
    pub deleted: u32,
}

/// Files changed, lines added and removed for each commit (the graph's CHANGES column).
pub fn stats(dir: &Path, shas: &[String]) -> Result<Vec<Stats>, String> {
    if shas.is_empty() {
        return Ok(Vec::new());
    }
    let mut args: Vec<&str> = vec!["log", "--no-walk=unsorted", "--diff-merges=first-parent", "--shortstat", "--format=%x1e%H"];
    args.extend(shas.iter().map(|s| s.as_str()));
    let o = run(dir, &args);
    if !o.ok { return Err(o.message()); }
    let mut out = Vec::new();
    for rec in o.stdout.split('\x1e').filter(|r| !r.trim().is_empty()) {
        let mut lines = rec.lines().filter(|l| !l.trim().is_empty());
        let sha = lines.next().unwrap_or("").trim().to_string();
        let mut s = Stats { sha, files: 0, added: 0, deleted: 0 };
        if let Some(stat) = lines.next() {
            for part in stat.split(',') {
                let n: u32 = part.split_whitespace().next().and_then(|x| x.parse().ok()).unwrap_or(0);
                if part.contains("file") {
                    s.files = n;
                } else if part.contains("insertion") {
                    s.added = n;
                } else if part.contains("deletion") {
                    s.deleted = n;
                }
            }
        }
        out.push(s);
    }
    Ok(out)
}

/// Files that differ between two points (`to` may be WORKDIR).
pub fn range_files(dir: &Path, from: &str, to: &str) -> Result<Vec<FileChange>, String> {
    let mut args = vec!["diff", "--name-status", "-z", "-M", from];
    if to != WORKDIR {
        args.push(to);
    }
    let o = run(dir, &args);
    if !o.ok { return Err(o.message()); }
    let mut out = Vec::new();
    let mut it = o.stdout.split('\0').filter(|s| !s.is_empty());
    while let Some(st) = it.next() {
        let ch = st.chars().next().unwrap_or('M');
        if ch == 'R' || ch == 'C' {
            let _old = it.next();
            if let Some(p) = it.next() {
                out.push(FileChange { path: p.to_string(), status: 'R' });
            }
        } else if let Some(p) = it.next() {
            out.push(FileChange { path: p.to_string(), status: ch });
        }
    }
    Ok(out)
}

/// Every tracked file (for the file history finder).
pub fn tracked_files(dir: &Path) -> Result<Vec<String>, String> {
    let output = run(dir, &["ls-files", "-z"]);
    if !output.ok { return Err(output.message()); }
    Ok(output.stdout.split('\0').filter(|s| !s.is_empty()).map(str::to_owned).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,3 +1,4 @@\n a\n-b\n+B\n+new\n c\n@@ -9,1 +10,1 @@\n-x\n+y\n";

    #[test]
    fn binary_test_matches_libgit2() {
        assert!(!is_binary(b"hello
world
	ok"));
        assert!(is_binary(b"abc def"));
        assert!(is_binary(&[0xFF, 0xFE, b'a', 0]));
        assert!(!is_binary(&[0xEF, 0xBB, 0xBF, b'a', b'b']));
        // 128 printable characters allow one control character (128 >> 7 = 1, and 1 < 1 is false)...
        let mut t = vec![b'a'; 128];
        t.push(1);
        assert!(!is_binary(&t));
        // ...but not two.
        t.push(2);
        assert!(is_binary(&t));
        // ESC, BS and FF count as printable; TAB, LF, VT and CR are neutral.
        assert!(!is_binary(&[27, 8, 12]));
        assert!(!is_binary(&[9, 10, 11, 13]));
        assert!(is_binary(&[14]));
        // Only the first 8000 bytes are looked at.
        let mut big = vec![b'a'; 8000];
        big.push(0);
        assert!(!is_binary(&big));
    }

    #[test]
    fn hunks_apply_only_at_their_own_line() {
        let patch = "--- a/f\n+++ b/f\n@@ -2,2 +2,2 @@\n b\n-c\n+C\n";
        // Forward: lines 2-3 must be "b", "c".
        assert!(check_hunks_exact("a\nb\nc\nd\n", patch, false).is_ok());
        // The same text one line lower would be found by git apply, but not by libgit2.
        assert_eq!(check_hunks_exact("x\na\nb\nc\nd\n", patch, false).unwrap_err(), "hunk at line 2 did not apply");
        // Reverse (revert): match the new side, including its actual line endings.
        assert!(check_hunks_exact("a\nb\nC\nd\n", patch, true).is_ok());
        assert!(check_hunks_exact("a\r\nb\r\nC\r\nd\r\n", patch, true).is_err());
        let crlf_patch = "--- a/f\n+++ b/f\n@@ -2,2 +2,2 @@\n b\r\n-c\r\n+C\r\n";
        assert!(check_hunks_exact("a\r\nb\r\nC\r\nd\r\n", crlf_patch, true).is_ok());
    }

    #[test]
    fn exact_hunks_follow_insertions_and_deletions() {
        let inserted = "--- a/f\n+++ b/f\n@@ -1,1 +1,2 @@\n a\n+inserted\n@@ -3,1 +4,1 @@\n-c\n+C\n";
        assert!(check_hunks_exact("a\nb\nc\n", inserted, false).is_ok());
        assert!(check_hunks_exact("a\ninserted\nb\nC\n", inserted, true).is_ok());
        let deleted = "--- a/f\n+++ b/f\n@@ -1,2 +1,1 @@\n a\n-b\n@@ -4,1 +3,1 @@\n-d\n+D\n";
        assert!(check_hunks_exact("a\nb\nc\nd\n", deleted, false).is_ok());
        assert!(check_hunks_exact("a\nc\nD\n", deleted, true).is_ok());
        let insert_after_first = "--- a/f\n+++ b/f\n@@ -1,0 +2,1 @@\n+inserted\n@@ -2,1 +3,1 @@\n-b\n+B\n";
        assert!(check_hunks_exact("a\nb\n", insert_after_first, false).is_ok());
    }

    #[test]
    fn exact_hunks_preserve_final_newline_and_patch_bytes() {
        let patch = "--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n";
        assert!(check_hunks_exact("old", patch, false).is_ok());
        assert!(check_hunks_exact("old\n", patch, false).is_err());
        assert!(check_hunks_exact("new\n", patch, true).is_ok());
        assert!(check_hunks_exact("new", patch, true).is_err());
        let crlf = "--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old\r\n+new\r\n";
        let partial = partial_patch(crlf, 0, None, false).unwrap();
        assert!(partial.contains("-old\r\n+new\r\n"));
        assert!(check_hunks_exact("old\r\n", &partial, false).is_ok());
        assert!(check_hunks_exact("", "--- a/f\n+++ b/f\n@@ -1 +1 @@\n-\n+x\n", false).is_err());
    }

    #[test]
    fn whole_hunk_patch() {
        let p = partial_patch(DIFF, 1, None, false).unwrap();
        assert!(p.starts_with("diff --git"));
        assert!(p.contains("@@ -9,1 +10,1 @@\n-x\n+y\n"));
        assert!(!p.contains("+new"));
    }

    #[test]
    fn line_patch_forward_and_reverse() {
        // body indexes: 0 " a", 1 "-b", 2 "+B", 3 "+new", 4 " c"; choose only "+new"
        let p = partial_patch(DIFF, 0, Some(&[3]), false).unwrap();
        assert!(p.contains(" a\n b\n+new\n c\n"), "{p}");
        let r = partial_patch(DIFF, 0, Some(&[3]), true).unwrap();
        // reverse: unchosen "+B" is already there (context), unchosen "-b" is gone (dropped)
        assert!(r.contains(" a\n B\n+new\n c\n"), "{r}");
        assert!(!r.contains("-b"));
    }
}

/// A file inside the repository's working folder. Refuses absolute paths and `..`, so the file actions below never
/// reach outside the repository.
pub fn repo_file(dir: &Path, file: &str) -> Result<std::path::PathBuf, String> {
    let rel = Path::new(file);
    if file.is_empty() || rel.is_absolute() || rel.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
        return Err(format!("\"{file}\" is not a file in this repository"));
    }
    // Push component by component: `dir.join("src/main.rs")` keeps the forward slash, and the Windows shell
    // APIs behind "Open in File Manager" (SHParseDisplayName) reject mixed separators.
    let mut full = dir.to_path_buf();
    for c in rel.components() {
        full.push(c);
    }
    Ok(full)
}

/// Largest file "Copy file contents" puts on the clipboard.
pub const COPY_CONTENTS_MAX: usize = 10 * 1024 * 1024;

/// The text of a file, from the working folder or (with `rev`) as it was in a commit, for "Copy file contents".
/// Binary files are refused by the same rule as the diff view (is_binary).
pub fn file_text(dir: &Path, file: &str, rev: Option<&str>) -> Result<String, String> {
    repo_file(dir, file)?;
    let source = rev.filter(|rev| !rev.is_empty()).unwrap_or(WORKDIR);
    let bytes = preview_contents(dir, source, file, COPY_CONTENTS_MAX)
        .map_err(|_| "This file is too large to copy.".to_string())?
        .ok_or_else(|| "Could not read this file. It may have been moved or removed.".to_string())?;
    if is_binary(&bytes) {
        return Err("This is a binary file, so it can't be copied as text.".into());
    }
    Ok(match crate::prefs::encoding_for(dir) {
        Some(e) => crate::prefs::decode(&bytes, e),
        None => String::from_utf8_lossy(&bytes).into_owned(),
    })
}

#[cfg(test)]
mod file_action_tests {
    use super::*;

    #[test]
    fn clipboard_contents_distinguish_index_worktree_and_saved_commit() {
        let dir = std::env::temp_dir().join(format!("gitcraft-copy-index-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for args in [&["init", "-q"][..], &["config", "user.name", "Test"], &["config", "user.email", "t@example.com"], &["config", "commit.gpgsign", "false"]] {
            assert!(git::run(&dir, args).ok);
        }
        std::fs::write(dir.join("a.txt"), "saved\n").unwrap();
        assert!(git::run(&dir, &["add", "."]).ok);
        assert!(git::run(&dir, &["commit", "-qm", "saved"]).ok);
        std::fs::write(dir.join("a.txt"), "staged\n").unwrap();
        assert!(git::run(&dir, &["add", "."]).ok);
        std::fs::write(dir.join("a.txt"), "working\n").unwrap();
        assert_eq!(file_text(&dir, "a.txt", Some(":")).unwrap().replace("\r\n", "\n"), "staged\n");
        assert_eq!(file_text(&dir, "a.txt", None).unwrap().replace("\r\n", "\n"), "working\n");
        assert_eq!(file_text(&dir, "a.txt", Some("HEAD")).unwrap().replace("\r\n", "\n"), "saved\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn repo_file_stays_inside_the_repository() {
        let dir = Path::new(if cfg!(windows) { "C:/repo" } else { "/repo" });
        assert!(repo_file(dir, "src/main.rs").is_ok());
        assert!(repo_file(dir, "../secret.txt").is_err());
        assert!(repo_file(dir, "src/../../x").is_err());
        assert!(repo_file(dir, "").is_err());
        if cfg!(windows) { assert!(repo_file(dir, "C:/Windows/win.ini").is_err()); }
        assert!(repo_file(dir, "/etc/passwd").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn repo_file_uses_only_backslashes_on_windows() {
        let p = repo_file(Path::new(r"C:\repo"), "src/ui/main.rs").unwrap();
        assert_eq!(p.to_string_lossy(), r"C:\repo\src\ui\main.rs");
    }
}

#[cfg(test)]
mod preview_tests {
    use super::*;
    #[test]
    fn oversized_working_files_require_an_explicit_preview_and_keep_small_sides() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.txt");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(PREVIEW_MAX as u64 + 1).unwrap();
        let src = DiffSource::Unstaged;
        let preview = versions_with_limit(dir.path(), &src, "large.txt", None, true, false);
        assert!(preview.too_large);
        assert!(preview.new.is_none());
        assert!(!preview.editable);
        assert!(preview_contents(dir.path(), WORKDIR, "large.txt", LARGE_PREVIEW_MAX).unwrap().is_some());
        file.set_len(LARGE_PREVIEW_MAX as u64 + 1).unwrap();
        assert!(versions_with_limit(dir.path(), &src, "large.txt", None, true, true).too_large);
        std::fs::write(&path, "small file").unwrap();
        let small = versions_with_limit(dir.path(), &src, "large.txt", None, true, false);
        assert!(!small.too_large);
        assert_eq!(small.new.as_deref(), Some("small file"));
    }

    #[test]
    fn oversized_git_blobs_are_rejected_before_previewing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(run(dir.path(), &["init"]).ok);
        std::fs::write(dir.path().join("large.txt"), vec![b'x'; PREVIEW_MAX + 1]).unwrap();
        assert!(run(dir.path(), &["add", "large.txt"]).ok);
        assert!(preview_contents(dir.path(), ":", "large.txt", PREVIEW_MAX).is_err());
        assert_eq!(preview_contents(dir.path(), ":", "large.txt", LARGE_PREVIEW_MAX).unwrap().unwrap().len(), PREVIEW_MAX + 1);
    }
}

#[cfg(test)]
mod resource_regressions {
    use super::*;
    #[test]
    fn collection_errors_are_not_empty_results() {
        let temp = tempfile::tempdir().unwrap();
        assert!(history(temp.path(), "file", 10).is_err());
        assert!(tracked_files(temp.path()).is_err());
        assert!(range_files(temp.path(), "HEAD", WORKDIR).is_err());
        assert!(run(temp.path(), &["init"]).ok);
        assert!(run(temp.path(), &["-c", "user.name=Test", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false", "commit", "--allow-empty", "-m", "root"]).ok);
        assert!(history(temp.path(), "absent", 10).unwrap().is_empty());
        assert!(history(temp.path(), "absent", 10).is_ok());
    }
    #[test]
    fn failed_staged_write_and_replace_preserve_original() {
        use std::io::Write;
        let temp = tempfile::tempdir().unwrap(); let file = temp.path().join("file.txt");
        std::fs::write(&file, "original").unwrap();
        assert!(replace_file_with(&file, |pending| { pending.write_all(b"partial")?; Err(std::io::Error::other("injected disk full")) }).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"original");
        // Replacing a directory fails at publication after the temporary file is complete.
        let directory = temp.path().join("directory"); std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("original"), "kept").unwrap();
        assert!(replace_file(&directory, b"replacement").is_err());
        assert_eq!(std::fs::read_to_string(directory.join("original")).unwrap(), "kept");
        replace_file(&file, b"complete").unwrap(); assert_eq!(std::fs::read(&file).unwrap(), b"complete");
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 2);
    }
    #[cfg(unix)]
    #[test]
    fn replacement_preserves_symlink_and_executable_permissions() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let temp = tempfile::tempdir().unwrap(); let target = temp.path().join("target"); let link = temp.path().join("link");
        std::fs::write(&target, "old").unwrap(); std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o751)).unwrap(); symlink(&target, &link).unwrap();
        replace_file(&link, b"new").unwrap(); assert!(link.is_symlink()); assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert_eq!(std::fs::metadata(target).unwrap().permissions().mode() & 0o777, 0o751);
    }
    #[cfg(unix)]
    #[test]
    fn editor_save_rejects_an_external_target_and_parent() {
        use std::os::unix::fs::symlink;
        let repository = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let destination = external.path().join("notes.txt");
        std::fs::write(&destination, "untouched").unwrap();
        symlink(&destination, repository.path().join("notes.txt")).unwrap();
        symlink(external.path(), repository.path().join("linked-dir")).unwrap();
        for file in ["notes.txt", "linked-dir/missing.txt"] {
            assert!(save_checked_mode(repository.path(), file, "edited", None, false).unwrap_err().contains("outside this repository"));
        }
        assert_eq!(std::fs::read_to_string(destination).unwrap(), "untouched");
        assert!(!external.path().join("missing.txt").exists());
        assert!(std::process::Command::new("git").args(["init", "-q"]).current_dir(repository.path()).status().unwrap().success());
        let config = repository.path().join(".git/config");
        let original = std::fs::read(&config).unwrap();
        symlink(&config, repository.path().join("settings-link")).unwrap();
        assert!(save_checked_mode(repository.path(), "settings-link", "edited", None, false).unwrap_err().contains("Git metadata"));
        assert_eq!(std::fs::read(&config).unwrap(), original);
        let inside = repository.path().join("inside.txt");
        std::fs::write(&inside, "before").unwrap();
        symlink(&inside, repository.path().join("inside-link")).unwrap();
        save_checked_mode(repository.path(), "inside-link", "after", None, false).unwrap();
        assert_eq!(std::fs::read_to_string(inside).unwrap(), "after");
        assert!(repository.path().join("inside-link").is_symlink());
    }
    #[test]
    fn blame_rejects_oversized_input_before_git_output() {
        let temp = tempfile::tempdir().unwrap(); let file = temp.path().join("huge.txt");
        std::fs::File::create(&file).unwrap().set_len(PREVIEW_MAX as u64 + 1).unwrap();
        assert!(blame(temp.path(), "huge.txt", None, true).unwrap_err().contains("2 MB"));
        std::fs::write(&file, "x\n".repeat(100_000)).unwrap();
        // Exactly 100,000 newline-terminated lines pass preflight and reach Git.
        assert!(!blame(temp.path(), "huge.txt", None, true).unwrap_err().contains("100,000"));
        std::fs::write(file, "x\n".repeat(100_001)).unwrap();
        assert!(blame(temp.path(), "huge.txt", None, true).unwrap_err().contains("100,000"));
    }
    #[test]
    fn a_malformed_ignore_revs_list_falls_back_to_ordinary_blame() {
        let temp = tempfile::tempdir().unwrap(); let dir = temp.path();
        let git = |args: &[&str]| assert!(std::process::Command::new("git").args(args).current_dir(dir).status().unwrap().success());
        git(&["init", "-q"]); git(&["config", "user.email", "a@b.c"]); git(&["config", "user.name", "A"]);
        std::fs::write(dir.join("f.txt"), "one\ntwo\n").unwrap();
        git(&["add", "f.txt"]); git(&["commit", "-qm", "first"]);
        std::fs::write(dir.join(".git-blame-ignore-revs"), "deadbeef\n").unwrap();
        let lines = blame(dir, "f.txt", None, true).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(blame(dir, "f.txt", Some("HEAD"), true).unwrap().len(), 2, "historical blame still accepts a revision");
        assert!(blame(dir, "f.txt", Some("--contents=f.txt"), false).unwrap_err().contains("Choose a commit"), "revisions are never read as options");
    }
}
