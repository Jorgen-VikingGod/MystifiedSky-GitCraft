//! Built-in terminal: a real shell in a pseudo-terminal (ConPTY on Windows), streamed to the
//! window's xterm.js panel. Each terminal tab is one session.

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

struct Session {
    owner: String,
    writer: std::sync::mpsc::SyncSender<Vec<u8>>,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    child: Box<dyn Child + Send + Sync>,
    output: Arc<OutputFlow>,
}

#[derive(Default)]
struct OutputFlow {
    state: Mutex<(usize, bool)>,
    changed: std::sync::Condvar,
}
impl OutputFlow {
    fn reserve(&self, bytes: usize) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        while state.0 > 0 && state.0 + bytes > 64 * 1024 && !state.1 { state = self.changed.wait(state).unwrap_or_else(|error| error.into_inner()); }
        if state.1 { return false; }
        state.0 += bytes;
        true
    }
    fn acknowledge(&self, bytes: usize) { let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner()); state.0 = state.0.saturating_sub(bytes); self.changed.notify_all(); }
    fn close(&self) { self.state.lock().unwrap_or_else(|error| error.into_inner()).1 = true; self.changed.notify_all(); }
    fn drain(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        while state.0 > 0 && !state.1 { state = self.changed.wait(state).unwrap_or_else(|error| error.into_inner()); }
    }
}

/// Each PTY writer owns its blocking I/O; callers only enqueue bounded input.
fn input_queue(mut writer: Box<dyn Write + Send>) -> std::sync::mpsc::SyncSender<Vec<u8>> {
    let (input, queued) = std::sync::mpsc::sync_channel::<Vec<u8>>(8);
    std::thread::spawn(move || {
        for bytes in queued {
            if writer.write_all(&bytes).and_then(|_| writer.flush()).is_err() { break; }
        }
    });
    input
}

#[derive(Default)]
pub struct Terminals {
    sessions: Arc<Mutex<HashMap<u32, Session>>>,
    next: AtomicU32,
}

/// Resolve the executable once; passing its full path also avoids PTY environment casing differences on Windows.
fn program_path(name: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|p| p.join(name)).find(|p| p.is_file()))
}

fn shell_command(name: &str) -> CommandBuilder {
    CommandBuilder::new(program_path(name).unwrap_or_else(|| name.into()))
}

/// Preferences > In-App Terminal > Default In-App Terminal: powershell, pwsh, cmd, gitbash, wsl (Windows);
/// default, bash, zsh, fish elsewhere.
pub fn shell_for(name: &str) -> Result<CommandBuilder, String> {
    Ok(match name {
        "powershell" if cfg!(windows) => shell_command("powershell.exe"),
        "pwsh" => shell_command(if cfg!(windows) { "pwsh.exe" } else { "pwsh" }),
        "cmd" if cfg!(windows) => shell_command("cmd.exe"),
        "wsl" if cfg!(windows) => shell_command("wsl.exe"),
        "gitbash" if cfg!(windows) => {
            #[cfg(windows)]
            let bash = crate::external::git_for_windows_program("bin/bash.exe");
            #[cfg(not(windows))]
            let bash: Option<std::path::PathBuf> = None;
            let mut c = CommandBuilder::new(bash.ok_or("Git Bash wasn't found next to the configured Git executable.")?);
            c.args(["--login", "-i"]);
            c
        }
        "bash" | "zsh" | "fish" => login_shell(name),
        _ => default_shell(),
    })
}

/// The shell to start: PowerShell on Windows, the user's shell elsewhere.
fn default_shell() -> CommandBuilder {
    if cfg!(windows) {
        match program_path("pwsh.exe") {
            Some(path) => CommandBuilder::new(path),
            None => shell_command("powershell.exe"),
        }
    } else {
        login_shell(&std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()))
    }
}

fn login_shell(path: &str) -> CommandBuilder {
    let mut shell = shell_command(path);
    if cfg!(target_os = "macos") && matches!(std::path::Path::new(path).file_name().and_then(|name| name.to_str()), Some("zsh" | "bash" | "fish")) {
        shell.arg("-l");
    }
    shell
}

