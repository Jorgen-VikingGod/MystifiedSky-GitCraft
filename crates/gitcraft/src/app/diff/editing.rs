use super::{EditorPrefs, clipboard, highlight};
use egui::epaint::text::{CharIndex, grapheme};
use egui::text::{CCursor, CCursorRange};
use egui::{Color32, Event, Id, Key, Modifiers, Stroke, Ui};
use std::borrow::Cow;
use std::cell::Cell;
use std::collections::BTreeSet;
use std::ops::Range;
use std::sync::{Arc, OnceLock};

const MAX_CURSORS: usize = 1000;

#[derive(Default)]
pub struct Cursors {
    others: Vec<CCursorRange>,
    pointer_origin: Option<CCursorRange>,
    last_galley: Option<Arc<egui::Galley>>,
}

impl Cursors {
    pub fn secondary(&self) -> &[CCursorRange] {
        &self.others
    }

    pub fn restore_secondary(&mut self, others: Vec<CCursorRange>) {
        self.others = others;
        self.pointer_origin = None;
    }

    pub fn multiple(&self) -> bool {
        !self.others.is_empty()
    }

    pub fn clear(&mut self) {
        self.others.clear();
        self.pointer_origin = None;
    }

    fn selections(&self, primary: CCursorRange) -> impl Iterator<Item = CCursorRange> + '_ {
        std::iter::once(primary).chain(self.others.iter().copied())
    }

    fn overlaps(&self, primary: CCursorRange, next: CCursorRange) -> bool {
        self.selections(primary)
            .any(|old| overlaps(&range(old), &range(next)))
    }

    pub fn pointer(
        &mut self,
        ui: &Ui,
        out: &mut egui::text_edit::TextEditOutput,
        before: Option<CCursorRange>,
    ) {
        let alt = ui.input(|input| input.modifiers.alt);
        if alt
            && ui.input(|input| input.pointer.primary_pressed())
            && out.response.contains_pointer()
        {
            self.pointer_origin = before;
        }
        if !out.response.clicked() && !out.response.drag_started() {
            return;
        }
        if alt && !out.response.clicked() {
            return;
        }
        let click = out.response.interact_pointer_pos();
        if let (true, Some(primary), Some(pos)) =
            (alt, self.pointer_origin.take().or(before), click)
        {
            let cursor = CCursorRange::one(out.galley.cursor_from_pos(pos - out.galley_pos));
            if let Some(at) = self
                .others
                .iter()
                .position(|old| overlaps(&range(*old), &range(cursor)))
            {
                self.others.remove(at);
            } else if self.others.len() + 1 < MAX_CURSORS && !self.overlaps(primary, cursor) {
                self.others.push(cursor);
            }
            out.state.cursor.set_char_range(Some(primary));
            out.state.clone().store(ui.ctx(), out.response.id);
        } else {
            self.clear();
        }
    }

    pub fn paint(
        &mut self,
        ui: &Ui,
        out: &egui::text_edit::TextEditOutput,
        color: Color32,
        positions: Option<&super::folding::Positions>,
        visible_rows: Range<usize>,
        visible_chars: Range<usize>,
    ) {
        if positions.is_none() {
            self.last_galley = Some(out.galley.clone());
        }
        if !out.response.has_focus() {
            return;
        }
        if visible_rows.is_empty() {
            return;
        }
        let painter = ui.painter().with_clip_rect(out.text_clip_rect);
        for selection in &self.others {
            let selection = positions.map_or(*selection, |positions| positions.in_view(*selection));
            let [from, to] = selection.sorted_cursors();
            if to.index.0 < visible_chars.start || from.index.0 >= visible_chars.end {
                continue;
            }
            if from != to {
                let first = out.galley.layout_from_cursor(from);
                let last = out.galley.layout_from_cursor(to);
                for i in first.row.max(visible_rows.start)..(last.row + 1).min(visible_rows.end) {
                    let row = &out.galley.rows[i];
                    let left = if i == first.row {
                        first.column
                    } else {
                        CharIndex::ZERO
                    };
                    let right = if i == last.row {
                        last.column
                    } else {
                        row.char_count_excluding_newline()
                    };
                    let pos = out.galley_pos + row.pos.to_vec2();
                    for x in row.x_ranges(left..right) {
                        let rect = egui::Rect::from_min_max(
                            pos + egui::vec2(x.min, 0.0),
                            pos + egui::vec2(x.max, row.height()),
                        );
                        painter.rect_filled(rect, 0, color.gamma_multiply(0.26));
                    }
                }
            }
            if visible_chars.contains(&selection.primary.index.0) {
                let rect = out
                    .galley
                    .pos_from_cursor(selection.primary)
                    .translate(out.galley_pos.to_vec2());
                painter.line_segment(
                    [rect.left_top(), rect.left_bottom()],
                    Stroke::new(1.0, color),
                );
            }
        }
    }
}

