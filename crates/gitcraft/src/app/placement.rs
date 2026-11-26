//! Where the window opens: the size, position and maximized state of the last run, kept in
//! `window.json` beside `settings.json` by craft-window's restore store.
//!
//! Windows validates the saved geometry against the connected displays. On other platforms the
//! system keeps the window on screen. Wayland only restores size and maximized state.

use craft_window::policy::Backend;
use craft_window::restore::{Defaults, MAIN, MonitorKey, Placement, Store, place};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const SIZE: [f32; 2] = [930.0, 669.0];
pub const MIN: [f32; 2] = [760.0, 520.0];

/// Wait this long after the last change before writing the file.
const SAVE_AFTER: Duration = Duration::from_secs(2);
/// After a failed write, wait this long before the next try.
const RETRY_AFTER: Duration = Duration::from_secs(10);

struct Restore {
    store: Store,
    no_position: bool,
    changed: Option<Instant>,
    /// The earlier web-view release's window, taken over on the first frame when this build
    /// has none saved yet.
    adopt: Option<WebViewWindow>,
    #[cfg(windows)]
    startup_position: Option<([i32; 2], bool)>,
}

/// A window as tauri-plugin-window-state saved it for the web-view release: inner size and
/// outer position in physical pixels.
#[derive(Debug, PartialEq, serde::Deserialize)]
struct WebViewWindow {
    width: u32,
    height: u32,
    #[serde(default)]
    x: i32,
    #[serde(default)]
    y: i32,
    #[serde(default)]
    maximized: bool,
}

/// The web-view release's main window, from its `.window-state.json` in the system's
/// configuration folder for `dev.gitcraft.desktop`.
fn webview_window() -> Option<WebViewWindow> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(std::path::PathBuf::from)?
    } else if cfg!(target_os = "macos") {
        std::path::PathBuf::from(std::env::var_os("HOME")?).join("Library").join("Application Support")
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))?
    };
    parse_webview(&std::fs::read(base.join("dev.gitcraft.desktop").join(".window-state.json")).ok()?)
}

fn parse_webview(bytes: &[u8]) -> Option<WebViewWindow> {
    let mut windows: std::collections::HashMap<String, WebViewWindow> = serde_json::from_slice(bytes).ok()?;
    windows.remove("main").filter(|w| w.width > 0 && w.height > 0)
}

fn restore() -> &'static Mutex<Restore> {
    static R: OnceLock<Mutex<Restore>> = OnceLock::new();
    R.get_or_init(|| {
        let store = if std::env::var_os("GITCRAFT_NO_WINDOW_RESTORE").is_some() {
            Store::in_memory()
        } else {
            Store::at(&window_file())
        };
        if let Some(moved) = store.moved_aside() {
            eprintln!("The window file was damaged and was moved to {}.", moved.display());
        }
        let adopt = if std::env::var_os("GITCRAFT_NO_WINDOW_RESTORE").is_none() && store.get(MAIN).is_none() { webview_window() } else { None };
        Mutex::new(Restore {
            store,
            no_position: cfg!(target_os = "linux") && Backend::from_env() == Backend::Wayland,
            changed: None,
            adopt,
            #[cfg(windows)]
            startup_position: None,
        })
    })
}

/// craft-window's `window.json` in the settings folder, beside `settings.json`. A file left by
/// an earlier build under its old name is moved there once.
fn window_file() -> std::path::PathBuf {
    let dir = gitcraft_core::sample::data_dir();
    let file = dir.join(craft_window::restore::FILE_NAME);
    let old = dir.join("egui-window.json");
    if !file.exists() && old.exists() {
        let _ = std::fs::rename(&old, &file);
    }
    file
}

fn lock() -> std::sync::MutexGuard<'static, Restore> {
    restore().lock().unwrap_or_else(|e| e.into_inner())
}

/// Saved startup placement, fitted to the connected displays where available.
pub fn startup() -> Option<Placement> {
    let r = lock();
    let saved = r.store.get(MAIN)?;
    let d = Defaults::new(SIZE, MIN).with_no_position(r.no_position);
    let monitors = monitors();
    let fitted = place(Some(&saved), displays_for_restore(&saved, &monitors), None, &d);
    #[cfg(windows)]
    {
        let mut r = r;
        r.startup_position = fitted.position_px.map(|pos| (pos, fitted.maximized));
    }
    Some(fitted)
}

fn displays_for_restore<'a>(saved: &Placement, monitors: &'a [craft_window::restore::Monitor]) -> &'a [craft_window::restore::Monitor] {
    if let Some(pos) = saved.position_px
        && saved.monitor.as_ref().is_some_and(|key| monitors.iter().all(|m| &m.key != key))
        && monitors.iter().all(|m| {
            let strip = [saved.normal.size[0], saved.normal.size[1].min(64.0)];
            (0..2).any(|axis| {
                let start = pos[axis] as f32;
                let monitor_start = m.pos_px[axis] as f32;
                start >= monitor_start + m.size_px[axis] as f32
                    || start + strip[axis] * m.scale <= monitor_start
            })
        })
        && let Some(primary) = monitors.iter().find(|m| m.pos_px == [0, 0])
    {
        return std::slice::from_ref(primary);
    }
    monitors
}

