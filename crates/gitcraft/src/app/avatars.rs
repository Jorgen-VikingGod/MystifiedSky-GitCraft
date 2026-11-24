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
    /// The picture, and when it was last drawn.
    Ready(TextureHandle, Instant),
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
    match map.get_mut(&key) {
        Some(State::Ready(t, used)) => {
            *used = Instant::now();
            return Some(t.clone());
        }
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
        prune(&mut map);
    }
    map.insert(key.clone(), State::Loading);
    RUNNING.fetch_add(1, Ordering::Relaxed);
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let picture = gitcraft_core::avatars::fetch(&key, PX).ok().flatten().and_then(|bytes| decode(&bytes));
        let state = match picture {
            Some(image) => State::Ready(ctx.load_texture(format!("avatar:{key}"), image, TextureOptions::LINEAR), Instant::now()),
            None => State::Missing(Instant::now()),
        };
        cache().lock().unwrap_or_else(|e| e.into_inner()).insert(key, state);
        RUNNING.fetch_sub(1, Ordering::Relaxed);
        ctx.request_repaint();
    });
    None
}

/// The cache is full: forget addresses whose wait before asking again is over, then the pictures
/// drawn longest ago, a quarter of the cache at a time so this runs rarely. Requests still
/// running, and addresses still waiting, stay.
fn prune(map: &mut HashMap<String, State>) {
    map.retain(|_, s| !matches!(s, State::Missing(at) if at.elapsed() >= RETRY_AFTER));
    if map.len() < KEEP {
        return;
    }
    let mut ready: Vec<(Instant, String)> = map.iter().filter_map(|(k, s)| if let State::Ready(_, used) = s { Some((*used, k.clone())) } else { None }).collect();
    ready.sort_unstable();
    for (_, key) in ready.into_iter().take((KEEP / 4).max(1)) {
        map.remove(&key);
    }
}

/// The largest avatar decoded, per side; avatars are drawn a few dozen pixels across.
const MAX_SIDE: u32 = 1024;

/// A picture from its bytes. Its size is read from the header first, and a picture larger than
/// [`MAX_SIDE`] either way (a small file can claim a huge size) is never decoded, so its pixels
/// are never allocated; the decoder's own allocation limit backs this up.
fn decode(bytes: &[u8]) -> Option<egui::ColorImage> {
    let reader = || {
        let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(MAX_SIDE);
        limits.max_image_height = Some(MAX_SIDE);
        limits.max_alloc = Some(u64::from(MAX_SIDE) * u64::from(MAX_SIDE) * 4 * 2);
        reader.limits(limits);
        Some(reader)
    };
    let (w, h) = reader()?.into_dimensions().ok()?;
    if w > MAX_SIDE || h > MAX_SIDE {
        return None;
    }
    let image = reader()?.decode().ok()?.to_rgba8();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        image::RgbaImage::new(w, h).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn oversized_pictures_are_not_decoded() {
        assert_eq!(decode(&png(64, 64)).map(|i| i.size), Some([64, 64]));
        assert!(decode(&png(MAX_SIDE + 1, 1)).is_none());
        // A header claiming 60000 × 60000 pixels: refused before any pixels are allocated.
        let mut huge = png(1, 1);
        huge[16..20].copy_from_slice(&60000u32.to_be_bytes());
        huge[20..24].copy_from_slice(&60000u32.to_be_bytes());
        assert!(decode(&huge).is_none());
        assert!(decode(b"not a picture").is_none());
    }

    #[test]
    fn a_full_cache_drops_old_pictures_and_keeps_requests_and_waits() {
        let ctx = egui::Context::default();
        let picture = egui::ColorImage::new([1, 1], vec![Color32::WHITE]);
        let start = Instant::now();
        let mut map: HashMap<String, State> = HashMap::new();
        for i in 0..KEEP {
            let texture = ctx.load_texture(format!("t{i}"), picture.clone(), TextureOptions::LINEAR);
            map.insert(format!("r{i}"), State::Ready(texture, start + Duration::from_millis(i as u64)));
        }
        map.insert("loading".into(), State::Loading);
        map.insert("waiting".into(), State::Missing(Instant::now()));
        if let Some(long_ago) = Instant::now().checked_sub(RETRY_AFTER * 2) {
            map.insert("expired".into(), State::Missing(long_ago));
        }
        prune(&mut map);
        assert!(map.contains_key("loading") && map.contains_key("waiting"));
        assert!(!map.contains_key("expired"));
        // The pictures drawn longest ago went, the latest stayed.
        assert!(!map.contains_key("r0"));
        assert!(map.contains_key(&format!("r{}", KEEP - 1)));
        assert!(map.len() < KEEP);
    }
}
