//! Undo and redo caret moves without touching the text buffer's undo history.

use super::FileRevision;
use egui::text::CCursorRange;
use egui::{Id, Vec2};
use std::collections::VecDeque;

const MAX_STATES: usize = 50;

#[derive(Clone, Copy)]
pub(super) enum Direction {
    Undo,
    Redo,
}

pub(super) struct Selection {
    pub primary: CCursorRange,
    pub secondary: Vec<CCursorRange>,
    pub scroll: Vec2,
}

#[derive(Default)]
pub(super) struct CursorHistory {
    revision: Option<(Id, FileRevision)>,
    previous: Option<Selection>,
    undo: VecDeque<Selection>,
    redo: Vec<Selection>,
    pub request: Option<Direction>,
}

impl CursorHistory {
    pub fn update_revision(&mut self, scope: Id, revision: FileRevision) {
        let key = (scope, revision);
        if self.revision != Some(key) {
            *self = Self {
                revision: Some(key),
                ..Self::default()
            };
        }
    }

    pub fn enabled(&self, action: Direction) -> bool {
        match action {
            Direction::Undo => !self.undo.is_empty(),
            Direction::Redo => !self.redo.is_empty(),
        }
    }

    pub fn restore(&mut self, action: Direction) -> Option<Selection> {
        let next = match action {
            Direction::Undo => self.undo.pop_back(),
            Direction::Redo => self.redo.pop(),
        }?;
        if let Some(previous) = self.previous.take() {
            match action {
                Direction::Undo => self.redo.push(previous),
                Direction::Redo => self.undo.push_back(previous),
            }
        }
        Some(next)
    }

    pub fn observe(
        &mut self,
        primary: CCursorRange,
        secondary: &[CCursorRange],
        scroll: Vec2,
        restored: bool,
    ) {
        if let Some(previous) = self.previous.as_mut()
            && previous.primary == primary
            && previous.secondary == secondary
        {
            previous.scroll = scroll;
            return;
        }
        let next = Selection {
            primary,
            secondary: secondary.to_vec(),
            scroll,
        };
        if let Some(previous) = self.previous.replace(next)
            && !restored
        {
            if self.undo.len() == MAX_STATES {
                self.undo.pop_front();
            }
            self.undo.push_back(previous);
            self.redo.clear();
        }
    }
}