fn range(selection: CCursorRange) -> Range<usize> {
    let chars = selection.as_sorted_char_range();
    chars.start.0..chars.end.0
}

fn selection(chars: Range<usize>) -> CCursorRange {
    CCursorRange::two(CCursor::new(chars.start), CCursor::new(chars.end))
}

fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    if a.is_empty() {
        b.start <= a.start && a.start <= b.end
    } else if b.is_empty() {
        a.start <= b.start && b.start <= a.end
    } else {
        a.start < b.end && b.start < a.end
    }
}

struct Index<'a> {
    text: &'a str,
    // ASCII needs no translation. For Unicode, store one byte offset per block of characters.
    checkpoints: Vec<usize>,
}

impl<'a> Index<'a> {
    const BLOCK: usize = 1024;

    fn new(text: &'a str) -> Self {
        Self {
            text,
            checkpoints: if text.is_ascii() {
                Vec::new()
            } else {
                text.char_indices()
                    .step_by(Self::BLOCK)
                    .map(|(byte, _)| byte)
                    .collect()
            },
        }
    }

    fn byte(&self, character: usize) -> usize {
        if self.checkpoints.is_empty() {
            return character.min(self.text.len());
        }
        let Some(&start) = self.checkpoints.get(character / Self::BLOCK) else {
            return self.text.len();
        };
        self.text[start..]
            .char_indices()
            .nth(character % Self::BLOCK)
            .map_or(self.text.len(), |(offset, _)| start + offset)
    }

    fn character(&self, byte: usize) -> usize {
        let byte = byte.min(self.text.len());
        if self.checkpoints.is_empty() {
            return byte;
        }
        let block = self.checkpoints.partition_point(|&offset| offset <= byte) - 1;
        block * Self::BLOCK + self.text[self.checkpoints[block]..byte].chars().count()
    }

    fn bytes(&self, chars: Range<usize>) -> Range<usize> {
        self.byte(chars.start)..self.byte(chars.end)
    }

    fn selection(&self, bytes: Range<usize>) -> CCursorRange {
        selection(self.character(bytes.start)..self.character(bytes.end))
    }
}

/// The layout counts both characters in CRLF. Keep edit positions outside that boundary.
fn snap_crlf(text: &str, index: CharIndex, byte: usize) -> CharIndex {
    if byte > 0 && text.as_bytes().get(byte - 1..=byte) == Some(b"\r\n") {
        CharIndex(index.0 - 1)
    } else {
        index
    }
}

impl Index<'_> {
    fn snap_cursor(&self, text: &str, mut cursor: CCursorRange) -> CCursorRange {
        for point in [&mut cursor.primary, &mut cursor.secondary] {
            point.index = snap_crlf(text, point.index, self.byte(point.index.0));
        }
        cursor
    }
}

pub fn document_cursor(text: &str, mut cursor: CCursorRange) -> CCursorRange {
    for point in [&mut cursor.primary, &mut cursor.secondary] {
        let byte = egui::TextBuffer::byte_index_from_char_index(&text, point.index);
        point.index = snap_crlf(text, point.index, byte.0);
    }
    cursor
}

pub struct DocumentBuffer<'a, 'doc> {
    pub text: &'a mut Cow<'doc, str>,
    pub crlf: bool,
    pub mutations: &'a Cell<u64>,
}

impl egui::TextBuffer for DocumentBuffer<'_, '_> {
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<DocumentBuffer<'static, 'static>>()
    }

    fn is_mutable(&self) -> bool {
        true
    }
    fn as_str(&self) -> &str {
        self.text.as_ref()
    }

    fn insert_text(&mut self, added: &str, at: CharIndex) -> usize {
        if added.is_empty() {
            return 0;
        }
        let byte = self.boundary(at);
        let inserted = inserted_text(added, self.crlf);
        self.text.to_mut().insert_str(byte, &inserted);
        self.mutations.set(self.mutations.get() + 1);
        inserted.chars().count()
    }

    fn delete_char_range(&mut self, range: Range<CharIndex>) {
        let range = self.boundary(range.start)..self.boundary(range.end);
        if !range.is_empty() {
            self.text.to_mut().replace_range(range, "");
            self.mutations.set(self.mutations.get() + 1);
        }
    }

    // Undo and redo supply the original document, including any mixed line endings.
    fn replace_with(&mut self, text: &str) {
        self.text.to_mut().clear();
        self.text.to_mut().push_str(text);
        self.mutations.set(self.mutations.get() + 1);
    }
}

impl DocumentBuffer<'_, '_> {
    fn boundary(&self, at: CharIndex) -> usize {
        let byte = egui::TextBuffer::byte_index_from_char_index(self.text, at).0;
        byte - (at.0 - snap_crlf(self.text, at, byte).0)
    }
}

