//! Live status of coding-agent sessions, reported by the agents themselves.
//!
//! GitCraft installs small hooks into the agents that support them (Claude Code hooks in `settings.json`, Codex's
//! `notify` program in `config.toml`). A hook runs `gitcraft --agent-event <agent>`, which records the session's
//! state as a file in `<data dir>/agent-status`, and the Agents view reads those files.
//!
//! A Claude Code permission request is recorded as well, and the hook then waits a while for GitCraft's Allow or
//! Deny. Without an answer it says nothing, so Claude Code asks in its own terminal as usual.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long a permission request waits for an answer from GitCraft before Claude Code asks in its terminal.
const ANSWER_WITHIN: Duration = Duration::from_secs(120);
/// States older than this are no longer shown as live.
const STALE_AFTER: u64 = 6 * 60 * 60;
/// What the installed hooks' commands contain, so GitCraft can find and remove its own.
const MARKER: &str = "--agent-event";

/// One session's latest state.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct AgentState {
    pub agent: String,
    pub session: String,
    /// The folder the agent runs in.
    pub cwd: String,
    /// "running", "waiting" (for your input), "permission" (asking to use a tool) or "done".
    pub state: String,
    /// What the agent said, when it said something (a notification).
    #[serde(default)]
    pub message: String,
    /// Unix seconds.
    pub time: u64,
    /// The permission request waiting for an answer.
    #[serde(default)]
    pub request: Option<PermissionRequest>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct PermissionRequest {
    pub id: String,
    pub tool: String,
    /// The command or input the tool would run with, shortened.
    pub detail: String,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

pub fn status_dir() -> PathBuf {
    crate::sample::data_dir().join("agent-status")
}

/// A file name from a session id: letters, digits, `-` and `_` only.
fn safe(id: &str) -> String {
    let s: String = id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(80).collect();
    if s.is_empty() { "session".into() } else { s }
}

fn write_state(dir: &Path, state: &AgentState) {
    let _ = std::fs::create_dir_all(dir);
    if let Ok(bytes) = serde_json::to_vec(state) {
        let file = dir.join(format!("{}.json", safe(&state.session)));
        let staged = file.with_extension("json.tmp");
        if std::fs::write(&staged, bytes).is_ok() {
            let _ = std::fs::rename(&staged, &file);
        }
    }
}

/// The live sessions (newest first); old ones are cleaned up.
pub fn read_all() -> Vec<AgentState> {
    read_in(&status_dir())
}

fn read_in(dir: &Path) -> Vec<AgentState> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<AgentState> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| {
            let state: AgentState = serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok()?;
            if now().saturating_sub(state.time) > STALE_AFTER {
                let _ = std::fs::remove_file(e.path());
                return None;
            }
            Some(state)
        })
        .collect();
    out.sort_by_key(|s| std::cmp::Reverse(s.time));
    out
}

/// Answer a permission request shown in GitCraft.
pub fn answer(session: &str, request: &str, allow: bool) -> Result<(), String> {
    let dir = status_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let body = json!({ "id": request, "allow": allow });
    std::fs::write(dir.join(format!("{}.answer", safe(session))), body.to_string()).map_err(|e| e.to_string())
}

/// `gitcraft --agent-event <agent> [payload]`: what a hook reports. `stdin` is the hook's input (Claude Code); Codex
/// passes its payload as the last argument. Returns what to print for the agent (a permission decision), if anything.
pub fn handle_event(agent: &str, payload: &str) -> Option<String> {
    handle_event_in(&status_dir(), agent, payload, ANSWER_WITHIN)
}

fn handle_event_in(dir: &Path, agent: &str, payload: &str, wait: Duration) -> Option<String> {
    let v: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
    let text = |key: &str| v[key].as_str().unwrap_or_default().to_owned();
    match agent {
        "claude" => {
            let session = text("session_id");
            if session.is_empty() {
                return None;
            }
            let event = text("hook_event_name");
            let mut state = AgentState { agent: "claude".into(), session: session.clone(), cwd: text("cwd"), time: now(), ..Default::default() };
            match event.as_str() {
                "UserPromptSubmit" | "PreToolUse" | "PostToolUse" | "SessionStart" => state.state = "running".into(),
                "Notification" => {
                    state.state = "waiting".into();
                    state.message = text("message");
                }
                "Stop" | "SubagentStop" => state.state = "done".into(),
                "SessionEnd" => {
                    let _ = std::fs::remove_file(dir.join(format!("{}.json", safe(&session))));
                    return None;
                }
                "PermissionRequest" => return permission(dir, state, &v, wait),
                _ => return None,
            }
            write_state(dir, &state);
            None
        }
        "codex" => {
            // Codex's notify program hears when a turn is complete: the agent waits for you.
            if text("type") != "agent-turn-complete" {
                return None;
            }
            let session = [text("thread-id"), text("turn-id")].into_iter().find(|s| !s.is_empty()).unwrap_or_else(|| "codex".into());
            let cwd = std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
            let message = text("last-assistant-message").chars().take(300).collect();
            write_state(dir, &AgentState { agent: "codex".into(), session, cwd, state: "waiting".into(), message, time: now(), request: None });
            None
        }
        _ => None,
    }
}