pub fn apply(viewport: egui::ViewportBuilder, p: &Placement) -> egui::ViewportBuilder {
    // Winit converts the builder's logical position using the initial monitor, which may be
    // different from the saved monitor. Windows positions the actual window in pixels below.
    #[cfg(windows)]
    if p.position_px.is_some() {
        return viewport.with_inner_size(p.normal.size).with_maximized(false);
    }
    let mut viewport = viewport.with_inner_size(p.normal.size).with_maximized(p.maximized);
    if let Some([x, y]) = p.position_px {
        viewport = viewport.with_position([x as f32 / p.scale, y as f32 / p.scale]);
    }
    viewport
}

#[cfg(windows)]
pub fn position_window(ctx: &egui::Context, frame: &eframe::Frame) {
    use egui_winit::winit::dpi::PhysicalPosition;
    let Some(window) = frame.winit_window() else { return };
    let mut r = lock();
    if r.startup_position.is_none() { return; }
    // Eframe shows the initially hidden window after the first paint. Windows can change
    // the window's position on its first show, so apply the saved position after that.
    if window.is_visible() != Some(true) {
        ctx.request_repaint();
        return;
    }
    if let Some(([x, y], maximized)) = r.startup_position.take() {
        window.set_outer_position(PhysicalPosition::new(x, y));
        if maximized {
            window.set_maximized(true);
        }
        ctx.request_repaint();
    }
}

#[cfg(windows)]
fn monitors() -> Vec<craft_window::restore::Monitor> {
    use craft_window::restore::Monitor;
    use windows_sys::Win32::Foundation::{LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
    use windows_sys::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI, SetThreadDpiAwarenessContext};
    unsafe extern "system" fn each(monitor: HMONITOR, _: HDC, rect: *mut RECT, out: LPARAM) -> i32 {
        // SAFETY: `out` is the vector `monitors` passed below, alive for the whole enumeration; `rect` is valid.
        let (list, r) = unsafe { (&mut *(out as *mut Vec<Monitor>), &*rect) };
        let (mut x, mut y) = (96_u32, 96_u32);
        // SAFETY: a valid monitor handle and two valid out pointers.
        unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y) };
        let size = [(r.right - r.left).max(0) as u32, (r.bottom - r.top).max(0) as u32];
        let key = MonitorKey::new("", size);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let mut screen = Monitor::new(key, [r.left, r.top], size, x.max(1) as f32 / 96.0);
        // SAFETY: the handle comes from EnumDisplayMonitors and info has the required size.
        if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
            let work = info.rcWork;
            screen = screen.with_work_px([work.left, work.top], [(work.right - work.left) as u32, (work.bottom - work.top) as u32]);
        }
        list.push(screen);
        1
    }
    let mut list: Vec<Monitor> = Vec::new();
    // SAFETY: the callback only uses the live vector during this call. Restore the caller's DPI
    // context after reading the physical desktop.
    unsafe {
        let previous = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(each), &mut list as *mut _ as LPARAM);
        if !previous.is_null() {
            SetThreadDpiAwarenessContext(previous);
        }
    }
    list
}

#[cfg(not(windows))]
fn monitors() -> Vec<craft_window::restore::Monitor> {
    Vec::new()
}

