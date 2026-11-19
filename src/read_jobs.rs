//! Bounded Git subprocess execution with an independent concurrency limit for reads.
use std::{collections::HashMap, sync::{Arc, Mutex, OnceLock}, time::{Duration, Instant}, process::{Command, Stdio}, io::Read};
use crate::git::GitOut;
use std::{future::Future, sync::atomic::{AtomicBool, Ordering}};
#[derive(Clone)]
pub struct Context { cancelled: tokio::sync::watch::Sender<bool>, deadline: Instant }
thread_local! { static CONTEXT: std::cell::RefCell<Option<Context>> = const { std::cell::RefCell::new(None) }; }
fn requests() -> &'static Mutex<HashMap<String, Context>> { static REQUESTS: OnceLock<Mutex<HashMap<String, Context>>> = OnceLock::new(); REQUESTS.get_or_init(Default::default) }
pub fn cancel(id: &str) {
    let mut map = requests().lock().unwrap_or_else(|error| error.into_inner());
    if let Some(context) = map.get(id) { context.cancelled.send_replace(true); }
    else {
        map.retain(|_, c| c.deadline > Instant::now());
        if map.len() < 1024 { map.insert(id.into(), Context { cancelled: tokio::sync::watch::channel(true).0, deadline: Instant::now() + Duration::from_secs(120) }); }
    }
}
pub fn scope<T>(id: Option<String>, f: impl FnOnce() -> T) -> T {
    scope_for(id, Duration::from_secs(120), f)
}
pub fn scope_for<T>(id: Option<String>, timeout: Duration, f: impl FnOnce() -> T) -> T {
    let context = Context { cancelled: tokio::sync::watch::channel(false).0, deadline: Instant::now() + timeout };
    let context = if let Some(id) = &id {
        let mut map = requests().lock().unwrap_or_else(|error| error.into_inner());
        map.retain(|_, c| c.deadline > Instant::now());
        map.entry(id.clone()).or_insert(context).clone()
    } else { context };
    let result = with(context, f);
    if let Some(id) = id { requests().lock().unwrap_or_else(|error| error.into_inner()).remove(&id); }
    result
}
fn with<T>(context: Context, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<Context>);
    impl Drop for Restore { fn drop(&mut self) { CONTEXT.with(|c| c.replace(self.0.take())); } }
    let _restore = Restore(CONTEXT.with(|c| c.replace(Some(context))));
    f()
}
pub fn inherit<T>(f: impl FnOnce() -> T) -> impl FnOnce() -> T {
    let context = CONTEXT.with(|c| c.borrow().clone());
    move || match context { Some(context) => with(context, f), None => f() }
}
pub fn context() -> Option<Context> { CONTEXT.with(|c| c.borrow().clone()) }
pub async fn wait_http<T>(operation: impl Future<Output = Result<T, String>>, context: Option<Context>, timeout: Duration) -> Result<T, String> {
    let expiry = context.as_ref().map_or(timeout, |context| timeout.min(context.deadline.saturating_duration_since(Instant::now())));
    let cancelled = async {
        if let Some(context) = &context {
            let mut receiver = context.cancelled.subscribe();
            let _ = receiver.wait_for(|cancelled| *cancelled).await;
        } else {
            std::future::pending::<()>().await;
        }
    };
    tokio::select! {
        biased;
        () = cancelled => Err("Request cancelled".into()),
        result = tokio::time::timeout(expiry, operation) => result.map_err(|_| "The request timed out.".to_string())?,
    }
}
pub fn check() -> Result<(), String> {
    if CONTEXT.with(|c| c.borrow().as_ref().is_some_and(stopped)) { Err("Read cancelled or timed out".into()) } else { Ok(()) }
}
pub fn lock<T>(mutex: &Mutex<T>) -> Result<std::sync::MutexGuard<'_, T>, String> {
    loop {
        check()?;
        match mutex.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(std::sync::TryLockError::Poisoned(error)) => return Ok(error.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}
pub fn has_context() -> bool { CONTEXT.with(|c| c.borrow().is_some()) }
#[derive(Default)]
struct ReadBudget {
    active: Mutex<usize>,
    changed: std::sync::Condvar,
}

fn budget() -> &'static ReadBudget {
    static BUDGET: OnceLock<ReadBudget> = OnceLock::new();
    BUDGET.get_or_init(Default::default)
}
pub fn active_count() -> usize { *budget().active.lock().unwrap_or_else(|error| error.into_inner()) }
fn stopped(context: &Context) -> bool { *context.cancelled.borrow() || Instant::now() >= context.deadline }
pub struct ReadOutput { pub ok: bool, pub stdout: Vec<u8>, pub stderr: Vec<u8> }
impl ReadOutput { fn failed(error: &str) -> Self { Self { ok: false, stdout: vec![], stderr: error.as_bytes().to_vec() } } }
pub fn run(command: Command, limit: usize, input: Option<Vec<u8>>) -> GitOut {
    let output = run_bytes(command, limit, input);
    GitOut { ok: output.ok, stdout: String::from_utf8_lossy(&output.stdout).into_owned(), stderr: String::from_utf8_lossy(&output.stderr).into_owned() }
}
pub fn run_mutation(command: Command, input: Option<Vec<u8>>) -> GitOut {
    run_bounded(command, input, Duration::from_secs(300), "Git command timed out. Review the repository state before retrying.")
}

