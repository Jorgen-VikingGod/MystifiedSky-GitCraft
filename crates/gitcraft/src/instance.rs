//! One GitCraft window per user: a folder opened from the file manager, the command line or a
//! `gitcraft://` link while GitCraft runs lands as a tab in the running window instead of starting
//! a second copy.
//!
//! The first launch takes an exclusive lock on `gitcraft.lock` in the user's runtime folder (the
//! OS drops it when the process ends, crash included) and starts a Craft Link server, which
//! publishes where it listens. A later launch that finds the lock taken looks the server up,
//! signs in with the token from its record and sends what it was asked to open as `app.activate`;
//! the running window queues it and comes to the front. A launch that can't hand over (the
//! running copy is still starting or is quitting) keeps trying for a while, takes the lock over as
//! soon as it is free, and otherwise opens its own window: a folder is never dropped.

use craft_ipc::client::{Client, HelloClient};
use craft_ipc::server::{Activate, Activation, ControlSink, FrameOpen, Frames, Server, ServerConfig};
use craft_ipc::{Discovery, LinkError, RuntimeDir};
use gitcraft_core::session::LaunchIntent;
use serde_json::{Value, json};
use std::fs::File;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

const APP: &str = "gitcraft";
const HANDOFF: Duration = Duration::from_secs(10);
/// More than this in one launch is not a file manager.
const MAX_INTENTS: usize = 64;

/// How long a launch waits for the window to take what it handed over.
const TAKEN_WITHIN: Duration = Duration::from_secs(3);

/// What the running window has been asked to open, and how to wake it.
#[derive(Default)]
struct Queue {
    intents: Vec<LaunchIntent>,
    ctx: Option<egui::Context>,
    /// Deliveries so far, and how many of them the window has taken.
    delivered: u64,
    taken: u64,
    /// The window is quitting: what arrives now would not be opened.
    closing: bool,
}

fn queue() -> &'static (Mutex<Queue>, Condvar) {
    static QUEUE: OnceLock<(Mutex<Queue>, Condvar)> = OnceLock::new();
    QUEUE.get_or_init(Default::default)
}

fn lock_queue() -> std::sync::MutexGuard<'static, Queue> {
    queue().0.lock().unwrap_or_else(|e| e.into_inner())
}

/// Hand launch intents to the window (from the server, a macOS event or a Finder service) and
/// bring it to the front. Returns the delivery's number, or `None` when the window is quitting.
pub fn deliver(intents: Vec<LaunchIntent>) -> Option<u64> {
    let mut q = lock_queue();
    if q.closing {
        return None;
    }
    q.intents.extend(intents);
    q.delivered += 1;
    if let Some(ctx) = &q.ctx {
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        // Where the system doesn't let a window take the focus by itself (Wayland), mark it.
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
        ctx.request_repaint();
    }
    Some(q.delivered)
}

/// The window is up: later deliveries wake it.
pub fn attach(ctx: &egui::Context) {
    lock_queue().ctx = Some(ctx.clone());
}

/// What arrived since the last frame.
pub fn take() -> Vec<LaunchIntent> {
    let mut q = lock_queue();
    q.taken = q.delivered;
    let intents = std::mem::take(&mut q.intents);
    drop(q);
    queue().1.notify_all();
    intents
}

/// The window is quitting: later launches open their own window.
pub fn close() {
    lock_queue().closing = true;
    queue().1.notify_all();
}

/// Read the `open` object of an `app.activate` call. `None` when the request can't be taken as a
/// whole (too many items, or one that isn't valid), so nothing of it is opened and the sender
/// opens its own window instead.
pub fn intents_from(open: &Value) -> Option<Vec<LaunchIntent>> {
    let list = open.get("gitcraft").and_then(Value::as_array)?;
    if list.len() > MAX_INTENTS {
        return None;
    }
    list.iter()
        .map(|item| match (item["kind"].as_str(), item["path"].as_str(), item["url"].as_str()) {
            // Any folder a launch may name on the command line.
            (Some("repository"), Some(path), _) if !path.is_empty() && !path.contains('\0') => Some(LaunchIntent::Repository { path: path.to_owned() }),
            (Some("clone"), _, Some(url)) => gitcraft_core::session::clone_url(url).map(|url| LaunchIntent::Clone { url }),
            _ => None,
        })
        .collect()
}

pub fn open_payload(intents: &[LaunchIntent]) -> Value {
    json!({ "gitcraft": intents })
}

