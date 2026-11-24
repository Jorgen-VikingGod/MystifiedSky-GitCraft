use egui::text::{CCursor, CCursorRange};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

pub struct FoldedRegions(Vec<(usize, Range<usize>)>);

#[derive(Default)]
pub struct Folds {
    revision: Option<(super::FileRevision, usize)>,
    generation: u64,
    regions: BTreeMap<usize, usize>,
    collapsed: BTreeSet<usize>,
}

impl Folds {
    pub fn update(&mut self, revision: super::FileRevision, file: &str, text: &str, tab: usize) {
        if self.revision.replace((revision, tab)) != Some((revision, tab)) {
            self.regions = regions(file, text, tab);
            self.unfold_all();
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn any(&self) -> bool {
        !self.regions.is_empty()
    }
    pub fn foldable(&self, row: usize) -> bool {
        self.regions.contains_key(&row)
    }
    pub fn collapsed(&self, row: usize) -> bool {
        self.collapsed.contains(&row)
    }
    pub fn has_collapsed(&self) -> bool {
        !self.collapsed.is_empty()
    }
    pub fn unfold_all(&mut self) {
        if !self.collapsed.is_empty() {
            self.collapsed.clear();
            self.generation += 1;
        }
    }

    pub fn fold_all(&mut self) {
        let old = self.collapsed.len();
        self.collapsed.extend(self.regions.keys());
        self.generation += u64::from(self.collapsed.len() != old);
    }

    pub fn toggle(&mut self, row: usize) {
        if self.regions.contains_key(&row) {
            if !self.collapsed.remove(&row) {
                self.collapsed.insert(row);
            }
            self.generation += 1;
        }
    }

    pub fn reveal(&mut self, row: usize) {
        let old = self.collapsed.len();
        self.collapsed
            .retain(|header| row <= *header || row > self.regions[header]);
        self.generation += u64::from(self.collapsed.len() != old);
    }

    pub(super) fn visible_lines(&self, count: usize) -> VisibleLines {
        let mut starts = vec![(0, 0)];
        let mut visible = 0;
        let mut next = 0;
        for &header in &self.collapsed {
            if header >= next && header < count {
                visible += header + 1 - next;
                next = (self.regions[&header] + 1).min(count);
                starts.push((visible, next));
            }
        }
        VisibleLines {
            starts,
            len: visible + count - next,
        }
    }

    pub fn snapshot(&self, text: &str) -> FoldedRegions {
        let starts: Vec<_> = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(at, _)| at + 1))
            .collect();
        FoldedRegions(
            self.collapsed
                .iter()
                .map(|&row| {
                    let end = starts
                        .get(self.regions[&row] + 1)
                        .copied()
                        .unwrap_or(text.len());
                    (row, starts[row]..end)
                })
                .collect(),
        )
    }

    pub fn restore_after_edit(&mut self, old: &str, new: &str, snapshot: FoldedRegions) {
        let shared = old
            .bytes()
            .zip(new.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        let prefix = old.floor_char_boundary(shared);
        let suffix = old
            .bytes()
            .rev()
            .zip(new.bytes().rev())
            .take(old.len().min(new.len()) - prefix)
            .take_while(|(a, b)| a == b)
            .count();
        let old_end = old.ceil_char_boundary(old.len() - suffix);
        let new_end = new.len() - (old.len() - old_end);
        let old_lines = old[prefix..old_end].bytes().filter(|&b| b == b'\n').count();
        let new_lines = new[prefix..new_end].bytes().filter(|&b| b == b'\n').count();
        for (row, bytes) in snapshot.0 {
            let unchanged = if bytes.end <= prefix {
                Some(row)
            } else if bytes.start > old_end {
                Some(row + new_lines - old_lines)
            } else {
                None
            };
            if let Some(row) = unchanged
                && self.foldable(row)
            {
                self.generation += u64::from(self.collapsed.insert(row));
            }
        }
    }
}

/// Each entry starts the next consecutive span of visible document lines.
pub(super) struct VisibleLines {
    starts: Vec<(usize, usize)>,
    len: usize,
}

impl VisibleLines {
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn get(&self, row: usize) -> Option<usize> {
        if row >= self.len {
            return None;
        }
        let at = self.starts.partition_point(|(visible, _)| *visible <= row) - 1;
        let (visible, document) = self.starts[at];
        Some(document + row - visible)
    }

