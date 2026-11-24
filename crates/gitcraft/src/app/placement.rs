//! Where the window opens: the size, position and maximized state of the last run, kept in
//! `window.json` beside `settings.json` by craft-window's restore store.
//!
//! The monitors aren't known before the window exists, so the saved geometry is applied as it is
//! (raised to the smallest size) and the system keeps the window on screen. Wayland doesn't tell
//! windows where they are, so there only the size and maximized state come back.

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
        Mutex::new(Restore { store, no_position: Backend::from_env() == Backend::Wayland, changed: None, adopt })
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

/// The window as it was last time, on a builder that already has the default size.
pub fn apply(viewport: egui::ViewportBuilder) -> egui::ViewportBuilder {
    let r = lock();
    let Some(saved) = r.store.get(MAIN) else { return viewport };
    let d = Defaults::new(SIZE, MIN).with_no_position(r.no_position);
    let p = place(Some(&saved), &[], None, &d);
    let mut viewport = viewport.with_inner_size(p.normal.size).with_maximized(p.maximized);
    if let Some([x, y]) = p.position_px {
        viewport = viewport.with_position([x as f32 / p.scale, y as f32 / p.scale]);
    }
    viewport
}

/// Once per frame: note the window's size and place, and write them a moment after they stop
/// changing.
pub fn track(ctx: &egui::Context) {
    let info = ctx.input(|i| i.viewport().clone());
    let zoom = ctx.zoom_factor();
    let mut r = lock();
    if info.minimized == Some(true) {
        return save_when_due(&mut r);
    }
    let scale = info.native_pixels_per_point.unwrap_or(1.0);
    // First run after the web-view release: its window size, place and maximized state, now
    // that the monitor's scale is known. They are noted from the next frames on.
    if let Some(w) = r.adopt.take() {
        let px = scale * zoom;
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
    fn the_web_view_window_state_is_read() {
        let file = br#"{"main":{"width":1395,"height":1003,"x":120,"y":-8,"prev_x":0,"prev_y":0,"maximized":true,"visible":true,"decorated":false,"fullscreen":false},"other":{"width":1,"height":1}}"#;
        assert_eq!(parse_webview(file), Some(WebViewWindow { width: 1395, height: 1003, x: 120, y: -8, maximized: true }));
        assert_eq!(parse_webview(br#"{"main":{"width":0,"height":0}}"#), None);
        assert_eq!(parse_webview(b"{}"), None);
        assert_eq!(parse_webview(b"damaged"), None);
    }
}