#[cfg(test)]
#[test]
fn headless_editor_buffer_never_splits_crlf() {
    use egui::TextBuffer;
    let mut text = Cow::Borrowed("é\r\nnext");
    let mutations = Cell::new(0);
    let mut buffer = DocumentBuffer {
        text: &mut text,
        crlf: true,
        mutations: &mutations,
    };
    assert_eq!(buffer.insert_text("字", CharIndex(2)), 1);
    assert_eq!(buffer.as_str(), "é字\r\nnext");
    buffer.delete_char_range(CharIndex(2)..CharIndex(3));
    assert_eq!(buffer.as_str(), "é字\r\nnext");
    buffer.delete_char_range(CharIndex(2)..CharIndex(4));
    assert_eq!(buffer.as_str(), "é字next");
    assert_eq!(buffer.insert_text("\nπ\r\n", CharIndex(2)), 5);
    assert_eq!(buffer.as_str(), "é字\r\nπ\r\nnext");
    buffer.replace_with("one\ntwo\r\n");
    assert_eq!(buffer.as_str(), "one\ntwo\r\n");
}

fn inserted_text(text: &str, crlf: bool) -> Cow<'_, str> {
    if crlf {
        let mut prior = b'\r';
        let bare_lf = text.bytes().any(|byte| {
            let bare = byte == b'\n' && prior != b'\r';
            prior = byte;
            bare
        }) || text.starts_with('\n');
        if bare_lf {
            return Cow::Owned(text.replace("\r\n", "\n").replace('\n', "\r\n"));
        }
    } else if text.contains("\r\n") {
        return Cow::Owned(text.replace("\r\n", "\n"));
    }
    Cow::Borrowed(text)
}

pub fn shortcut(key: Key, mods: Modifiers) -> bool {
    (key == Key::D && mods.matches_exact(Modifiers::COMMAND))
        || (key == Key::L && mods.matches_exact(Modifiers::COMMAND | Modifiers::SHIFT))
        || (matches!(key, Key::ArrowUp | Key::ArrowDown)
            && mods.matches_exact(Modifiers::COMMAND | Modifiers::ALT))
        || (key == Key::Slash && mods.matches_exact(Modifiers::COMMAND))
        || (key == Key::Backslash && mods.matches_exact(Modifiers::COMMAND | Modifiers::SHIFT))
        || line_command(key, mods).is_some()
}

pub fn navigation(key: Key) -> bool {
    matches!(
        key,
        Key::ArrowLeft
            | Key::ArrowRight
            | Key::ArrowUp
            | Key::ArrowDown
            | Key::Home
            | Key::End
            | Key::PageUp
            | Key::PageDown
    )
}

#[derive(Clone, Copy)]
enum LineCommand {
    Move(bool),
    Duplicate(bool),
    Delete,
}

fn line_command(key: Key, mods: Modifiers) -> Option<LineCommand> {
    if key == Key::K && mods.matches_exact(Modifiers::COMMAND | Modifiers::SHIFT) {
        return Some(LineCommand::Delete);
    }
    let up = match key {
        Key::ArrowUp => true,
        Key::ArrowDown => false,
        _ => return None,
    };
    if mods.matches_exact(Modifiers::ALT | Modifiers::SHIFT) {
        Some(LineCommand::Duplicate(up))
    } else if mods.matches_exact(Modifiers::ALT) {
        Some(LineCommand::Move(up))
    } else {
        None
    }
}

