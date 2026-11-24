//! A benchmark probe, only when `GITCRAFT_BENCH` names a file (see TESTING.md):
//!
//! - `ready <unix ms>` is written on the first frame that draws a repository's commits, so a
//!   runner that notes when it started GitCraft can time the launch.
//! - With `GITCRAFT_BENCH_SCROLL=<frames>`, the graph is then scrolled by wheel events for that
//!   many frames, drawn without waiting for the display, and the frame times are written as
//!   `scroll frames=… mean=… p50=… p95=… max=…` (ms of CPU per frame: building, tessellating and
//!   painting, without waiting for the display), `ui mean=… p95=…` (ms, building only) and
//!   `interval mean=…` (ms between frames, which the compositor may pace), between
//!   `mark scroll-start` and `mark scroll-end`. GitCraft stays open for the runner to measure and
//!   close.

use std::io::Write;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Frames left alone after the graph appears, before scrolling, for pages and pictures to load.
const SETTLE: usize = 60;

pub struct Bench {
    file: std::path::PathBuf,
    ready: bool,
    scroll: usize,
    settled: usize,
    last: Option<Instant>,
    frames: Vec<f64>,
    ui: Vec<f64>,
    intervals: Vec<f64>,
    /// Scrolling down until the end, then back up.
    down: bool,
    done: bool,
}

/// Note when a step of the launch happened (`mark <step> <unix ms>`), with the probe on.
pub fn mark(step: &str) {
    let Some(file) = std::env::var_os("GITCRAFT_BENCH") else { return };
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(file) {
        let _ = writeln!(f, "mark {step} {ms}");
    }
}

/// The vsync-free mode the scroll benchmark draws in.
pub fn scrolling() -> bool {
    std::env::var_os("GITCRAFT_BENCH").is_some() && std::env::var_os("GITCRAFT_BENCH_SCROLL").is_some()
}

impl Bench {
    pub fn from_env() -> Option<Bench> {
        let file = std::env::var_os("GITCRAFT_BENCH")?.into();
        let scroll = std::env::var("GITCRAFT_BENCH_SCROLL").ok().and_then(|n| n.parse().ok()).unwrap_or(0);
        Some(Bench { file, ready: false, scroll, settled: 0, last: None, frames: Vec::new(), ui: Vec::new(), intervals: Vec::new(), down: true, done: false })
    }

    fn write(&self, line: &str) {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.file) {
            let _ = writeln!(f, "{line}");
        }
    }

    fn scrolling_now(&self) -> bool {
        self.ready && self.scroll > 0 && self.settled >= SETTLE && !self.done
    }

    /// Wheel events over the graph while the scroll test runs.
    pub fn input(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if !self.scrolling_now() {
            return;
        }
        let rect = ctx.content_rect();
        // The graph is in the middle, between the side panels.
        let at = egui::pos2(rect.left() + rect.width() * 0.42, rect.center().y);
        if self.intervals.len() % 400 == 399 {
            self.down = !self.down;
        }
        let dy = if self.down { -120.0 } else { 120.0 };
        raw.events.push(egui::Event::PointerMoved(at));
        raw.events.push(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, dy), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Unknown, modifiers: Default::default() });
    }

    /// After a frame was built: `has_graph` says whether it drew commits, `built` when it started.
    /// `cpu` is eframe's CPU time of the previous whole frame.
    pub fn frame(&mut self, ctx: &egui::Context, has_graph: bool, built: Instant, cpu: Option<f32>) {
        if self.done {
            return;
        }
        if !self.ready {
            if has_graph {
                self.ready = true;
                let ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
                self.write(&format!("ready {ms}"));
            }
            if self.scroll > 0 {
                ctx.request_repaint();
            }
            return;
        }
        if self.scroll == 0 {
            return;
        }
        ctx.request_repaint();
        if self.settled < SETTLE {
            self.settled += 1;
            return;
        }
        let now = Instant::now();
        if self.last.is_none() {
            mark("scroll-start");
        }
        if let Some(last) = self.last {
            self.intervals.push((now - last).as_secs_f64() * 1000.0);
            self.ui.push((now - built).as_secs_f64() * 1000.0);
            if let Some(cpu) = cpu {
                self.frames.push(f64::from(cpu) * 1000.0);
            }
        }
        self.last = Some(now);
        if self.intervals.len() >= self.scroll {
            self.done = true;
            mark("scroll-end");
            let (f, u, i) = (stats(&mut self.frames), stats(&mut self.ui), stats(&mut self.intervals));
            self.write(&format!("scroll frames={} mean={:.3} p50={:.3} p95={:.3} max={:.3}", self.frames.len(), f.0, f.1, f.2, f.3));
            self.write(&format!("ui mean={:.3} p95={:.3}", u.0, u.2));
            self.write(&format!("interval mean={:.3}", i.0));
        }
    }
}

/// Mean, median, 95th percentile and maximum.
fn stats(v: &mut [f64]) -> (f64, f64, f64, f64) {
    if v.is_empty() {
        return (0.0, 0.0, 0.0, 0.0);
    }
    v.sort_by(f64::total_cmp);
    let at = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
    (v.iter().sum::<f64>() / v.len() as f64, at(0.5), at(0.95), v[v.len() - 1])
}