/// Splits bytes into valid UTF-8 text, keeping an incomplete character for the next chunk.
pub fn take_utf8(buf: &mut Vec<u8>) -> String {
    match std::str::from_utf8(buf) {
        Ok(s) => {
            let s = s.to_string();
            buf.clear();
            s
        }
        Err(e) => {
            let good = e.valid_up_to();
            // An invalid byte (not just a cut-off character): replace it and move on.
            let cut = if e.error_len().is_some() { buf.len() } else { good };
            let s = String::from_utf8_lossy(&buf[..cut]).into_owned();
            buf.drain(..cut);
            s
        }
    }
}

impl Terminals {
    pub fn pending_output(&self) -> Vec<(u32, usize)> {
        self.sessions.lock().unwrap_or_else(|error| error.into_inner()).iter().map(|(id, session)| (*id, session.output.state.lock().unwrap_or_else(|error| error.into_inner()).0)).collect()
    }
    /// Start a shell in `cwd`. Output goes to `on_data`; `on_exit` runs when the shell ends.
    pub fn open(
        &self,
        owner: &str,
        shell: &str,
        cwd: &str,
        cols: u16,
        rows: u16,
        agent_env: Option<&HashMap<String, String>>,
        on_data: Arc<dyn Fn(u32, String) + Send + Sync>,
        on_exit: Arc<dyn Fn(u32) + Send + Sync>,
    ) -> Result<u32, String> {
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize { rows: rows.max(2), cols: cols.max(10), pixel_width: 0, pixel_height: 0 })
            .map_err(|e| e.to_string())?;
        let mut cmd = shell_for(shell)?;
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        #[cfg(target_os = "macos")]
        if std::env::var_os("LANG").is_none_or(|value| value.is_empty()) { cmd.env("LANG", "en_US.UTF-8"); }
        // Keep agent config paths out of shell command text.
        if let Some(vars) = agent_env {
            for key in ["CODEX_HOME", "CLAUDE_CONFIG_DIR"] {
                if let Some(value) = vars.get(key) {
                    let expanded = value.strip_prefix("~/").or_else(|| value.strip_prefix("~\\"))
                        .and_then(|rest| std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(|home| std::path::PathBuf::from(home).join(rest)));
                    cmd.env(key, expanded.unwrap_or_else(|| value.into()));
                }
            }
        }
        let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        let input = input_queue(writer);
        let output = Arc::new(OutputFlow::default());
        self.sessions.lock().unwrap_or_else(|error| error.into_inner()).insert(id, Session { owner: owner.into(), writer: input, master: Arc::new(Mutex::new(pair.master)), child, output: output.clone() });
        let sessions = self.sessions.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            let mut pending = Vec::new();
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        pending.extend_from_slice(&buf[..n]);
                        let text = take_utf8(&mut pending);
                        if !text.is_empty() {
                            if !output.reserve(text.len()) { break; }
                            on_data(id, text);
                        }
                    }
                }
            }
            if !pending.is_empty() {
                let text = String::from_utf8_lossy(&pending).into_owned();
                if output.reserve(text.len()) { on_data(id, text); }
            }
            output.drain();
            // EOF releases the PTY and reaps shells that exited without the user closing their tab.
            let session = sessions.lock().unwrap_or_else(|error| error.into_inner()).remove(&id);
            if let Some(mut session) = session {
                let _ = session.child.wait();
            }
            on_exit(id);
        });
        Ok(id)
    }

    fn session_value<T>(&self, owner: &str, id: u32, value: impl FnOnce(&Session) -> T) -> Option<T> {
        self.sessions.lock().unwrap_or_else(|error| error.into_inner()).get(&id)
            .filter(|session| session.owner == owner).map(value)
    }

    pub fn write(&self, owner: &str, id: u32, data: &str) -> Result<(), String> {
        if data.len() > 256 * 1024 { return Err("Paste at most 256 KB at a time into the terminal.".into()); }
        let writer = self.session_value(owner, id, |session| session.writer.clone()).ok_or("That terminal has closed")?;
        writer.try_send(data.as_bytes().to_vec()).map_err(|_| "Terminal input is busy or closed. Wait before sending more input.".into())
    }

    pub fn resize(&self, owner: &str, id: u32, cols: u16, rows: u16) -> Result<(), String> {
        let master = self.session_value(owner, id, |session| session.master.clone()).ok_or("That terminal has closed")?;
        let result = master.lock().unwrap_or_else(|error| error.into_inner()).resize(PtySize { rows: rows.max(2), cols: cols.max(10), pixel_width: 0, pixel_height: 0 }).map_err(|e| e.to_string());
        result
    }

    pub fn acknowledge(&self, owner: &str, id: u32, bytes: usize) {
        let output = self.session_value(owner, id, |session| session.output.clone());
        if let Some(output) = output { output.acknowledge(bytes); }
    }

    pub fn close(&self, owner: &str, id: u32) {
        let mut sessions = self.sessions.lock().unwrap_or_else(|error| error.into_inner());
        let removed = sessions.get(&id).is_some_and(|session| session.owner == owner)
            .then(|| sessions.remove(&id)).flatten();
        drop(sessions);
        if let Some(session) = removed { Self::stop(session); }
    }

    pub fn close_window(&self, label: &str) {
        self.close_matching(|session| session.owner == label);
    }

    pub fn close_all(&self) {
        self.close_matching(|_| true);
    }

    fn close_matching(&self, matches: impl Fn(&Session) -> bool) {
        let removed: Vec<_> = self.sessions.lock().unwrap_or_else(|error| error.into_inner())
            .extract_if(|_, session| matches(session)).map(|(_, session)| session).collect();
        for session in removed { Self::stop(session); }
    }

    fn stop(mut session: Session) {
        session.output.close();
        let _ = session.child.kill();
        std::thread::spawn(move || { let _ = session.child.wait(); });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_waits_for_renderer_acknowledgement_and_close_releases_waiters() {
        use std::time::Duration;
        let flow = Arc::new(OutputFlow::default());
        assert!(flow.reserve(64 * 1024));
        let other = flow.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || { tx.send(other.reserve(1)).unwrap(); });
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
        flow.acknowledge(64 * 1024);
        assert!(rx.recv_timeout(Duration::from_secs(1)).unwrap()); worker.join().unwrap();
        let other = flow.clone(); let worker = std::thread::spawn(move || other.drain());
        flow.close(); worker.join().unwrap(); assert!(!flow.reserve(1));
    }

    #[test]
    fn blocked_input_does_not_block_another_writer_and_queue_is_bounded() {
        use std::sync::mpsc::{channel, TrySendError};
        use std::time::{Duration, Instant};
        struct Blocked { entered: std::sync::mpsc::Sender<()>, release: std::sync::mpsc::Receiver<()> }
        impl Write for Blocked {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.entered.send(()).unwrap(); self.release.recv().unwrap(); Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
        }
        struct Responsive(std::sync::mpsc::Sender<Vec<u8>>);
        impl Write for Responsive {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> { self.0.send(bytes.to_vec()).unwrap(); Ok(bytes.len()) }
            fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
        }
        let (entered_tx, entered) = channel(); let (release, release_rx) = channel();
        let blocked = input_queue(Box::new(Blocked {entered: entered_tx, release: release_rx}));
        blocked.try_send(vec![0]).unwrap(); entered.recv_timeout(Duration::from_secs(2)).unwrap();
        for _ in 0..8 { blocked.try_send(vec![0]).unwrap(); }
        assert!(matches!(blocked.try_send(vec![0]), Err(TrySendError::Full(_))));
        let (tx, received) = channel(); let responsive = input_queue(Box::new(Responsive(tx)));
        let started = Instant::now(); responsive.try_send(b"responsive".to_vec()).unwrap();
        assert_eq!(received.recv_timeout(Duration::from_secs(2)).unwrap(), b"responsive");
        eprintln!("Independent terminal input while first writer is blocked: {:?}; eight queued packets maximum", started.elapsed());
        // Let every queued write finish before releasing the fixture.
        for _ in 0..9 { release.send(()).unwrap(); }
        drop(blocked); drop(responsive);
    }

    #[test]
    fn sustained_output_stays_bounded_and_drains_in_order() {
        let flow = Arc::new(OutputFlow::default()); let producer = flow.clone();
        let (tx, rx) = std::sync::mpsc::channel(); let started = std::time::Instant::now();
        let worker = std::thread::spawn(move || {
            for sequence in 0..2048 { assert!(producer.reserve(8192)); tx.send(sequence).unwrap(); }
            producer.drain();
        });
        let mut peak = 0;
        for expected in 0..2048 {
            assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(), expected);
            peak = peak.max(flow.state.lock().unwrap().0);
            // Deliberately slower acknowledgements model a busy renderer.
            std::thread::sleep(std::time::Duration::from_micros(50)); flow.acknowledge(8192);
        }
        worker.join().unwrap(); assert!(peak <= 64 * 1024); assert_eq!(flow.state.lock().unwrap().0, 0);
        eprintln!("16 MiB ordered output: {:?}, peak pending {peak} bytes, 2048 frames", started.elapsed());
    }

    #[test]
    fn keeps_cut_off_characters_for_the_next_chunk() {
        let mut b = "hé".as_bytes()[..2].to_vec(); // "h" + first byte of "é"
        assert_eq!(take_utf8(&mut b), "h");
        assert_eq!(b.len(), 1);
        b.extend_from_slice(&"é".as_bytes()[1..]);
        assert_eq!(take_utf8(&mut b), "é");
        assert!(b.is_empty());
    }

    #[test]
    fn closing_one_window_preserves_other_terminal_sessions() {
        let terminals = Terminals::default();
        let directory = std::env::temp_dir();
        let open = |owner| terminals.open(owner, "", &directory.to_string_lossy(), 80, 24, None,
            Arc::new(|_, _| {}), Arc::new(|_| {})).unwrap();
        let first = open("first");
        let second = open("second");
        assert!(terminals.resize("first", second, 80, 24).is_err());
        terminals.close("first", second);
        terminals.close_window("first");
        assert!(terminals.resize("first", first, 80, 24).is_err());
        assert!(terminals.resize("second", second, 80, 24).is_ok());
        terminals.close_all();
        assert!(terminals.pending_output().is_empty());
    }

    #[test]
    fn runs_a_shell_command() {
        let t = Arc::new(Terminals::default());
        let out = Arc::new(Mutex::new(String::new()));
        let o2 = out.clone();
        let ack = t.clone();
        let id = t
            .open("main", "", &std::env::temp_dir().to_string_lossy(), 80, 24, None, Arc::new(move |id, s| { o2.lock().unwrap().push_str(&s); ack.acknowledge("main", id, s.len()); }), Arc::new(|_| {}))
            .unwrap();
        // Like xterm.js does in the app: answer the console's "where is the cursor?" request.
        let mut answered = false;
        let mut sent = false;
        for _ in 0..200 {
            let got = out.lock().unwrap().clone();
            if !answered && got.contains("\x1b[6n") {
                t.write("main", id, "\x1b[1;1R").unwrap();
                answered = true;
            }
            if !sent && (answered || got.len() > 20) {
                std::thread::sleep(std::time::Duration::from_millis(300));
                t.write("main", id, "echo gitcraft-terminal-ok\r").unwrap();
                sent = true;
            }
            if got.matches("gitcraft-terminal-ok").count() >= 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        t.close("main", id);
        let got = out.lock().unwrap().clone();
        assert!(got.contains("gitcraft-terminal-ok"), "terminal output was: {:?}", got.chars().take(600).collect::<String>());
    }
}