fn line_edit(
    text: &str,
    cursor: CCursorRange,
    command: LineCommand,
    crlf: bool,
) -> Option<(String, CCursorRange)> {
    let index = Index::new(text);
    let chars = range(cursor);
    let mut lines = Vec::new();
    let mut starts = Vec::new();
    let mut offset = 0;
    for raw in text
        .split_inclusive('\n')
        .chain((text.is_empty() || text.ends_with('\n')).then_some(""))
    {
        let body = raw.trim_end_matches('\n');
        let body = if raw.ends_with('\n') {
            body.strip_suffix('\r').unwrap_or(body)
        } else {
            body
        };
        starts.push(index.character(offset));
        lines.push((body, &raw[body.len()..]));
        offset += raw.len();
    }
    let row_at = |at| {
        starts
            .partition_point(|&start| start <= at)
            .saturating_sub(1)
    };
    let first = row_at(chars.start);
    let last = row_at(if chars.is_empty() {
        chars.end
    } else {
        chars.end - 1
    });
    let end = last + 1;
    let len = end - first;
    let destination = match command {
        LineCommand::Move(true) if first > 0 => {
            lines[first - 1..end].rotate_left(1);
            first - 1
        }
        LineCommand::Move(false) if end < lines.len() => {
            lines[first..end + 1].rotate_right(1);
            first + 1
        }
        LineCommand::Move(_) => return None,
        LineCommand::Duplicate(up) => {
            let block = lines[first..end].to_vec();
            let destination = if up { first } else { end };
            lines.splice(destination..destination, block);
            destination
        }
        LineCommand::Delete => {
            lines.drain(first..end);
            first.min(lines.len().saturating_sub(1))
        }
    };
    let mut output = String::with_capacity(text.len());
    let mut new_starts = Vec::with_capacity(lines.len());
    let mut position = 0;
    for (i, (body, source_ending)) in lines.iter().enumerate() {
        new_starts.push(position);
        let eol = if i + 1 == lines.len() {
            ""
        } else if !source_ending.is_empty() {
            source_ending
        } else if crlf {
            "\r\n"
        } else {
            "\n"
        };
        output.push_str(body);
        output.push_str(eol);
        position += body.chars().count() + eol.len();
    }
    if matches!(command, LineCommand::Delete) {
        let column = chars.start - starts[first];
        let at = new_starts.get(destination).map_or(0, |&start| {
            start + column.min(lines[destination].0.chars().count())
        });
        return Some((output, CCursorRange::one(CCursor::new(at))));
    }
    let move_cursor = |point: CCursor| {
        let pos = point.index.0;
        let row = row_at(pos);
        let new_row = destination + (row - first).min(len);
        let column = pos - starts[row];
        let at = new_starts
            .get(new_row)
            .map_or(position, |&start| start + column);
        CCursor::new(at.min(position))
    };
    let moved = CCursorRange {
        primary: move_cursor(cursor.primary),
        secondary: move_cursor(cursor.secondary),
        ..cursor
    };
    Some((output, moved))
}

fn comment_marker(file: &str) -> Option<&'static str> {
    let ext = file.rsplit('.').next()?.to_ascii_lowercase();
    match ext.as_str() {
        "rs" | "c" | "h" | "cpp" | "cc" | "cs" | "dart" | "go" | "java" | "js" | "jsx" | "kt"
        | "swift" | "ts" | "tsx" => Some("//"),
        "py" | "rb" | "sh" | "bash" | "zsh" | "yaml" | "yml" | "toml" | "ps1" | "dockerfile"
        | "makefile" => Some("#"),
        "lua" | "sql" | "hs" => Some("--"),
        "ini" => Some(";"),
        _ => None,
    }
}

fn word_at(text: &str, byte: usize) -> Option<Range<usize>> {
    static WORD: OnceLock<regex::Regex> = OnceLock::new();
    let word = WORD.get_or_init(|| regex::Regex::new(r"[\p{L}\p{M}\p{N}_]+").unwrap());
    let start = text[..byte].rfind('\n').map_or(0, |pos| pos + 1);
    let end = text[byte..].find('\n').map_or(text.len(), |pos| byte + pos);
    word.find_iter(&text[start..end])
        .find(|m| (start + m.start()..=start + m.end()).contains(&byte))
        .map(|m| start + m.start()..start + m.end())
}

fn occurrences(text: &str, primary: &mut CCursorRange, cursors: &mut Cursors, all: bool) {
    let index = Index::new(text);
    let mut bytes = index.bytes(range(*primary));
    if bytes.is_empty() {
        let Some(word) = word_at(text, bytes.start) else {
            return;
        };
        bytes = word;
        *primary = index.selection(bytes.clone());
        if !all {
            return;
        }
    }
    let needle = &text[bytes.clone()];
    if all {
        cursors.clear();
        for (start, _) in text.match_indices(needle) {
            let next = index.selection(start..start + needle.len());
            if !overlaps(&range(*primary), &range(next)) {
                cursors.others.push(next);
                if cursors.others.len() + 1 == MAX_CURSORS {
                    break;
                }
            }
        }
    } else if cursors.others.len() + 1 < MAX_CURSORS {
        let found = text[bytes.end..]
            .match_indices(needle)
            .map(|(at, _)| at + bytes.end)
            .chain(text[..bytes.start].match_indices(needle).map(|(at, _)| at))
            .map(|start| index.selection(start..start + needle.len()))
            .find(|&next| !cursors.overlaps(*primary, next));
        if let Some(next) = found {
            cursors.others.push(*primary);
            *primary = next;
        }
    }
}

fn adjacent_line(text: &str, primary: CCursorRange, up: bool) -> Option<CCursorRange> {
    let index = Index::new(text);
    let pos = index.byte(primary.primary.index.0);
    let start = text[..pos].rfind('\n').map_or(0, |at| at + 1);
    let (from, to) = if up {
        let end = start.checked_sub(1)?;
        (text[..end].rfind('\n').map_or(0, |n| n + 1), end)
    } else {
        let start = pos + text[pos..].find('\n')? + 1;
        (
            start,
            text[start..].find('\n').map_or(text.len(), |n| start + n),
        )
    };
    let end = to - usize::from(to > from && text.as_bytes()[to - 1] == b'\r');
    let column = index.character(pos) - index.character(start);
    let char_pos = (index.character(from) + column).min(index.character(end));
    Some(CCursorRange::one(CCursor::new(char_pos)))
}