struct Window;

impl Activate for Window {
    fn activate(&self, _token: Option<String>, _from_pid: Option<u32>, open: Option<Value>) -> Activation {
        let intents = match open.as_ref().map(intents_from) {
            Some(Some(intents)) => intents,
            None => Vec::new(),
            // Refused as a whole: the sender opens a window of its own.
            Some(None) => return Activation::No,
        };
        let has_window = lock_queue().ctx.is_some();
        let Some(number) = deliver(intents) else { return Activation::No };
        // Queued before the window opened, they are shown with it.
        if !has_window {
            return Activation::Yes;
        }
        // Answer once the window took them; if it quits first, the sender takes over.
        let (lock, taken) = queue();
        let q = lock.lock().unwrap_or_else(|e| e.into_inner());
        let (q, _) = taken.wait_timeout_while(q, TAKEN_WITHIN, |q| q.taken < number && !q.closing).unwrap_or_else(|e| e.into_inner());
        if q.taken >= number {
            Activation::Requested
        } else if q.closing {
            Activation::No
        } else {
            // Still queued in a running window: it opens them on its next frame.
            Activation::Requested
        }
    }
}

/// GitCraft takes no other Craft Link requests.
struct Nothing;

impl ControlSink for Nothing {
    fn submit(&self, _method: String, _params: Value) -> Option<std::sync::mpsc::Receiver<Value>> {
        None
    }
}

impl Frames for Nothing {
    fn open(&self, _params: &serde_json::Map<String, Value>) -> Result<FrameOpen, LinkError> {
        Err(LinkError::NotReady("GitCraft shares no pictures".into()))
    }
    fn render(&self, _ticket: &str, _header: &Value) -> Result<(Value, Vec<u8>), LinkError> {
        Err(LinkError::NotReady("GitCraft shares no pictures".into()))
    }
    fn close_ticket(&self, _ticket: &str) {}
}

/// Kept for the life of the running window.
pub struct Primary {
    _lock: File,
    server: Option<Server>,
    /// What starting the server again needs, if quitting does not go through.
    runtime: RuntimeDir,
    version: String,
}

impl Primary {
    /// Stop taking launches (GitCraft is quitting), so the next one starts its own window.
    pub fn shutdown(&mut self) {
        close();
        if let Some(server) = self.server.take() {
            let _ = server.shutdown();
        }
    }

    /// Quitting did not happen (the install or the relaunch failed): take launches again.
    pub fn resume(&mut self) {
        lock_queue().closing = false;
        if self.server.is_none() {
            self.server = start_server(&self.runtime, &self.version);
        }
    }
}

pub enum Claim {
    /// This process is the running window.
    Primary(Box<Primary>),
    /// The running window took the launch; this process exits.
    Forwarded,
    /// Neither: open a window of our own (the reason, for the log).
    Standalone(String),
}

/// Become the running window, or hand `intents` to the one that is.
pub fn claim(intents: &[LaunchIntent]) -> Claim {
    let runtime = match RuntimeDir::resolve() {
        Ok(r) => r,
        Err(e) => return Claim::Standalone(format!("no runtime folder: {e}")),
    };
    claim_in(&runtime, intents, HANDOFF, env!("CARGO_PKG_VERSION"))
}