    pub fn find(&self, line: usize) -> Option<usize> {
        let at = self
            .starts
            .partition_point(|(_, document)| *document <= line)
            - 1;
        let (visible, document) = self.starts[at];
        let row = visible + line - document;
        let end = self
            .starts
            .get(at + 1)
            .map_or(self.len, |(visible, _)| *visible);
        (row < end).then_some(row)
    }
}

fn regions(file: &str, text: &str, tab: usize) -> BTreeMap<usize, usize> {
    let mut regions = BTreeMap::new();
    let mut stack = Vec::new();
    let mut previous = None;
    let markdown = file.rsplit_once('.').is_some_and(|(_, extension)| {
        extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
    });
    let mut fence = None;
    let mut headings = Vec::new();
    let mut explicit = Vec::new();
    for (row, line) in text.lines().enumerate() {
        let body = line.trim_start();
        if body.trim_end().is_empty() {
            continue;
        }
        let indent = line[..line.len() - body.len()]
            .bytes()
            .fold(0, |column, byte| {
                if byte == b'\t' {
                    column + tab - column % tab
                } else {
                    column + 1
                }
            });
        if let Some((last, old_indent)) = previous {
            while let Some(&(header, level)) = stack.last() {
                if indent > level {
                    break;
                }
                stack.pop();
                regions.insert(header, last);
            }
            if indent > old_indent {
                stack.push((last, old_indent));
            }
        }
        previous = Some((row, indent));

        let marker = body.trim_start_matches(['/', ';', '-']).trim_start();
        let directive = marker.split_whitespace().next();
        if directive == Some("#region") {
            explicit.push(row);
        } else if directive == Some("#endregion")
            && let Some(header) = explicit.pop()
            && row > header + 1
        {
            regions.insert(header, row - 1);
        }

        if markdown {
            for token in ["\u{60}\u{60}\u{60}", "~~~"] {
                if body.starts_with(token) && fence.is_none_or(|open| open == token) {
                    fence = if fence.is_none() { Some(token) } else { None };
                    break;
                }
            }
            if fence.is_none() {
                let hashes = body.bytes().take_while(|&b| b == b'#').count();
                if (1..=6).contains(&hashes) && body.as_bytes().get(hashes) == Some(&b' ') {
                    while let Some(&(header, level)) = headings.last() {
                        if hashes > level {
                            break;
                        }
                        headings.pop();
                        if row > header + 1 {
                            regions.insert(header, row - 1);
                        }
                    }
                    headings.push((row, hashes));
                }
            }
        }
    }
    if let Some((last, _)) = previous {
        for (header, _) in stack.into_iter().chain(headings) {
            if last > header {
                regions.insert(header, last);
            }
        }
    }
    regions
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Line {
    pub original: usize,
    pub start: usize,
    pub removed: Option<usize>,
    document_start: usize,
    content_len: usize,
    len: usize,
}

#[derive(Hash)]
pub struct RemovedLine<'a> {
    pub before: Option<usize>,
    pub number: usize,
    pub text: &'a str,
}

#[derive(Clone, PartialEq, Eq)]
pub struct Positions {
    pub lines: Vec<Line>,
    document_len: usize,
    visible_len: usize,
    has_removed: bool,
}

pub struct Projection<'a> {
    pub text: Cow<'a, str>,
    pub positions: Positions,
}

impl<'a> Projection<'a> {
    #[cfg(test)]
    pub fn new(text: &'a str, folds: &Folds) -> Self {
        Self::with_removed(text, folds, &[])
    }

    pub fn with_removed(text: &'a str, folds: &Folds, removed: &[RemovedLine<'_>]) -> Self {
        let projected = folds.has_collapsed() || !removed.is_empty();
        let mut visible = projected.then(String::new);
        let mut lines = Vec::new();
        let mut visible_len = 0;
        let mut document_len = 0;
        let mut hidden_through = None;
        let mut deletions = removed.iter().peekable();
        let add_removed = |line: &RemovedLine<'_>,
                           row: usize,
                           canonical: usize,
                           lines: &mut Vec<Line>,
                           rendered: &mut usize,
                           visible: &mut Option<String>| {
            let Some(view) = visible else { return };
            if !view.is_empty() && !view.ends_with('\n') {
                view.push('\n');
                *rendered += 1;
            }
            let width = line.text.chars().count();
            lines.push(Line {
                original: row,
                start: *rendered,
                removed: Some(line.number),
                document_start: canonical,
                content_len: width,
                len: width + 1,
            });
            view.push_str(line.text);
            view.push('\n');
            *rendered += width + 1;
        };
        let lines_with_ends = text
            .split_inclusive('\n')
            .chain((text.is_empty() || text.ends_with('\n')).then_some(""));
        for (original, line) in lines_with_ends.enumerate() {
            let len = line.chars().count();
            let show = hidden_through.is_none_or(|end| original > end);
            while deletions
                .peek()
                .is_some_and(|line| line.before == Some(original))
            {
                let line = deletions.next().expect("peek returned a deletion");
                if show {
                    add_removed(
                        line,
                        original,
                        document_len,
                        &mut lines,
                        &mut visible_len,
                        &mut visible,
                    );
                }
            }
            if show {
                let content_len = len
                    - if line.ends_with("\r\n") {
                        2
                    } else {
                        usize::from(line.ends_with('\n'))
                    };
                lines.push(Line {
                    original,
                    start: visible_len,
                    removed: None,
                    document_start: document_len,
                    content_len,
                    len,
                });
                visible_len += len;
                if let Some(visible) = &mut visible {
                    visible.push_str(line);
                }
                if folds.collapsed(original) {
                    hidden_through = Some(folds.regions[&original]);
                }
            }
            document_len += len;
        }
        for line in deletions {
            add_removed(
                line,
                usize::MAX,
                document_len,
                &mut lines,
                &mut visible_len,
                &mut visible,
            );
        }
        Self {
            text: visible.map_or(Cow::Borrowed(text), Cow::Owned),
            positions: Positions {
                lines,
                document_len,
                visible_len,
                has_removed: !removed.is_empty(),
            },
        }
    }
}

