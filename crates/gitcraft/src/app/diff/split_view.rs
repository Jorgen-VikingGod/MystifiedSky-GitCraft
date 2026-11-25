//! Shared scroll coordinates and the split comparison overview ruler.

use super::{ACCENT, BAD, GOOD, p};
use crate::app::textdiff::SplitIndex;
use egui::{Id, Key, Modifiers, Rect, Sense, Stroke, StrokeKind, Ui, pos2};
use std::sync::Arc;

const RULER_WIDTH: f32 = 22.0;

pub(super) fn reserve_ruler(mut body: Rect) -> (Rect, Rect) {
    let width = (body.width() * 0.12).clamp(0.0, RULER_WIDTH);
    let ruler = Rect::from_min_max(pos2(body.right() - width, body.top()), body.max);
    body.max.x = ruler.left();
    (body, ruler)
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Scroll {
    pub top: f32,
    pub viewport: f32,
    pub total: f32,
}

impl Scroll {
    pub fn from_output<R>(output: &egui::scroll_area::ScrollAreaOutput<R>) -> Self {
        Self {
            top: output.state.clamped_offset().y,
            viewport: output.inner_rect.height(),
            total: output.content_size.y,
        }
    }

    pub fn clamp(self, top: f32) -> f32 {
        top.clamp(0.0, (self.total - self.viewport).max(0.0))
    }
}

#[derive(Clone)]
pub(super) enum LineTops {
    Fixed(f32),
    Consecutive(Arc<Vec<f32>>),
    Editor(Arc<Vec<(usize, f32)>>),
}

#[derive(Clone)]
pub(super) struct Pane {
    pub scroll: Scroll,
    pub lines: LineTops,
    pub inset: f32,
}

impl Pane {
    pub fn anchor(&self, y: f32) -> (usize, f32) {
        let y = (y - self.inset).max(0.0);
        let end = (self.scroll.total - self.inset).max(0.0);
        let (line, start, next) = match &self.lines {
            LineTops::Fixed(height) => {
                let line = (y / height).floor() as usize;
                (line, line as f32 * height, (line + 1) as f32 * height)
            }
            LineTops::Consecutive(tops) if tops.len() > 1 => {
                let line = tops
                    .partition_point(|top| *top <= y)
                    .saturating_sub(1)
                    .min(tops.len() - 2);
                (line, tops[line], tops[line + 1])
            }
            LineTops::Editor(tops) if !tops.is_empty() => {
                let at = tops.partition_point(|(_, top)| *top <= y).saturating_sub(1);
                let (line, start) = tops[at];
                (line, start, tops.get(at + 1).map_or(end, |(_, top)| *top))
            }
            _ => return (0, 0.0),
        };
        let within = (y - start) / (next - start).max(1.0);
        (line, within.clamp(0.0, 1.0))
    }

    pub fn offset(&self, line: usize, within: f32) -> f32 {
        let end = (self.scroll.total - self.inset).max(0.0);
        let (start, next, within) = match &self.lines {
            LineTops::Fixed(height) => (line as f32 * height, (line + 1) as f32 * height, within),
            LineTops::Consecutive(tops) if tops.len() > 1 => {
                let Some(&next) = tops.get(line.saturating_add(1)) else {
                    return self.scroll.total;
                };
                (tops[line], next, within)
            }
            LineTops::Editor(tops) if !tops.is_empty() => {
                let at = tops
                    .partition_point(|(visible, _)| *visible <= line)
                    .saturating_sub(1);
                let (visible, top) = tops[at];
                let next = tops.get(at + 1).map_or(end, |(_, top)| *top);
                (top, next, if line == visible { within } else { 0.0 })
            }
            _ => return 0.0,
        };
        self.inset + start + (next - start).max(0.0) * within
    }
}

pub(super) fn diff_position(
    index: &SplitIndex,
    side: usize,
    anchor: (usize, f32),
    rows: usize,
) -> f32 {
    mapped_position(index, side, anchor, rows).unwrap_or(rows as f32)
}

/// Where `anchor` is in the comparison: the end of the document is at its last row, but a line the comparison
/// doesn't cover (the text changed since it was made) can't be placed, so it is `None`.
pub(super) fn mapped_position(
    index: &SplitIndex,
    side: usize,
    anchor: (usize, f32),
    rows: usize,
) -> Option<f32> {
    let (line, within) = anchor;
    match index.lines[side].row(line) {
        Some(row) => Some(row as f32 + within),
        None if line == index.lines[side].lines() => Some(rows as f32),
        None => None,
    }
}

pub(super) fn pane_offset(index: &SplitIndex, side: usize, position: f32, pane: &Pane) -> f32 {
    if position <= 0.0 {
        return 0.0;
    }
    let row = position.floor() as usize;
    let Some((line, exact)) = index.lines[side].line_at_or_after(row) else {
        return pane.scroll.total;
    };
    pane.offset(line, if exact { position.fract() } else { 0.0 })
}

#[derive(Clone, Default)]
pub(super) struct Sync {
    previous: Option<[f32; 2]>,
    pending: [Option<f32>; 2],
    leader: usize,
    layout_key: Option<Id>,
}

impl Sync {
    #[cfg(test)]
    pub fn positions(&self) -> [f32; 2] {
        self.previous.unwrap_or_default()
    }

    pub fn input(&self) -> [Option<f32>; 2] {
        self.pending
    }

    /// Propagate only real movement. A follower hitting a shorter pane's scroll limit must
    /// never send that clamped offset back to the pane the user is scrolling.
    pub fn update(
        &mut self,
        panes: [&Pane; 2],
        layout: Id,
        preferred: usize,
        map: impl Fn(usize, usize, f32) -> f32,
    ) -> bool {
        let previous = self.previous.unwrap_or([0.0; 2]);
        let moved = [0, 1].map(|side| {
            let scroll = panes[side].scroll;
            let expected = self.pending[side].unwrap_or(previous[side]);
            (scroll.top - scroll.clamp(expected)).abs() > 0.75
        });
        // The reference moves only when a pane really moved or was set by a correction, so slow movement adds up
        // against the last synchronized position instead of being forgotten frame by frame.
        let mut reference = previous;
        for side in 0..2 {
            if moved[side] || self.pending[side].is_some() {
                reference[side] = panes[side].scroll.top;
            }
        }
        self.previous = Some(reference);
        self.pending = [None; 2];
        let reflow = self.layout_key.replace(layout) != Some(layout);
        if moved[preferred] {
            self.leader = preferred;
        } else if moved[1 - preferred] {
            self.leader = 1 - preferred;
        } else if !reflow {
            return false;
        }
        let from = self.leader;
        let to = 1 - from;
        let follower = panes[to].scroll;
        let target = follower.clamp(map(from, to, panes[from].scroll.top));
        if (target - follower.top).abs() > 0.75 {
            self.pending[to] = Some(target);
            true
        } else {
            false
        }
    }

    pub fn leader(&self, panes: [&Pane; 2]) -> usize {
        if panes[self.leader].scroll.clamp(f32::MAX) > 0.0 {
            self.leader
        } else {
            1 - self.leader
        }
    }

    pub fn jump(&mut self, offsets: [f32; 2], leader: usize) {
        self.pending = offsets.map(Some);
        self.leader = leader;
    }
}

/// The left and right lanes mark removals and additions. Coordinates are fractions of the
/// shared comparison timeline, so the ruler remains fixed while either text pane scrolls.
pub(super) fn ruler(
    ui: &Ui,
    id: Id,
    rect: Rect,
    index: &SplitIndex,
    viewport: (f32, f32),
    geometry: Id,
    fraction_at: impl Fn(usize) -> f32,
) -> Option<f32> {
    if rect.width() < 4.0 || rect.height() < 4.0 {
        return None;
    }
    let colors = p();
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 0, colors.field);
    painter.vline(rect.left(), rect.y_range(), Stroke::new(1.0, colors.line));
    let middle = rect.center().x;
    let y = |fraction: f32| rect.top() + fraction.clamp(0.0, 1.0) * rect.height();
    type Marks = Arc<[Vec<(f32, f32)>; 2]>;
    let marks_id = id.with("overview_marks");
    let geometry = geometry.with(rect.height().to_bits());
    let previous = ui.data_mut(|data| data.get_temp::<(Id, Marks)>(marks_id));
    let marks = previous
        .filter(|(key, _)| *key == geometry)
        .map(|(_, marks)| marks)
        .unwrap_or_else(|| {
            let marks = Arc::new(index.marks.each_ref().map(|ranges| {
                let mut marks: Vec<(f32, f32)> = Vec::new();
                for rows in ranges {
                    let at = |row| fraction_at(row).clamp(0.0, 1.0) * rect.height();
                    let start = at(rows.start).min(rect.height() - 2.0);
                    let end = at(rows.end).max(start + 2.0).min(rect.height());
                    if let Some(last) = marks.last_mut().filter(|last| start <= last.1) {
                        last.1 = last.1.max(end);
                    } else {
                        marks.push((start, end));
                    }
                }
                marks
            }));
            ui.data_mut(|data| data.insert_temp(marks_id, (geometry, marks.clone())));
            marks
        });
    for (side, (ink, left, right)) in [
        (BAD, rect.left() + 1.0, middle),
        (GOOD, middle, rect.right() - 1.0),
    ]
    .into_iter()
    .enumerate()
    {
        for &(start, end) in &marks[side] {
            let top = rect.top();
            painter.rect_filled(
                Rect::from_min_max(pos2(left, top + start), pos2(right, top + end)),
                0,
                ink,
            );
        }
    }
    let response = ui.interact(rect, id.with("overview_ruler"), Sense::click_and_drag());
    if response.clicked() || response.drag_started() {
        response.request_focus();
    }
    response.widget_info(|| {
        egui::WidgetInfo::slider(
            ui.is_enabled(),
            f64::from(viewport.0) * 100.0,
            "Diff overview",
        )
    });
    let thumb_top = y(viewport.0).min(rect.bottom() - 4.0);
    let thumb = Rect::from_min_max(
        pos2(rect.left() + 1.0, thumb_top),
        pos2(rect.right(), y(viewport.1).max(thumb_top + 4.0)),
    );
    painter.rect_filled(thumb, 1.0, colors.fg.gamma_multiply(0.14));
    painter.rect_stroke(
        thumb,
        1.0,
        Stroke::new(
            1.0,
            if response.has_focus() {
                ACCENT
            } else {
                colors.fg.gamma_multiply(0.42)
            },
        ),
        StrokeKind::Inside,
    );
    let page = (viewport.1 - viewport.0).max(0.0);
    let grip_id = id.with("overview_grip");
    if response.is_pointer_button_down_on()
        && let Some(pointer) = response.interact_pointer_pos()
        && ui.input(|input| input.pointer.primary_pressed())
    {
        let grab = if thumb.contains(pointer) {
            (pointer.y - thumb_top) / rect.height()
        } else {
            page / 2.0
        };
        ui.data_mut(|data| data.insert_temp(grip_id, grab));
    }
    if (response.clicked() || response.dragged())
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let grab = ui
            .data_mut(|data| data.get_temp::<f32>(grip_id))
            .unwrap_or(page / 2.0);
        return Some(((pointer.y - rect.top()) / rect.height() - grab).clamp(0.0, 1.0));
    }
    if response.hovered() {
        let wheel = ui.input(|input| input.smooth_scroll_delta().y);
        if wheel != 0.0 {
            return Some((viewport.0 - wheel / rect.height() * page).clamp(0.0, 1.0));
        }
    }
    if response.has_focus() {
        return ui.input_mut(|input| {
            let mut consume = |key| input.consume_key(Modifiers::NONE, key);
            if consume(Key::Home) {
                Some(0.0)
            } else if consume(Key::End) {
                Some(1.0)
            } else if consume(Key::PageUp) {
                Some((viewport.0 - page).max(0.0))
            } else if consume(Key::PageDown) {
                Some((viewport.0 + page).min(1.0))
            } else if consume(Key::ArrowUp) {
                Some((viewport.0 - page / 12.0).max(0.0))
            } else if consume(Key::ArrowDown) {
                Some((viewport.0 + page / 12.0).min(1.0))
            } else {
                None
            }
        });
    }
    None
}

