//! Author pictures for the graph, the commit details and hosted content: fetched from Gravatar
//! once per address, at most a few at a time, and kept as textures. An address without a
//! picture is asked again after a minute (the network may have been away); until then, and
//! when *Display author initials* is on, callers draw the initials.

use egui::{Color32, CornerRadius, Rect, TextureHandle, TextureOptions, pos2};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Pixels asked for: sharp at the largest size shown on a 2× screen.
const PX: u32 = 64;
const IN_FLIGHT: usize = 4;
const RETRY_AFTER: Duration = Duration::from_secs(60);
const KEEP: usize = 1024;

enum State {
    Loading,
    Ready(TextureHandle),
    Missing(Instant),
}

static ON: AtomicBool = AtomicBool::new(false);
static RUNNING: AtomicUsize = AtomicUsize::new(0);

fn cache() -> &'static Mutex<HashMap<String, State>> {
    static C: OnceLock<Mutex<HashMap<String, State>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Once per frame: whether pictures are wanted (off when the person chose initials).
pub fn enable(on: bool) {
    ON.store(on, Ordering::Relaxed);
}

/// The author's picture when it has arrived; starts fetching it otherwise.
pub fn texture(ctx: &egui::Context, email: &str) -> Option<TextureHandle> {
    if !ON.load(Ordering::Relaxed) {
        return None;
    }
    let key = email.trim().to_lowercase();
    gitcraft_core::avatars::url(&key, PX)?;
    let mut map = cache().lock().unwrap_or_else(|e| e.into_inner());
    match map.get(&key) {
        Some(State::Ready(t)) => return Some(t.clone()),
        Some(State::Loading) => return None,
        Some(State::Missing(at)) if at.elapsed() < RETRY_AFTER => return None,
        _ => {}
    }
    if RUNNING.load(Ordering::Relaxed) >= IN_FLIGHT {
        // Asked again on a later frame.
        ctx.request_repaint_after(Duration::from_millis(200));
        return None;
    }
    if map.len() >= KEEP {
        map.retain(|_, s| matches!(s, State::Ready(_)));
    }
    map.insert(key.clone(), State::Loading);
    RUNNING.fetch_add(1, Ordering::Relaxed);
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let picture = gitcraft_core::avatars::fetch(&key, PX).ok().flatten().and_then(|bytes| decode(&bytes));
        let state = match picture {
            Some(image) => State::Ready(ctx.load_texture(format!("avatar:{key}"), image, TextureOptions::LINEAR)),
            None => State::Missing(Instant::now()),
        };
        cache().lock().unwrap_or_else(|e| e.into_inner()).insert(key, state);
        RUNNING.fetch_sub(1, Ordering::Relaxed);
        ctx.request_repaint();
    });
    None
}

fn decode(bytes: &[u8]) -> Option<egui::ColorImage> {
    let image = image::load_from_memory(bytes).ok()?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()))
}

/// Paint a picture as a circle of `diameter` around `center`, faded by `alpha`.
pub fn paint(painter: &egui::Painter, texture: &TextureHandle, center: egui::Pos2, diameter: f32, alpha: f32) {
    let rect = Rect::from_center_size(center, egui::Vec2::splat(diameter));
    let radius = (diameter / 2.0).round().clamp(0.0, 255.0) as u8;
    let shape = egui::epaint::RectShape::filled(rect, CornerRadius::same(radius), Color32::WHITE.gamma_multiply(alpha))
        .with_texture(texture.id(), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)));
    painter.add(shape);
}