pub fn run_bounded(command: Command, input: Option<Vec<u8>>, timeout: Duration, timeout_message: &'static str) -> GitOut {
    let context = Context { cancelled: tokio::sync::watch::channel(false).0, deadline: Instant::now() + timeout };
    let output = execute(command, 64 * 1024 * 1024, input, &context, timeout_message);
    GitOut { ok: output.ok, stdout: String::from_utf8_lossy(&output.stdout).into_owned(), stderr: String::from_utf8_lossy(&output.stderr).into_owned() }
}

pub fn run_bytes(command: Command, limit: usize, input: Option<Vec<u8>>) -> ReadOutput {
    run_with_budget(command, limit, input, budget())
}

fn run_with_budget(command: Command, limit: usize, input: Option<Vec<u8>>, budget: &ReadBudget) -> ReadOutput {
    let context = CONTEXT.with(|c| c.borrow().clone()).unwrap_or_else(|| Context { cancelled: tokio::sync::watch::channel(false).0, deadline: Instant::now() + Duration::from_secs(120) });
    let mut count = budget.active.lock().unwrap_or_else(|error| error.into_inner());
    while *count >= 8 && !stopped(&context) { count = budget.changed.wait_timeout(count, Duration::from_millis(20)).unwrap_or_else(|error| error.into_inner()).0; }
    if stopped(&context) { return ReadOutput::failed("Git read cancelled or timed out"); }
    *count += 1; drop(count);
    struct Permit<'a>(&'a ReadBudget);
    impl Drop for Permit<'_> {
        fn drop(&mut self) {
            *self.0.active.lock().unwrap_or_else(|error| error.into_inner()) -= 1;
            self.0.changed.notify_one();
        }
    }
    let _permit = Permit(budget);
    execute(command, limit, input, &context, "Git read cancelled or timed out")
}

