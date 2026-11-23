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
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const APP: &str = "gitcraft";
const HANDOFF: Duration = Duration::from_secs(10);
/// More than this in one launch is not a file manager.
const MAX_INTENTS: usize = 64;

/// What the running window has been asked to open, and how to wake it.
#[derive(Default)]
struct Queue {
    intents: Vec<LaunchIntent>,
    ctx: Option<egui::Context>,
}

fn queue() -> &'static Mutex<Queue> {
    static QUEUE: OnceLock<Mutex<Queue>> = OnceLock::new();
    QUEUE.get_or_init(Default::default)
}

/// Hand launch intents to the window (from the server, a macOS event or a Finder service) and
/// bring it to the front.
pub fn deliver(intents: Vec<LaunchIntent>) {
    let mut q = queue().lock().unwrap_or_else(|e| e.into_inner());
    q.intents.extend(intents);
    if let Some(ctx) = &q.ctx {
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        ctx.request_repaint();
    }
}

/// The window is up: later deliveries wake it.
pub fn attach(ctx: &egui::Context) {
    queue().lock().unwrap_or_else(|e| e.into_inner()).ctx = Some(ctx.clone());
}

/// What arrived since the last frame.
pub fn take() -> Vec<LaunchIntent> {
    std::mem::take(&mut queue().lock().unwrap_or_else(|e| e.into_inner()).intents)
}

/// Read the `open` object of an `app.activate` call.
pub fn intents_from(open: &Value) -> Vec<LaunchIntent> {
    let Some(list) = open.get("gitcraft").and_then(Value::as_array) else { return Vec::new() };
    list.iter()
        .take(MAX_INTENTS)
        .filter_map(|item| match (item["kind"].as_str(), item["path"].as_str(), item["url"].as_str()) {
            (Some("repository"), Some(path), _) if !path.is_empty() && !path.chars().any(char::is_control) => Some(LaunchIntent::Repository { path: path.to_owned() }),
            // Checked again as a link, so the running copy applies its own rules.
            (Some("clone"), _, Some(url)) => gitcraft_core::session::clone_launch(&format!("gitcraft://clone?url={}", url::form_urlencoded::byte_serialize(url.as_bytes()).collect::<String>())),
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
        let has_window = queue().lock().unwrap_or_else(|e| e.into_inner()).ctx.is_some();
        deliver(open.as_ref().map(intents_from).unwrap_or_default());
        // Queued before the window opened, they are shown with it.
        if has_window { Activation::Requested } else { Activation::Yes }
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
}

impl Primary {
    /// Stop taking launches (GitCraft is quitting), so the next one starts its own window.
    pub fn shutdown(&mut self) {
        if let Some(server) = self.server.take() {
            let _ = server.shutdown();
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
    let server = match Server::start(config) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("GitCraft could not take launches from other windows: {e}");
            None
        }
    };
    Primary { _lock: lock, server }
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
        match call {
            Ok(_) => return Ok(()),
            Err(e) => last = e.to_string(),
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_payloads_round_trip_and_are_checked() {
        let intents = vec![LaunchIntent::Repository { path: "/home/a/repo".into() }, LaunchIntent::Clone { url: "https://example.invalid/a/b.git".into() }];
        assert_eq!(intents_from(&open_payload(&intents)), intents);
        let bad = json!({"gitcraft": [{"kind": "clone", "url": "file:///etc"}, {"kind": "repository", "path": "a\nb"}, {"kind": "other"}]});
        assert!(intents_from(&bad).is_empty());
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
