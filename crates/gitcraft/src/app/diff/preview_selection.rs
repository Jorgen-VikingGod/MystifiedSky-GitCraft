//! Selection in the virtual file and comparison panes. Only visible rows are laid out.

use super::{FileRevision, clipboard};
use crate::app::highlight::Runs;
use egui::epaint::text::CharIndex;
use egui::{Color32, Galley, Id, Key, Modifiers, Pos2, Rect, Response, Sense, Ui};
use std::ops::Range;

#[derive(Clone, Copy)]
struct Selection {
    revision: FileRevision,
    start: usize,
    end: usize,
    side: usize,
}

impl Selection {
    fn range(self) -> Range<usize> {
        self.start.min(self.end)..self.start.max(self.end)
    }
}

pub(super) struct PreviewSelection<'a> {
    state: Selection,
    sources: [&'a str; 2],
    response: Response,
    pointer: Option<Pos2>,
    nearest: Option<(f32, usize, usize)>,
    extend: bool,
    pressed: bool,
}

impl<'a> PreviewSelection<'a> {
    pub fn new(
        ui: &Ui,
        id: Id,
        rect: Rect,
        revision: FileRevision,
        sources: [&'a str; 2],
        side: usize,
    ) -> Self {
        let response = ui
            .interact(rect.intersect(ui.clip_rect()), id, Sense::click_and_drag())
            .on_hover_cursor(egui::CursorIcon::Text);
        let state = ui
            .data_mut(|data| data.get_temp::<Selection>(id))
            .filter(|state| state.revision == revision)
            .unwrap_or(Selection {
                revision,
                start: 0,
                end: 0,
                side,
            });
        let (pointer, pressed, extend) = ui.input(|input| {
            let held = response.is_pointer_button_down_on() || response.drag_stopped();
            let pressed = held && input.pointer.primary_pressed();
            (
                held.then(|| input.pointer.interact_pos()).flatten(),
                pressed,
                input.modifiers.shift,
            )
        });
        if pressed {
            response.request_focus();
        }
        Self {
            state,
            sources,
            response,
            pointer,
            nearest: None,
            extend,
            pressed,
        }
    }

    pub fn prefers_original(&self) -> bool {
        self.state.side == 0 && (!self.pressed || self.extend)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn line(
        &mut self,
        painter: &egui::Painter,
        rect: Rect,
        glyphs: &Galley,
        pos: Pos2,
        text: &str,
        offset: usize,
        side: usize,
        color: Color32,
    ) {
        if let Some(pointer) = self.pointer
            && (side == self.state.side || self.pressed && !self.extend)
        {
            let delta = if rect.y_range().contains(pointer.y) {
                0.0
            } else {
                (pointer.y - rect.top())
                    .abs()
                    .min((pointer.y - rect.bottom()).abs())
            };
            if self.nearest.is_none_or(|(distance, _, _)| delta < distance) {
                let char_index = glyphs.cursor_from_pos(pointer - pos).index.0;
                let byte = text
                    .char_indices()
                    .nth(char_index)
                    .map_or(text.len(), |(n, _)| n);
                self.nearest = Some((delta, offset + byte, side));
            }
        }
        if side != self.state.side {
            return;
        }
        let selected = self.state.range();
        let start = selected.start.saturating_sub(offset).min(text.len());
        let end = selected.end.saturating_sub(offset).min(text.len());
        if start >= end {
            return;
        }
        let start = text[..start].chars().count();
        let end = text[..end].chars().count();
        let color = color.gamma_multiply(0.27);
        let mut offset = 0;
        for row in &glyphs.rows {
            let count = row.char_count_excluding_newline().0;
            let a = start.saturating_sub(offset).min(count);
            let b = end.saturating_sub(offset).min(count);
            if a < b {
                for x in row.x_ranges(CharIndex(a)..CharIndex(b)) {
                    let min = egui::pos2(pos.x + row.pos.x + x.min, pos.y + row.min_y());
                    let max = egui::pos2(pos.x + row.pos.x + x.max, pos.y + row.max_y());
                    painter.rect_filled(Rect::from_min_max(min, max), 0, color);
                }
            }
            offset += row.char_count_including_newline().0;
        }
    }

    pub fn finish(
        mut self,
        ui: &Ui,
        syntax: [&[Runs]; 2],
        ink: Color32,
        context: impl FnOnce(&mut Ui),
    ) {
        let before = self.state.range();
        if let Some((_, byte, side)) = self.nearest {
            if self.pressed && !self.extend {
                self.state.start = byte;
                self.state.side = side;
            }
            self.state.end = byte;
        }
        let source = self.sources[self.state.side];
        let syntax = syntax[self.state.side];
        if let Some(pointer) = self
            .pointer
            .filter(|_| self.response.dragged() && self.response.rect.height() > 0.0)
        {
            let overshoot = pointer.y
                - pointer
                    .y
                    .clamp(self.response.rect.top(), self.response.rect.bottom());
            if overshoot != 0.0 {
                ui.scroll_with_delta(egui::vec2(0.0, (-overshoot * 0.3).clamp(-30.0, 30.0)));
                ui.ctx().request_repaint();
            }
        }
        let mut copied = None;
        if self.response.has_focus() {
            // Select All and Copy are applied in the order they arrived: Copy then Select All copies the earlier
            // selection, Select All then Copy copies the whole file.
            let escape = ui.input_mut(|input| {
                let mut kept = Vec::with_capacity(input.events.len());
                for event in std::mem::take(&mut input.events) {
                    match &event {
                        egui::Event::Key { key: Key::A, modifiers, pressed: true, .. } if modifiers.matches_logically(Modifiers::COMMAND) => {
                            self.state.start = 0;
                            self.state.end = source.len();
                            continue;
                        }
                        egui::Event::Copy => {
                            let range = self.state.range();
                            if !range.is_empty() {
                                copied = Some(range);
                                continue;
                            }
                        }
                        _ => {}
                    }
                    kept.push(event);
                }
                input.events = kept;
                input.consume_key(Modifiers::NONE, Key::Escape)
            });
            if escape {
                self.state.start = self.state.end;
                self.response.surrender_focus();
            }
        }
        let range = self.state.range();
        if let Some(copied) = copied {
            clipboard::copy(ui, source, std::slice::from_ref(&copied), &[], ink);
        }
        self.response.context_menu(|ui| {
            if ui
                .add_enabled(!range.is_empty(), egui::Button::new("Copy"))
                .clicked()
            {
                clipboard::copy(ui, source, std::slice::from_ref(&range), &[], ink);
                ui.close();
            }
            if ui
                .add_enabled(
                    !range.is_empty() && !syntax.is_empty(),
                    egui::Button::new("Copy With Syntax Highlighting"),
                )
                .clicked()
            {
                clipboard::copy(ui, source, std::slice::from_ref(&range), syntax, ink);
                ui.close();
            }
            if ui
                .add_enabled(!source.is_empty(), egui::Button::new("Select All"))
                .clicked()
            {
                self.state.start = 0;
                self.state.end = source.len();
                self.response.request_focus();
                ui.close();
            }
            context(ui);
        });
        if self.state.range() != before {
            ui.ctx().request_repaint();
        }
        ui.data_mut(|data| data.insert_temp(self.response.id, self.state));
    }
}