/// Record the request, wait for GitCraft's answer, and give Claude Code the decision (or nothing, to ask as usual).
fn permission(dir: &Path, mut state: AgentState, v: &Value, wait: Duration) -> Option<String> {
    let tool = v["tool_name"].as_str().unwrap_or("a tool").to_owned();
    let input = &v["tool_input"];
    let detail = input["command"].as_str().or_else(|| input["file_path"].as_str()).map(str::to_owned).unwrap_or_else(|| input.to_string());
    let id = format!("{}-{}", now(), std::process::id());
    state.state = "permission".into();
    state.request = Some(PermissionRequest { id: id.clone(), tool, detail: detail.chars().take(2000).collect() });
    write_state(dir, &state);
    let answer_file = dir.join(format!("{}.answer", safe(&state.session)));
    let started = std::time::Instant::now();
    let decision = loop {
        if let Ok(bytes) = std::fs::read(&answer_file) {
            let a: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            if a["id"].as_str() == Some(id.as_str()) {
                let _ = std::fs::remove_file(&answer_file);
                break a["allow"].as_bool();
            }
        }
        if started.elapsed() >= wait {
            break None;
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    state.request = None;
    state.state = "running".into();
    state.time = now();
    write_state(dir, &state);
    let allow = decision?;
    let behavior = if allow { "allow" } else { "deny" };
    let mut decision = json!({ "behavior": behavior });
    if !allow {
        decision["message"] = json!("Denied in GitCraft.");
    }
    Some(json!({ "hookSpecificOutput": { "hookEventName": "PermissionRequest", "decision": decision } }).to_string())
}

/// The command a hook runs: this program with `--agent-event`.
fn hook_command(agent: &str) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Ok(format!("\"{}\" {MARKER} {agent}", exe.to_string_lossy()))
}

fn claude_settings(config_dir: Option<&str>) -> PathBuf {
    match config_dir.map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => crate::agents::home_dir().join(".claude"),
    }
    .join("settings.json")
}

const CLAUDE_EVENTS: [&str; 7] = ["SessionStart", "UserPromptSubmit", "PreToolUse", "Notification", "Stop", "SessionEnd", "PermissionRequest"];

/// Whether GitCraft's hooks are in Claude Code's settings.
pub fn claude_installed(config_dir: Option<&str>) -> bool {
    std::fs::read_to_string(claude_settings(config_dir)).is_ok_and(|s| s.contains(MARKER))
}

/// Add GitCraft's status hooks to Claude Code's `settings.json`, keeping everything else in it.
pub fn install_claude(config_dir: Option<&str>) -> Result<(), String> {
    let file = claude_settings(config_dir);
    let mut settings: Value = match std::fs::read_to_string(&file) {
        Ok(text) if !text.trim().is_empty() => serde_json::from_str(&text).map_err(|_| format!("{} is not valid JSON, so GitCraft left it alone.", file.display()))?,
        _ => json!({}),
    };
    remove_claude_hooks(&mut settings);
    let command = hook_command("claude")?;
    let hooks = settings.as_object_mut().ok_or("Claude Code's settings are not an object.")?.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks.as_object_mut().ok_or("Claude Code's hooks are not an object.")?;
    for event in CLAUDE_EVENTS {
        let timeout = if event == "PermissionRequest" { ANSWER_WITHIN.as_secs() + 10 } else { 10 };
        let entry = json!({ "matcher": "*", "hooks": [{ "type": "command", "command": command, "timeout": timeout }] });
        let list = hooks.entry(event).or_insert_with(|| json!([]));
        if let Some(list) = list.as_array_mut() {
            list.push(entry);
        }
    }
    save_json(&file, &settings)
}

/// Take GitCraft's hooks out of Claude Code's settings again.
pub fn uninstall_claude(config_dir: Option<&str>) -> Result<(), String> {
    let file = claude_settings(config_dir);
    let Ok(text) = std::fs::read_to_string(&file) else { return Ok(()) };
    let mut settings: Value = serde_json::from_str(&text).map_err(|_| format!("{} is not valid JSON, so GitCraft left it alone.", file.display()))?;
    remove_claude_hooks(&mut settings);
    save_json(&file, &settings)
}

fn remove_claude_hooks(settings: &mut Value) {
    let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) else { return };
    for list in hooks.values_mut().filter_map(Value::as_array_mut) {
        list.retain(|entry| !entry["hooks"].as_array().is_some_and(|h| h.iter().any(|c| c["command"].as_str().is_some_and(|c| c.contains(MARKER)))));
    }
    hooks.retain(|_, list| list.as_array().is_none_or(|l| !l.is_empty()));
}

fn save_json(file: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    crate::files::write_replacing(file, text.as_bytes()).map_err(|e| e.to_string())
}

fn codex_config(config_dir: Option<&str>) -> PathBuf {
    match config_dir.map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => crate::agents::home_dir().join(".codex"),
    }
    .join("config.toml")
}