#[derive(Clone, Copy)]
enum Change<'a> {
    Insert(&'a str),
    Enter,
    Tab,
    Backspace(bool),
    Delete(bool),
    Cut,
}

fn replacement(
    text: &str,
    index: &Index,
    chars: Range<usize>,
    change: Change<'_>,
    prefs: &EditorPrefs,
) -> (Range<usize>, String) {
    let start = index.byte(chars.start);
    let before = &text[..start];
    let line = before.rsplit('\n').next().unwrap_or_default();
    match change {
        Change::Insert(s) => (chars, inserted_text(s, prefs.crlf).into_owned()),
        Change::Enter => {
            let indent: String = line
                .chars()
                .take_while(|&c| c == ' ' || c == '\t')
                .collect();
            let mut inserted = if prefs.crlf {
                "\r\n".to_owned()
            } else {
                "\n".to_owned()
            };
            inserted.push_str(&indent);
            if line.trim_end().ends_with(['{', '[', '(']) {
                inserted.extend(std::iter::repeat_n(' ', prefs.tab));
            }
            (chars, inserted)
        }
        Change::Tab => {
            let column = line.chars().count();
            (chars, " ".repeat(prefs.tab - column % prefs.tab))
        }
        Change::Backspace(true) if chars.is_empty() => (
            egui::text_selection::text_cursor_state::ccursor_previous_word(
                text,
                CCursor::new(chars.start),
            )
            .index
            .0..chars.end,
            String::new(),
        ),
        Change::Delete(true) if chars.is_empty() => (
            chars.start
                ..egui::text_selection::text_cursor_state::ccursor_next_word(
                    text,
                    CCursor::new(chars.end),
                )
                .index
                .0,
            String::new(),
        ),
        Change::Backspace(false) if chars.is_empty() => (
            grapheme::backspace_start(text, CharIndex(chars.start)).0..chars.end,
            String::new(),
        ),
        Change::Delete(false) if chars.is_empty() => (
            chars.start..grapheme::next_boundary(text, CharIndex(chars.end)).0,
            String::new(),
        ),
        _ => (chars, String::new()),
    }
}

fn apply(
    text: &str,
    primary: CCursorRange,
    cursors: &Cursors,
    change: Change<'_>,
    prefs: &EditorPrefs,
) -> (String, CCursorRange, Vec<CCursorRange>) {
    let index = Index::new(text);
    let mut edits: Vec<_> = cursors
        .selections(primary)
        .enumerate()
        .map(|(i, cursor)| {
            let (chars, insert) = replacement(text, &index, range(cursor), change, prefs);
            (chars, i, insert)
        })
        .collect();
    edits.sort_unstable_by_key(|(r, i, _)| (r.start, r.end, *i));
    let mut merged: Vec<(Range<usize>, Vec<usize>, String)> = Vec::new();
    for (chars, at, insert) in edits {
        if let Some((previous, indices, _)) = merged.last_mut()
            && (chars.start < previous.end || chars == *previous)
        {
            previous.end = previous.end.max(chars.end);
            indices.push(at);
        } else {
            merged.push((chars, vec![at], insert));
        }
    }
    let mut result = String::with_capacity(text.len());
    let mut previous = 0;
    let mut char_pos = 0;
    let mut positions = vec![0; cursors.others.len() + 1];
    for (chars, indices, insert) in merged {
        result.push_str(&text[index.byte(previous)..index.byte(chars.start)]);
        result.push_str(&insert);
        char_pos += chars.start - previous + insert.chars().count();
        for i in indices {
            positions[i] = char_pos;
        }
        previous = chars.end;
    }
    result.push_str(&text[index.byte(previous)..]);
    let at = |pos| CCursorRange::one(CCursor::new(pos));
    let primary = at(positions[0]);
    let mut other: Vec<_> = positions[1..]
        .iter()
        .copied()
        .filter(|&pos| pos != positions[0])
        .collect();
    other.sort_unstable();
    other.dedup();
    (result, primary, other.into_iter().map(at).collect())
}

