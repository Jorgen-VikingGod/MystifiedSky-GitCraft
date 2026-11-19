//! A cancellable process group on Unix and a kill-on-close job on Windows.
use std::{collections::HashSet, io, process::{Child, Command}, sync::{Mutex, OnceLock}};

#[derive(Default)]
struct ProcessRegistry {
    active: HashSet<usize>,
    stopping: bool,
}

fn registry() -> &'static Mutex<ProcessRegistry> {
    static REGISTRY: OnceLock<Mutex<ProcessRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}

pub fn shutdown() {
    let mut registry = registry().lock().unwrap_or_else(|error| error.into_inner());
    registry.stopping = true;
    for &id in &registry.active { kill_tree(id); }
}

pub enum StartError {
    Spawn(io::Error),
    Tree(String),
    Shutdown,
}

impl std::fmt::Display for StartError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(error) => write!(formatter, "{error}"),
            Self::Tree(error) => formatter.write_str(error),
            Self::Shutdown => formatter.write_str("GitCraft is closing."),
        }
    }
}

pub struct ProcessTree { id: usize }
impl ProcessTree {
    pub fn spawn(command: &mut Command) -> Result<(Child, Self), StartError> {
        // Hold the registry through spawn and attachment so shutdown cannot miss a new group.
        let mut registry = registry().lock().unwrap_or_else(|error| error.into_inner());
        if registry.stopping { return Err(StartError::Shutdown); }
        #[cfg(unix)] {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn().map_err(StartError::Spawn)?;
        match Self::attach(&child) {
            Ok(tree) => {
                registry.active.insert(tree.id);
                Ok((child, tree))
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(StartError::Tree(error))
            }
        }
    }

    fn attach(child: &Child) -> Result<Self, String> {
        #[cfg(windows)] unsafe {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::{Foundation::CloseHandle, System::JobObjects::*};
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() { return Err(std::io::Error::last_os_error().to_string()); }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(job, JobObjectExtendedLimitInformation, &info as *const _ as _, std::mem::size_of_val(&info) as u32) == 0 || AssignProcessToJobObject(job, child.as_raw_handle() as _) == 0 {
                let error = std::io::Error::last_os_error().to_string();
                CloseHandle(job);
                return Err(error);
            }
            Ok(Self { id: job as usize })
        }
        #[cfg(not(windows))] { Ok(Self { id: child.id() as usize }) }
    }
    pub fn kill(&self) -> bool { kill_tree(self.id) }
}

fn kill_tree(id: usize) -> bool {
    #[cfg(windows)] unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(id as _, 1) != 0 }
    #[cfg(unix)] unsafe {
        extern "C" { fn kill(pid: i32, signal: i32) -> i32; }
        kill(-(id as i32), 9) == 0
    }
    #[cfg(not(any(windows, unix)))] { let _ = id; false }
}
impl Drop for ProcessTree {
    fn drop(&mut self) {
        let mut registry = registry().lock().unwrap_or_else(|error| error.into_inner());
        registry.active.remove(&self.id);
        #[cfg(windows)] unsafe { windows_sys::Win32::Foundation::CloseHandle(self.id as _); }
        #[cfg(unix)] { self.kill(); }
    }
}