/// Whether GitCraft is Codex's notify program.
pub fn codex_installed(config_dir: Option<&str>) -> bool {
    std::fs::read_to_string(codex_config(config_dir)).is_ok_and(|s| s.lines().any(|l| l.trim_start().starts_with("notify") && l.contains(MARKER)))
}

/// Make GitCraft Codex's notify program. Codex has one; another program already there is left in place.
pub fn install_codex(config_dir: Option<&str>) -> Result<(), String> {
    let file = codex_config(config_dir);
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    if text.lines().any(|l| l.trim_start().starts_with("notify") && !l.contains(MARKER)) {
        return Err("Codex already runs another notify program, so GitCraft left it in place.".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let quoted = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    let line = format!("notify = [{}, {}, {}]", quoted(&exe.to_string_lossy()), quoted(MARKER), quoted("codex"));
    // A top-level key belongs before the first table.
    let kept: Vec<&str> = text.lines().filter(|l| !(l.trim_start().starts_with("notify") && l.contains(MARKER))).collect();
    let body = format!("{line}\n{}", kept.join("\n"));
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    crate::files::write_replacing(&file, body.trim_end().to_owned().add_newline().as_bytes()).map_err(|e| e.to_string())
}

/// Stop being Codex's notify program.
pub fn uninstall_codex(config_dir: Option<&str>) -> Result<(), String> {
    let file = codex_config(config_dir);
    let Ok(text) = std::fs::read_to_string(&file) else { return Ok(()) };
    let kept: Vec<&str> = text.lines().filter(|l| !(l.trim_start().starts_with("notify") && l.contains(MARKER))).collect();
    crate::files::write_replacing(&file, kept.join("\n").trim_end().to_owned().add_newline().as_bytes()).map_err(|e| e.to_string())
}

trait AddNewline {
    fn add_newline(self) -> String;
}

impl AddNewline for String {
    fn add_newline(mut self) -> String {
        if !self.is_empty() {
            self.push('\n');
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_events_become_states_and_permissions_wait_for_an_answer() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let prompt = json!({ "session_id": "abc", "cwd": "/repo", "hook_event_name": "UserPromptSubmit" }).to_string();
        assert_eq!(handle_event_in(d, "claude", &prompt, Duration::ZERO), None);
        assert_eq!(read_in(d)[0].state, "running");
        let note = json!({ "session_id": "abc", "cwd": "/repo", "hook_event_name": "Notification", "message": "Claude needs your input" }).to_string();
        handle_event_in(d, "claude", &note, Duration::ZERO);
        assert_eq!(read_in(d)[0].message, "Claude needs your input");
        // Unanswered: nothing is printed, so Claude Code asks in its terminal.
        let ask = json!({ "session_id": "abc", "cwd": "/repo", "hook_event_name": "PermissionRequest", "tool_name": "Bash", "tool_input": { "command": "rm -rf build" } }).to_string();
        assert_eq!(handle_event_in(d, "claude", &ask, Duration::ZERO), None);
        // Answered while waiting: the decision goes back to Claude Code.
        let d2 = d.to_path_buf();
        let waiter = std::thread::spawn(move || handle_event_in(&d2, "claude", &ask, Duration::from_secs(10)));
        let request = loop {
            if let Some(r) = read_in(d).into_iter().find_map(|s| s.request) {
                break r;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        assert_eq!(request.detail, "rm -rf build");
        std::fs::write(d.join("abc.answer"), json!({ "id": request.id, "allow": false }).to_string()).unwrap();
        let out = waiter.join().unwrap().unwrap();
        assert!(out.contains("\"deny\""));
        let end = json!({ "session_id": "abc", "hook_event_name": "SessionEnd" }).to_string();
        handle_event_in(d, "claude", &end, Duration::ZERO);
        assert!(read_in(d).is_empty());
    }

    #[test]
    fn claude_hooks_are_added_and_removed_without_touching_others() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().to_string_lossy().into_owned();
        std::fs::write(dir.path().join("settings.json"), r#"{"model":"x","hooks":{"Stop":[{"hooks":[{"type":"command","command":"say done"}]}]}}"#).unwrap();
        install_claude(Some(&config)).unwrap();
        assert!(claude_installed(Some(&config)));
        install_claude(Some(&config)).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("settings.json")).unwrap()).unwrap();
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 2, "installed once, beside the user's own hook");
        uninstall_claude(Some(&config)).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("settings.json")).unwrap()).unwrap();
        assert_eq!(v["model"], "x");
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], "say done");
        assert!(v["hooks"].get("PermissionRequest").is_none());
    }

    #[test]
    fn codex_notify_is_set_only_when_free() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().to_string_lossy().into_owned();
        std::fs::write(dir.path().join("config.toml"), "model = \"o4\"\n\n[profiles.x]\nmodel = \"y\"\n").unwrap();
        install_codex(Some(&config)).unwrap();
        let text = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        assert!(text.starts_with("notify = ["));
        assert!(codex_installed(Some(&config)));
        uninstall_codex(Some(&config)).unwrap();
        assert!(!codex_installed(Some(&config)));
        std::fs::write(dir.path().join("config.toml"), "notify = [\"other\"]\n").unwrap();
        assert!(install_codex(Some(&config)).is_err());
    }
}