enum LinePrefix {
    Indent,
    Unindent,
    Comment(&'static str),
}

fn prefix_lines(
    text: &str,
    primary: CCursorRange,
    cursors: &Cursors,
    command: LinePrefix,
    tab: usize,
) -> (String, CCursorRange, Vec<CCursorRange>) {
    let index = Index::new(text);
    let mut lines = BTreeSet::new();
    for cursor in cursors.selections(primary) {
        let bytes = index.bytes(range(cursor));
        let line = text[..bytes.start].rfind('\n').map_or(0, |at| at + 1);
        lines.insert(line);
        for (at, _) in text[line..bytes.end].match_indices('\n') {
            if line + at + 1 < bytes.end {
                lines.insert(line + at + 1);
            }
        }
    }
    let lines: Vec<_> = lines
        .into_iter()
        .map(|start| {
            let line = text[start..]
                .split('\n')
                .next()
                .unwrap_or("")
                .trim_end_matches('\r');
            let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
            (start, line, indent)
        })
        .collect();
    let uncomment = if let LinePrefix::Comment(marker) = command {
        lines.iter().any(|(_, line, indent)| line.len() > *indent)
            && lines.iter().all(|(_, line, indent)| {
                line.len() == *indent || line[*indent..].starts_with(marker)
            })
    } else {
        false
    };
    let padding = " ".repeat(tab);
    let comment = match command {
        LinePrefix::Comment(marker) => format!("{marker} "),
        _ => String::new(),
    };
    let mut changes = Vec::new();
    let mut result = String::with_capacity(text.len());
    let mut written = 0;
    for (start, line, indent) in lines {
        let at = if matches!(command, LinePrefix::Comment(_)) {
            start + indent
        } else {
            start
        };
        let (length, inserted) = match command {
            LinePrefix::Indent => (0, padding.as_str()),
            LinePrefix::Unindent if line.starts_with('\t') => (1, ""),
            LinePrefix::Unindent => (
                line.bytes().take(tab).take_while(|&b| b == b' ').count(),
                "",
            ),
            LinePrefix::Comment(marker) if uncomment && line[indent..].starts_with(marker) => {
                let len = marker.len()
                    + usize::from(line.as_bytes().get(indent + marker.len()) == Some(&b' '));
                (len, "")
            }
            LinePrefix::Comment(_) if !uncomment && line.len() > indent => (0, comment.as_str()),
            LinePrefix::Comment(_) => continue,
        };
        result.push_str(&text[written..at]);
        result.push_str(inserted);
        written = at + length;
        changes.push((
            index.character(at)..index.character(written),
            inserted.chars().count(),
        ));
    }
    result.push_str(&text[written..]);
    let adjust = |cursor: CCursor| {
        let position = cursor.index.0;
        let mut added = 0;
        let mut removed = 0;
        for (range, len) in &changes {
            if position < range.start {
                break;
            }
            if position < range.end {
                return CCursor::new(range.start + added - removed);
            }
            added += len;
            removed += range.len();
        }
        CCursor::new(position + added - removed)
    };
    let map = |cursor: CCursorRange| CCursorRange {
        primary: adjust(cursor.primary),
        secondary: adjust(cursor.secondary),
        ..cursor
    };
    (
        result,
        map(primary),
        cursors.others.iter().copied().map(map).collect(),
    )
}

fn record_change(
    state: &mut egui::text_edit::TextEditState,
    text: &mut Cow<'_, str>,
    before: CCursorRange,
    after: CCursorRange,
    updated: String,
) -> bool {
    if updated == *text {
        return false;
    }
    let mut undo = state.undoer();
    undo.add_undo(&(
        before,
        std::mem::replace(text, Cow::Owned(updated)).into_owned(),
    ));
    undo.add_undo(&(after, text.to_string()));
    state.set_undoer(undo);
    true
}

pub(super) fn copy(
    ui: &Ui,
    file: &str,
    text: &str,
    primary: CCursorRange,
    cursors: &Cursors,
    dark: bool,
    syntax: bool,
) {
    let index = Index::new(text);
    let mut ranges: Vec<_> = cursors
        .selections(primary)
        .map(|cursor| index.bytes(range(cursor)))
        .collect();
    if ranges.iter().all(Range::is_empty) {
        for range in &mut ranges {
            range.start = text[..range.start].rfind('\n').map_or(0, |n| n + 1);
            range.end = text[range.end..]
                .find('\n')
                .map_or(text.len(), |n| range.end + n + 1);
        }
    }
    ranges.retain(|r| !r.is_empty());
    ranges.sort_unstable_by_key(|r| (r.start, r.end));
    ranges.dedup();
    let runs = if syntax {
        highlight::whole(file, text, dark)
    } else {
        Vec::new()
    };
    clipboard::copy(ui, text, &ranges, &runs, super::p().fg);
}

#[allow(clippy::too_many_arguments)]
pub fn input(
    ui: &Ui,
    id: Id,
    file: &str,
    text: &mut Cow<'_, str>,
    cursors: &mut Cursors,
    prefs: &EditorPrefs,
    dark: bool,
    editable: bool,
) -> InputEffect {
    if !ui.memory(|memory| memory.has_focus(id))
        || crate::app::dialog_open(ui.ctx())
        || ui.input(|input| input.events.is_empty())
    {
        return InputEffect::default();
    }
    let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) else {
        return InputEffect::default();
    };
    let Some(mut primary) = state.cursor.char_range() else {
        return InputEffect::default();
    };
    let limit = text.chars().count();
    primary.primary.index.0 = primary.primary.index.0.min(limit);
    primary.secondary.index.0 = primary.secondary.index.0.min(limit);
    primary = document_cursor(text, primary);
    let before = primary;
    cursors.others.retain(|cursor| range(*cursor).end <= limit);
    if !cursors.others.is_empty() && text.as_bytes().contains(&b'\r') {
        let index = Index::new(text);
        for cursor in &mut cursors.others {
            *cursor = index.snap_cursor(text, *cursor);
        }
    }
    let marker = comment_marker(file);
    let mut process = Vec::new();
    ui.input_mut(|input| {
        if input
            .events
            .iter()
            .any(|event| matches!(event, Event::Ime(_)))
        {
            cursors.clear();
            return;
        }
        let mut multiple = !cursors.others.is_empty();
        input.events.retain(|event| {
            let handle = match event {
                Event::Key {
                    key,
                    modifiers,
                    pressed: true,
                    ..
                } if shortcut(*key, *modifiers)
                    && (*key != Key::Slash || editable && marker.is_some())
                    && (editable || line_command(*key, *modifiers).is_none()) =>
                {
                    multiple = true;
                    true
                }
                Event::Copy => true,
                Event::Cut | Event::Paste(_) | Event::Text(_) => multiple && editable,
                Event::Key {
                    key,
                    modifiers,
                    pressed: true,
                    ..
                } => {
                    let navigation = multiple && navigation(*key);
                    let edit = editable
                        && (modifiers.is_none() && matches!(key, Key::Enter | Key::Tab)
                            || modifiers.matches_exact(Modifiers::SHIFT) && *key == Key::Tab
                            || multiple && matches!(key, Key::Backspace | Key::Delete));
                    if multiple
                        && (*key == Key::Escape
                            || modifiers.command && matches!(key, Key::A | Key::Z | Key::Y))
                    {
                        cursors.clear();
                        multiple = false;
                    }
                    edit || navigation
                }
                _ => false,
            };
            if handle {
                process.push(event.clone());
            }
            !handle
        });
    });