pub fn claim_in(runtime: &RuntimeDir, intents: &[LaunchIntent], budget: Duration, version: &str) -> Claim {
    // More than one launch hands over: open them in a window of our own.
    if intents.len() > MAX_INTENTS {
        return Claim::Standalone(format!("{} folders are more than a launch hands over", intents.len()));
    }
    let lock_path = runtime.path().join(format!("{APP}.lock"));
    let file = match File::options().create(true).truncate(false).write(true).open(&lock_path) {
        Ok(f) => f,
        Err(e) => return Claim::Standalone(format!("could not open {}: {e}", lock_path.display())),
    };
    let deadline = Instant::now() + budget;
    let mut last;
    loop {
        match file.try_lock() {
            Ok(()) => return Claim::Primary(Box::new(start(file, runtime, version))),
            Err(std::fs::TryLockError::WouldBlock) => match forward(runtime, intents, version) {
                Ok(()) => return Claim::Forwarded,
                Err(e) => last = e,
            },
            Err(std::fs::TryLockError::Error(e)) => return Claim::Standalone(format!("could not lock {}: {e}", lock_path.display())),
        }
        if Instant::now() >= deadline {
            return Claim::Standalone(last);
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

fn start(lock: File, runtime: &RuntimeDir, version: &str) -> Primary {
    Primary { _lock: lock, server: start_server(runtime, version), runtime: runtime.clone(), version: version.to_owned() }
}

fn start_server(runtime: &RuntimeDir, version: &str) -> Option<Server> {
    let config = ServerConfig {
        app: APP.into(),
        app_version: version.into(),
        instance: None,
        runtime: Some(runtime.clone()),
        tcp_port: None,
        sink: Arc::new(Nothing),
        activate: Arc::new(Window),
        frames: Arc::new(Nothing),
    };
    match Server::start(config) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("GitCraft could not take launches from other windows: {e}");
            None
        }
    }
}

fn forward(runtime: &RuntimeDir, intents: &[LaunchIntent], version: &str) -> Result<(), String> {
    let records = Discovery::scan(runtime).map_err(|e| e.to_string())?;
    let mut last = "the running copy has not published where it listens yet".to_owned();
    for record in records.iter().filter(|r| r.app == APP) {
        let call = (|| {
            let mut client = Client::connect(record)?;
            client.hello(&HelloClient { app: APP.into(), app_version: version.into(), pid: std::process::id(), instance: craft_ipc::random_uuid()?, protocol_min: 1, protocol_max: 1 })?;
            client.call("app.activate", json!({ "from_pid": std::process::id(), "open": open_payload(intents) }))
        })();
        match call.map(|reply| handed_over(&reply)) {
            Ok(true) => return Ok(()),
            // Refused (the window is quitting, or can't take this): keep trying, then open our own.
            Ok(false) => last = "the running copy did not take the launch".to_owned(),
            Err(e) => last = e.to_string(),
        }
    }
    Err(last)
}

/// Whether an `app.activate` reply (`{"activated": true | false | "requested"}`) says the running
/// window took the launch; anything else counts as refused.
fn handed_over(reply: &Value) -> bool {
    matches!(reply.get("activated"), Some(Value::Bool(true))) || reply.get("activated").and_then(Value::as_str) == Some("requested")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_payloads_round_trip_and_are_checked() {
        let intents = vec![LaunchIntent::Repository { path: "/home/a/repo".into() }, LaunchIntent::Clone { url: "https://example.invalid/a/b.git".into() }];
        assert_eq!(intents_from(&open_payload(&intents)), Some(intents));
        // A folder name may hold any character but NUL, as on the command line.
        let newline = vec![LaunchIntent::Repository { path: "/home/a/odd\nname".into() }];
        assert_eq!(intents_from(&open_payload(&newline)), Some(newline));
        // One bad item, or too many, refuses the whole request.
        for bad in [json!({"gitcraft": [{"kind": "repository", "path": "/a"}, {"kind": "clone", "url": "file:///etc"}]}), json!({"gitcraft": [{"kind": "other"}]})] {
            assert_eq!(intents_from(&bad), None);
        }
        let many: Vec<_> = (0..=MAX_INTENTS).map(|i| LaunchIntent::Repository { path: format!("/r{i}") }).collect();
        assert_eq!(intents_from(&open_payload(&many)), None);
    }

    #[test]
    fn only_a_taken_launch_counts_as_handed_over() {
        assert!(handed_over(&json!({"activated": true})));
        assert!(handed_over(&json!({"activated": "requested"})));
        for refused in [json!({"activated": false}), json!(false), json!({}), json!({"activated": "no"})] {
            assert!(!handed_over(&refused));
        }
    }

    #[test]
    fn a_second_launch_hands_its_folders_to_the_first() {
        let dir = std::env::temp_dir().join(format!("gitcraft-instance-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let runtime = RuntimeDir::at(&dir).unwrap();
        let Claim::Primary(mut primary) = claim_in(&runtime, &[], Duration::from_secs(1), "0.1.0") else { panic!("first launch should run the window") };
        let wanted = vec![LaunchIntent::Repository { path: "/tmp/x".into() }];
        assert!(matches!(claim_in(&runtime, &wanted, Duration::from_secs(5), "0.1.0"), Claim::Forwarded));
        assert_eq!(take(), wanted);
        primary.shutdown();
        drop(primary);
        assert!(matches!(claim_in(&runtime, &[], Duration::from_secs(1), "0.1.0"), Claim::Primary(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