#[cfg(test)]
#[test]
fn headless_split_scroll_keeps_driver_at_an_unmatched_or_clamped_end() {
    use crate::app::textdiff::compare;
    let comparison = compare("a\nb\nc\nd\n", "a\nx\nb\nc\nd\n", false);
    let index = &comparison.split_index;
    let mut old = Pane {
        scroll: Scroll {
            top: 0.0,
            viewport: 20.0,
            total: 70.0,
        },
        lines: LineTops::Consecutive(Arc::new(vec![0.0, 10.0, 50.0, 60.0, 70.0])),
        inset: 0.0,
    };
    let mut new = Pane {
        scroll: Scroll {
            top: 0.0,
            viewport: 40.0,
            total: 50.0,
        },
        lines: LineTops::Fixed(10.0),
        inset: 0.0,
    };
    assert!(index.marks[0].is_empty());
    assert_eq!(index.marks[1].as_slice(), std::slice::from_ref(&(1..2)));
    assert_eq!(pane_offset(index, 0, 1.8, &old), 10.0);
    assert_eq!(pane_offset(index, 0, 2.5, &old), 30.0);
    let id = Id::unique("split");
    let mut sync = Sync::default();
    assert!(!sync.update([&old, &new], id, 0, |_, _, y| y));
    old.scroll.top = 30.0;
    let map = |from, to, y| {
        let panes = [&old, &new];
        pane_offset(
            index,
            to,
            diff_position(index, from, panes[from].anchor(y), comparison.split.len()),
            panes[to],
        )
    };
    assert!(sync.update([&old, &new], id, 0, map));
    assert_eq!(sync.input(), [None, Some(10.0)]);
    new.scroll.top = 10.0;
    assert!(
        !sync.update([&old, &new], id, 1, |_, _, y| y),
        "clamped follower must not take over"
    );
    assert_eq!(sync.positions(), [30.0, 10.0]);
}