    let mut changed = false;
    for event in process {
        if let Event::Key { key, modifiers, .. } = event
            && let Some(command) = line_command(key, modifiers)
        {
            if let Some((next, cursor)) = line_edit(text, primary, command, prefs.crlf) {
                changed |= record_change(&mut state, text, primary, cursor, next);
                primary = cursor;
                cursors.clear();
            }
            continue;
        }
        if let Event::Key { key, modifiers, .. } = event
            && !shortcut(key, modifiers)
            && navigation(key)
        {
            let galley = cursors
                .last_galley
                .as_ref()
                .filter(|galley| galley.job.text == *text)
                .cloned()
                .unwrap_or_else(|| {
                    ui.fonts_mut(|fonts| {
                        fonts.layout_no_wrap(
                            text.to_string(),
                            prefs.font.clone(),
                            Color32::PLACEHOLDER,
                        )
                    })
                });
            let os = ui.ctx().os();
            primary.on_key_press(os, &galley, &modifiers, key);
            for cursor in &mut cursors.others {
                cursor.on_key_press(os, &galley, &modifiers, key);
            }
            let mut seen = vec![range(primary)];
            cursors.others.retain(|cursor| {
                let next = range(*cursor);
                if seen.iter().any(|existing| overlaps(existing, &next)) {
                    false
                } else {
                    seen.push(next);
                    true
                }
            });
            continue;
        }
        let prefix = match &event {
            Event::Key {
                key: Key::Tab,
                modifiers,
                ..
            } if modifiers.shift => Some(LinePrefix::Unindent),
            Event::Key { key: Key::Tab, .. }
                if cursors.selections(primary).any(|cursor| !cursor.is_empty()) =>
            {
                Some(LinePrefix::Indent)
            }
            Event::Key {
                key: Key::Slash, ..
            } => marker.map(LinePrefix::Comment),
            _ => None,
        };
        if let Some(prefix) = prefix {
            let (next, cursor, others) = prefix_lines(text, primary, cursors, prefix, prefs.tab);
            changed |= record_change(&mut state, text, primary, cursor, next);
            primary = cursor;
            cursors.others = others;
            continue;
        }
        let op = match &event {
            Event::Key { key: Key::D, .. } => {
                occurrences(text, &mut primary, cursors, false);
                None
            }
            Event::Key { key: Key::L, .. } => {
                occurrences(text, &mut primary, cursors, true);
                None
            }
            Event::Key {
                key: Key::Backslash,
                ..
            } => {
                let index = Index::new(text);
                let caret = index.byte(primary.primary.index.0);
                if let Some((_, other)) =
                    highlight::bracket_at(&highlight::brackets(file, text), caret)
                {
                    primary = CCursorRange::one(CCursor::new(index.character(other)));
                    cursors.clear();
                }
                None
            }
            Event::Key {
                key: key @ (Key::ArrowUp | Key::ArrowDown),
                ..
            } => {
                if cursors.others.len() + 1 < MAX_CURSORS
                    && let Some(next) = adjacent_line(text, primary, *key == Key::ArrowUp)
                    && !cursors.overlaps(primary, next)
                {
                    cursors.others.push(primary);
                    primary = next;
                }
                None
            }
            Event::Copy | Event::Cut => {
                copy(ui, file, text, primary, cursors, dark, prefs.syntax);
                matches!(event, Event::Cut).then_some(Change::Cut)
            }
            Event::Text(s) | Event::Paste(s) => (!s.is_empty()).then_some(Change::Insert(s)),
            Event::Key {
                key: Key::Enter, ..
            } => Some(Change::Enter),
            Event::Key { key: Key::Tab, .. } => Some(Change::Tab),
            Event::Key {
                key: Key::Backspace,
                modifiers,
                ..
            } => Some(Change::Backspace(modifiers.ctrl || modifiers.alt)),
            Event::Key {
                key: Key::Delete,
                modifiers,
                ..
            } => Some(Change::Delete(modifiers.ctrl || modifiers.alt)),
            _ => None,
        };
        if let Some(op) = op {
            let (next, cursor, others) = apply(text, primary, cursors, op, prefs);
            changed |= record_change(&mut state, text, primary, cursor, next);
            primary = cursor;
            cursors.others = others;
        }
    }
    state.cursor.set_char_range(Some(primary));
    state.store(ui.ctx(), id);
    InputEffect {
        changed,
        reveal_caret: primary != before,
    }
}

