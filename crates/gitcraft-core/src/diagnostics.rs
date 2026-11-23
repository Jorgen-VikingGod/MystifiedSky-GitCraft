use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    sync::{atomic::{AtomicBool, Ordering}, Mutex},
    time::Duration,
};

const FILE: &str = "diagnostics.log";
const PREVIOUS: &str = "diagnostics.previous.log";
const LIMIT: u64 = 128 * 1024;
static ENABLED: AtomicBool = AtomicBool::new(false);
static LOCK: Mutex<()> = Mutex::new(());

pub fn install() {
    ENABLED.store(true, Ordering::Relaxed);
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(location) = info.location() {
            let file = Path::new(location.file()).file_name().and_then(|part| part.to_str()).unwrap_or("unknown");
            write(&format!("Panic in {file}:{}", location.line()));
        } else {
            write("Panic with no source location");
        }
        default(info);
    }));
}

fn write(event: &str) {
    if !ENABLED.load(Ordering::Relaxed) { return; }
    let Ok(_guard) = LOCK.try_lock() else { return; };
    let dir = crate::sample::data_dir();
    if fs::create_dir_all(&dir).is_err() { return; }
    let path = dir.join(FILE);
    if path.symlink_metadata().is_ok_and(|metadata| !metadata.is_file()) { return; }
    if path.metadata().is_ok_and(|metadata| metadata.len() >= LIMIT) {
        let previous = dir.join(PREVIOUS);
        if previous.exists() && fs::remove_file(&previous).is_err() { return; }
        if fs::rename(&path, previous).is_err() { return; }
    }
    if let Ok(mut log) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(log, "{} {}", chrono::Utc::now().to_rfc3339(), event);
    }
}

pub fn record_failure(command: &str, output: &str) {
    let verb = command.strip_prefix("git ").and_then(|command| command.split_whitespace().next())
        .filter(|verb| matches!(*verb, "clone" | "fetch" | "pull" | "push" | "rebase" | "merge" | "commit" | "checkout" | "stash" | "bisect"))
        .unwrap_or("operation");
    let output = output.to_ascii_lowercase();
    let cause = if output.contains("timed out") { "timeout" }
        else if output.contains("cancelled") { "cancelled" }
        else if output.contains("authentication") || output.contains("permission denied") { "authentication or permissions" }
        else if output.contains("conflict") { "conflict" }
        else if output.contains("could not start") { "could not start Git" }
        else { "Git returned an error" };
    // Command arguments and raw Git output may contain repository text or credentials.
    write(&format!("Git {verb}: {cause}"));
}

pub fn report(version: &str) -> String {
    let program = crate::prefs::git_program();
    let cwd = crate::sample::data_dir();
    let output = crate::read_jobs::run_bounded(crate::git::git_command(
        cwd.parent().filter(|path| path.is_dir()).unwrap_or_else(|| Path::new(".")), &["--version"],
    ), None, Duration::from_secs(5), "Git version check timed out.");
    let version_line = output.stdout.lines().next().filter(|line| line.starts_with("git version ")
        && line.len() < 80 && line.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b' ' | b'.' | b'-' | b'+')))
        .unwrap_or("Unavailable");
    let source = if program == "git" { "System PATH" } else { "Configured executable in Preferences" };
    let recent = [PREVIOUS, FILE].into_iter().filter_map(|file| {
        let path = cwd.join(file);
        let metadata = path.symlink_metadata().ok()?;
        if !metadata.is_file() || metadata.len() > LIMIT + 1024 { return None; }
        fs::read_to_string(path).ok()
    }).collect::<Vec<_>>().join("");
    let lines: Vec<_> = recent.lines().rev().take(20).collect();
    let events = if lines.is_empty() { "No recent entries.".into() } else { lines.join("\n") };
    format!("GitCraft {version}\nPlatform: {} ({})\nGit: {version_line}\nGit executable source: {source}\nRecent diagnostic entries (newest first)\n{events}",
        std::env::consts::OS, std::env::consts::ARCH)
}