/// Once per frame: note the window's size and place, and write them a moment after they stop
/// changing.
pub fn track(ctx: &egui::Context) {
    let info = ctx.input(|i| i.viewport().clone());
    let zoom = ctx.zoom_factor();
    let mut r = lock();
    #[cfg(windows)]
    if r.startup_position.is_some() { return; }
    if info.minimized == Some(true) {
        // A minimized window's geometry isn't sampled, but a save still waiting (or to be retried) must happen.
        save_when_due(&mut r);
        if r.changed.is_some() {
            ctx.request_repaint_after(SAVE_AFTER);
        }
        return;
    }
    let scale = info.native_pixels_per_point.unwrap_or(1.0);
    // First run after the web-view release: its window size, place and maximized state, now
    // that the monitor's scale is known. They are noted from the next frames on.
    if let Some(w) = r.adopt.take() {
        let px = scale * zoom;
        // Keep the legacy normal size and place in the store before maximizing, so that un-maximizing later
        // returns to them instead of to the default.
        let mut seeded = Placement::new(SIZE, scale);
        seeded.normal.size = [(w.width as f32 / scale).max(MIN[0]), (w.height as f32 / scale).max(MIN[1])];
        seeded.position_px = if r.no_position { None } else { Some([w.x, w.y]) };
        seeded.normal.pos = seeded.position_px.map_or([0.0, 0.0], |[x, y]| [x as f32 / scale, y as f32 / scale]);
        seeded.scale = scale;
        seeded.zoom = zoom;
        seeded.maximized = w.maximized;
        r.store.set(MAIN, &seeded);
        r.changed = Some(Instant::now());
        let size = egui::vec2((w.width as f32 / px).max(MIN[0]), (w.height as f32 / px).max(MIN[1]));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        if !r.no_position {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(w.x as f32 / px, w.y as f32 / px)));
        }
        if w.maximized {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
        }
        ctx.request_repaint();
        return;
    }
    let maximized = info.maximized.unwrap_or(false);
    let fullscreen = info.fullscreen.unwrap_or(false);
    let mut p = r.store.get(MAIN).unwrap_or_else(|| Placement::new(SIZE, scale));
    let before = p.clone();
    // egui gives places and sizes in interface points: times the zoom they are the system's
    // points, and times the native scale too, physical pixels.
    let px = scale * zoom;
    if !maximized && !fullscreen {
        // The whole client area (GitCraft draws its own title bar and frame in it). Where the
        // system doesn't say where the window is (Wayland), its size still comes from the
        // area egui lays out in.
        let size = info.inner_rect.map(|inner| inner.size()).unwrap_or_else(|| ctx.content_rect().size());
        p.normal.size = [size.x * zoom, size.y * zoom];
        p.position_px = if r.no_position {
            None
        } else {
            info.outer_rect.map(|o| [(o.min.x * px).round() as i32, (o.min.y * px).round() as i32])
        };
        p.normal.pos = p.position_px.map_or([0.0, 0.0], |[x, y]| [x as f32 / scale, y as f32 / scale]);
        p.scale = scale;
    }
    p.zoom = zoom;
    p.maximized = maximized;
    p.fullscreen = fullscreen;
    if let Some(size) = info.monitor_size {
        p.monitor = Some(MonitorKey::new("", [(size.x * px).round() as u32, (size.y * px).round() as u32]));
    }
    if p != before && p.normal.size.iter().all(|v| v.is_finite() && *v > 0.0) {
        r.store.set(MAIN, &p);
        r.changed = Some(Instant::now());
    }
    save_when_due(&mut r);
    if r.changed.is_some() {
        ctx.request_repaint_after(SAVE_AFTER);
    }
}

fn save_when_due(r: &mut Restore) {
    if r.changed.is_some_and(|t| t.elapsed() >= SAVE_AFTER) {
        save_now(r);
    }
}

fn save_now(r: &mut Restore) {
    match r.store.save() {
        Ok(_) => r.changed = None,
        Err(e) => {
            eprintln!("Couldn't save the window's size and place: {e}");
            // Try again a little later rather than every frame.
            r.changed = Instant::now().checked_add(RETRY_AFTER - SAVE_AFTER);
        }
    }
}

/// When the program ends: write what hasn't been written yet.
pub fn save() {
    let mut r = lock();
    if r.store.is_dirty() {
        save_now(&mut r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_offscreen_display_falls_back_to_the_primary() {
        use craft_window::restore::Monitor;
        let displays = [
            Monitor::new(MonitorKey::new("", [1920, 1080]), [-1920, 500], [1920, 1080], 1.5),
            Monitor::new(MonitorKey::new("", [2560, 1440]), [0, 0], [2560, 1440], 1.0),
        ];
        let mut saved = Placement::new(SIZE, 1.5);
        saved.position_px = Some([-10_000, 600]);
        saved.monitor = Some(MonitorKey::new("", [1234, 765]));
        let defaults = Defaults::new(SIZE, MIN);
        let result = place(Some(&saved), displays_for_restore(&saved, &displays), None, &defaults);
        assert_eq!(result.monitor, Some(displays[1].key.clone()));
        assert!(result.position_px.unwrap()[0] >= 0);

        saved.position_px = Some([-1600, 540]);
        let result = place(Some(&saved), displays_for_restore(&saved, &displays), None, &defaults);
        assert_eq!(result.monitor, Some(displays[0].key.clone()));
        assert_eq!(result.position_px, saved.position_px);
    }

    #[test]
    fn the_web_view_window_state_is_read() {
        let file = br#"{"main":{"width":1395,"height":1003,"x":120,"y":-8,"prev_x":0,"prev_y":0,"maximized":true,"visible":true,"decorated":false,"fullscreen":false},"other":{"width":1,"height":1}}"#;
        assert_eq!(parse_webview(file), Some(WebViewWindow { width: 1395, height: 1003, x: 120, y: -8, maximized: true }));
        assert_eq!(parse_webview(br#"{"main":{"width":0,"height":0}}"#), None);
        assert_eq!(parse_webview(b"{}"), None);
        assert_eq!(parse_webview(b"damaged"), None);
    }
}
