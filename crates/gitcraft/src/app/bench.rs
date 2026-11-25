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
    /// Wheel events sent so far.
    injected: usize,
    /// Set once the last wheel event is sent: the next frame only reports its CPU time.
    tail: bool,
    tail_seen: bool,
    done: bool,
}

/// Note when a step of the launch happened (`mark <step> <unix ms>`), with the probe on.
pub fn mark(step: &str) {
    let Some(file) = std::env::var_os("GITCRAFT_BENCH") else { return };
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
    // The process's own CPU time (with the children it has waited for) at this moment, so a runner needn't
    // catch the mark in time to measure the phase between two marks.
    let cpu = cpu_ticks().map_or_else(String::new, |t| format!(" cpu={t}"));
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(file) {
        let _ = writeln!(f, "mark {step} {ms}{cpu}");
    }
}

/// User and system CPU time of this process and the children it waited for, in clock ticks (Linux).
fn cpu_ticks() -> Option<u64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    (11..=14).map(|i| fields.get(i)?.parse::<u64>().ok()).sum()
}

/// The frame count in `GITCRAFT_BENCH_SCROLL`, when it is a positive number.
fn scroll_frames() -> Option<usize> {
    std::env::var("GITCRAFT_BENCH_SCROLL").ok()?.trim().parse().ok().filter(|n| *n > 0)
}

/// The vsync-free mode the scroll benchmark draws in.
pub fn scrolling() -> bool {
    std::env::var_os("GITCRAFT_BENCH").is_some() && scroll_frames().is_some()
}

impl Bench {
    pub fn from_env() -> Option<Bench> {
        let file = std::env::var_os("GITCRAFT_BENCH")?.into();
        let scroll = scroll_frames().unwrap_or(0);
        Some(Bench { file, ready: false, scroll, settled: 0, last: None, frames: Vec::new(), ui: Vec::new(), intervals: Vec::new(), down: true, injected: 0, tail: false, tail_seen: false, done: false })
    }

    fn write(&self, line: &str) {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.file) {
            let _ = writeln!(f, "{line}");
        }
    }

    fn scrolling_now(&self) -> bool {
        self.ready && self.scroll > 0 && self.settled >= SETTLE && !self.done
    }

    fn injecting(&self) -> bool {
        self.scrolling_now() && self.injected < self.scroll
    }

    /// Wheel events over the graph while the scroll test runs.
    pub fn input(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if !self.injecting() {
            return;
        }
        // The baseline comes before the first wheel event, so the first interval is a wheel-driven frame's.
        if self.last.is_none() {
            mark("scroll-start");
            self.last = Some(Instant::now());
        }
        self.injected += 1;
        if self.injected >= self.scroll {
            self.tail = true;
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
        let Some(last) = self.last else { return };
        // The frame after the last wheel event only reports its predecessor's CPU time.
        if self.intervals.len() < self.scroll {
            self.intervals.push((now - last).as_secs_f64() * 1000.0);
            self.ui.push((now - built).as_secs_f64() * 1000.0);
        }
        // `cpu` is the previous frame's, so the first one (a frame before any wheel event) isn't counted.
        if let Some(cpu) = cpu.filter(|_| self.intervals.len() > 1 || self.tail_seen) {
            self.frames.push(f64::from(cpu) * 1000.0);
        }
        self.last = Some(now);
        if self.tail && self.intervals.len() >= self.scroll && self.injected >= self.scroll {
            // The tail frame is the one after the last wheel event, so the last wheel frame's CPU time is in.
            if self.tail_seen {
                self.done = true;
                let (f, u, i) = (stats(&mut self.frames), stats(&mut self.ui), stats(&mut self.intervals));
                self.write(&format!("scroll frames={} mean={:.3} p50={:.3} p95={:.3} max={:.3}", self.frames.len(), f.0, f.1, f.2, f.3));
                self.write(&format!("ui mean={:.3} p95={:.3}", u.0, u.2));
                self.write(&format!("interval mean={:.3}", i.0));
                // The marker is the last signal, after the whole report.
                mark("scroll-end");
            }
            self.tail_seen = true;
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