impl Positions {
    pub fn has_removed(&self) -> bool {
        self.has_removed
    }
    fn document_offset(&self, visible: usize) -> usize {
        if visible >= self.visible_len {
            return self.document_len;
        }
        let row = self
            .lines
            .partition_point(|line| line.start <= visible)
            .saturating_sub(1);
        let line = self.lines[row];
        if line.removed.is_some() {
            line.document_start
        } else {
            let offset = (visible - line.start).min(line.len);
            let offset = if line.len == line.content_len + 2 && offset == line.content_len + 1 {
                line.content_len
            } else {
                offset
            };
            line.document_start + offset
        }
    }

    fn view_offset(&self, document: usize) -> usize {
        if document >= self.document_len {
            return self
                .lines
                .iter()
                .rev()
                .find(|line| line.removed.is_none())
                .map_or(self.visible_len, |line| line.start + line.len);
        }
        let row = self
            .lines
            .partition_point(|line| line.document_start <= document)
            .saturating_sub(1);
        let line = self.lines[row];
        if line.removed.is_some() {
            return self.lines[..=row]
                .iter()
                .rev()
                .find(|line| line.removed.is_none())
                .map_or(0, |line| line.start + line.content_len);
        }
        let offset = document - line.document_start;
        line.start
            + if offset < line.len {
                offset
            } else {
                line.content_len
            }
    }

    pub fn in_document(&self, cursors: CCursorRange) -> CCursorRange {
        map_cursors(cursors, |cursor| self.document_offset(cursor))
    }

    pub fn in_view(&self, cursors: CCursorRange) -> CCursorRange {
        map_cursors(cursors, |cursor| self.view_offset(cursor))
    }
}

impl std::ops::Deref for Projection<'_> {
    type Target = Positions;

    fn deref(&self) -> &Positions {
        &self.positions
    }
}

fn map_cursors(mut cursors: CCursorRange, change: impl Fn(usize) -> usize) -> CCursorRange {
    cursors.primary = CCursor::new(change(cursors.primary.index.0));
    cursors.secondary = CCursor::new(change(cursors.secondary.index.0));
    cursors
}

#[cfg(test)]
#[test]
fn headless_folds_hide_nested_lines_without_changing_document_positions() {
    let text = "fn main() {\r\n\tif flag {\r\n\t\tπ();\r\n\t}\r\n}\r\nnext();\r\n";
    let mut folds = Folds::default();
    folds.update((1, None), "main.rs", text, 4);
    assert!(folds.foldable(0) && folds.foldable(1));
    folds.toggle(1);
    folds.toggle(0);
    let projection = Projection::new(text, &folds);
    assert_eq!(projection.text, "fn main() {\r\n}\r\nnext();\r\n");
    let at_next = "fn main() {\r\n}\r\n".chars().count();
    let point = CCursorRange::one(CCursor::new(at_next));
    let canonical = projection.in_document(point);
    assert_eq!(
        canonical.primary.index.0,
        text.split_once("next").unwrap().0.chars().count()
    );
    assert_eq!(projection.in_view(canonical), point);
    folds.reveal(2);
    assert_eq!(Projection::new(text, &folds).text, text);
}

#[cfg(test)]
#[test]
fn headless_editor_refolds_unchanged_blocks_after_edits_above_them() {
    let before = "α;\r\nfn block() {\r\n    work();\r\n}\r\n";
    let after = "é;\r\nα;\r\nfn block() {\r\n    work();\r\n}\r\n";
    let mut folds = Folds::default();
    folds.update((1, None), "main.rs", before, 4);
    folds.toggle(1);
    let snapshot = folds.snapshot(before);
    folds.update((1, Some(1)), "main.rs", after, 4);
    folds.restore_after_edit(before, after, snapshot);
    assert!(folds.collapsed(2));
    assert_eq!(
        Projection::new(after, &folds).text,
        "é;\r\nα;\r\nfn block() {\r\n}\r\n"
    );
}

#[cfg(test)]
#[test]
fn headless_inline_removed_rows_map_to_a_valid_editable_position() {
    let text = "α\r\nnew\r\n";
    let old = [RemovedLine {
        before: Some(1),
        number: 0,
        text: "discarded",
    }];
    let folds = Folds::default();
    let projection = Projection::with_removed(text, &folds, &old);
    assert_eq!(projection.text, "α\r\ndiscarded\nnew\r\n");
    let cursor = CCursorRange::one(CCursor::new(5));
    assert_eq!(projection.in_document(cursor).primary.index.0, 3);
    let actual = CCursorRange::one(CCursor::new(4));
    assert_eq!(projection.in_document(projection.in_view(actual)), actual);
}