#[derive(Default)]
pub struct InputEffect {
    pub changed: bool,
    pub reveal_caret: bool,
}

#[cfg(test)]
#[test]
fn headless_multiple_cursors_edit_unicode_and_crlf_in_document_order() {
    let text = "π match\r\nmatch\r\nmatch";
    let index = Index::new(text);
    let mut primary = CCursorRange::one(CCursor::new(3));
    let mut cursors = Cursors::default();
    occurrences(text, &mut primary, &mut cursors, false);
    occurrences(text, &mut primary, &mut cursors, true);
    assert_eq!(range(primary), 2..7);
    assert_eq!(cursors.others.len(), 2);
    assert_eq!(&text[index.bytes(range(cursors.others[1]))], "match");
    let prefs = EditorPrefs {
        font: egui::FontId::monospace(12.0),
        tab: 4,
        crlf: true,
        syntax: true,
        numbers: true,
    };
    let (edited, primary, others) = apply(text, primary, &cursors, Change::Insert("é\n字"), &prefs);
    assert_eq!(edited, "π é\r\n字\r\né\r\n字\r\né\r\n字");
    assert_eq!(range(primary), 6..6);
    assert_eq!(others.len(), 2);
}

#[cfg(test)]
#[test]
fn headless_editor_indents_and_comments_without_replacing_selected_text() {
    let text = "  α()\r\n  β()\r\nγ()";
    let cursor = selection(0..text.split_once("γ").unwrap().0.chars().count());
    let cursors = Cursors::default();
    let (indented, shifted, _) = prefix_lines(text, cursor, &cursors, LinePrefix::Indent, 2);
    assert_eq!(indented, "    α()\r\n    β()\r\nγ()");
    let (restored, shifted, _) =
        prefix_lines(&indented, shifted, &cursors, LinePrefix::Unindent, 2);
    assert_eq!(restored, text);
    let (commented, shifted, _) =
        prefix_lines(text, shifted, &cursors, LinePrefix::Comment("//"), 2);
    assert_eq!(commented, "  // α()\r\n  // β()\r\nγ()");
    assert_eq!(
        prefix_lines(&commented, shifted, &cursors, LinePrefix::Comment("//"), 2).0,
        text
    );
}

#[cfg(test)]
#[test]
fn headless_editor_moves_duplicates_and_deletes_lines_with_crlf() {
    let text = "α\r\nβ\r\nγ";
    let cursor = CCursorRange::one(CCursor::new(3));
    let (moved, moved_at) = line_edit(text, cursor, LineCommand::Move(false), true).unwrap();
    assert_eq!(moved, "α\r\nγ\r\nβ");
    assert_eq!(range(moved_at), 6..6);
    let (duplicated, duplicated_at) =
        line_edit(&moved, moved_at, LineCommand::Duplicate(false), true).unwrap();
    assert_eq!(duplicated, "α\r\nγ\r\nβ\r\nβ");
    assert_eq!(
        line_edit(&duplicated, duplicated_at, LineCommand::Delete, true)
            .unwrap()
            .0,
        moved
    );
}