fn execute(mut command: Command, limit: usize, input: Option<Vec<u8>>, context: &Context, timeout: &'static str) -> ReadOutput {
    if stopped(context) { return ReadOutput::failed(timeout); }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    if input.is_some() { command.stdin(Stdio::piped()); } else { command.stdin(Stdio::null()); }
    let (mut child, tree) = match crate::process_tree::ProcessTree::spawn(&mut command) {
        Ok(process) => process,
        Err(crate::process_tree::StartError::Tree(error)) => return ReadOutput::failed(&error),
        Err(error) => return ReadOutput::failed(&format!("Could not start git: {error}")),
    };
    let overflow = Arc::new(AtomicBool::new(false));
    fn reader(mut pipe: impl Read + Send + 'static, limit: usize, overflow: Arc<AtomicBool>) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || { let mut bytes = Vec::new(); let _ = (&mut pipe).take(limit as u64 + 1).read_to_end(&mut bytes); if bytes.len() > limit { overflow.store(true, Ordering::Release); bytes.truncate(limit); } bytes })
    }
    let stdout = reader(child.stdout.take().unwrap(), limit, overflow.clone());
    let stderr = reader(child.stderr.take().unwrap(), 64 * 1024, overflow.clone());
    let writer = input.map(|input| { let mut stdin = child.stdin.take().unwrap(); std::thread::spawn(move || { use std::io::Write; let _ = stdin.write_all(&input); }) });
    let mut failure = None;
    let status = loop {
        if stopped(context) || overflow.load(Ordering::Acquire) {
            failure = Some(if overflow.load(Ordering::Acquire) { "Git output exceeded the read budget" } else { timeout });
            tree.kill(); let _ = child.kill(); break child.wait().ok();
        }
        match child.try_wait() { Ok(Some(status)) => break Some(status), Ok(None) => std::thread::sleep(Duration::from_millis(10)), Err(_) => { tree.kill(); let _ = child.wait(); break None; } }
    };
    // Stop descendants which inherited the pipes even if the direct child already exited.
    drop(tree);
    let out = stdout.join().unwrap_or_default(); let err = stderr.join().unwrap_or_default();
    if let Some(writer) = writer { let _ = writer.join(); }
    if overflow.load(Ordering::Acquire) { failure = Some("Git output exceeded the read budget"); }
    if let Some(error) = failure { return ReadOutput::failed(error); }
    ReadOutput { ok: status.is_some_and(|s| s.success()), stdout: out, stderr: err }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queued_read_locks_can_be_cancelled_without_waiting_for_a_mutation() {
        let lock = Arc::new(Mutex::new(())); let guard = lock.lock().unwrap(); let held = lock.clone();
        let (started, ready) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || scope(Some("queued-read-lock".into()), || { started.send(()).unwrap(); super::lock(&held).is_err() }));
        ready.recv_timeout(Duration::from_secs(2)).unwrap(); cancel("queued-read-lock");
        assert!(worker.join().unwrap()); drop(guard);
    }
    #[test]
    fn eight_read_processes_are_the_limit_and_mutations_still_run() {
        let dir = tempfile::tempdir().unwrap(); let mut workers = Vec::new();
        let budget = Arc::new(ReadBudget::default());
        for i in 0..12 {
            let path = dir.path().to_owned();
            let budget = budget.clone();
            workers.push(std::thread::spawn(move || scope(Some(format!("budget-read-{i}")), || {
                let mut command = crate::git::git_cmd(&path);
                command.args(["-c", &format!("alias.busy=!echo ready > helper-{i}; sleep 30"), "busy"]);
                run_with_budget(command, 4096, None, &budget)
            })));
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while std::fs::read_dir(dir.path()).unwrap().count() < 8 { assert!(Instant::now() < deadline); std::thread::sleep(Duration::from_millis(10)); }
        assert_eq!(*budget.active.lock().unwrap(), 8); assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 8);
        let started = Instant::now(); assert!(crate::git::run(dir.path(), &["init"]).ok); assert!(started.elapsed() < Duration::from_secs(5));
        for i in 0..12 { cancel(&format!("budget-read-{i}")); }
        for worker in workers { assert!(!worker.join().unwrap().ok); }
        assert_eq!(*budget.active.lock().unwrap(), 0);
    }

    #[test]
    fn cancelled_repository_load_is_an_error_instead_of_empty_success() {
        let dir = tempfile::tempdir().unwrap();
        assert!(crate::git::run(dir.path(), &["init"]).ok);
        cancel("cancelled-repo-load");
        let result = scope(Some("cancelled-repo-load".into()), || crate::git::load_view_checked(dir.path(), 500, &Default::default()));
        assert!(result.unwrap_err().contains("cancelled"));
    }

    #[test]
    fn cancelled_before_start_never_spawns_and_deadlines_are_explicit() {
        cancel("cancel-before-start");
        let result = scope(Some("cancel-before-start".into()), || {
            let mut command = Command::new("git"); command.arg("--version"); run(command, 4096, None)
        });
        assert!(!result.ok); assert!(result.stderr.contains("cancelled"));
        let result = with(Context { cancelled: tokio::sync::watch::channel(false).0, deadline: Instant::now() }, || {
            let mut command = Command::new("git"); command.arg("--version"); run(command, 4096, None)
        });
        assert!(!result.ok); assert!(result.stderr.contains("timed out"));
    }
    #[test]
    fn cancelling_a_read_kills_its_helper_and_inherited_pipes() {
        let temp = tempfile::tempdir().unwrap(); let path = temp.path().to_path_buf();
        let marker = path.join("helper-ready");
        let worker = std::thread::spawn(move || scope(Some("read-helper".into()), || {
            let mut command = crate::git::git_cmd(&path);
            command.args(["-c", "alias.slow=!echo ready > helper-ready; sleep 30; echo leaked > late-marker", "slow"]);
            run(command, 4096, None)
        }));
        let deadline = Instant::now() + Duration::from_secs(10);
        while !marker.exists() && Instant::now() < deadline { std::thread::sleep(Duration::from_millis(10)); }
        assert!(marker.exists()); let now = Instant::now(); cancel("read-helper");
        let result = worker.join().unwrap(); assert!(!result.ok); assert!(now.elapsed() < Duration::from_secs(5));
        assert!(!temp.path().join("late-marker").exists());
    }
    #[test]
    fn oversized_reads_fail_instead_of_returning_partial_success() {
        let mut command = Command::new("git"); command.arg("--version");
        let result = run(command, 2, None); assert!(!result.ok); assert!(result.stderr.contains("budget"));
    }
}
