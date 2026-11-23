//! Comparing two versions of a file for the diff views: hunks from Git's diff, the rows of the
//! inline and split views from the two texts, and which words changed within a replaced line.

use gitcraft_core::git::{DiffLine, DiffLineKind};
use std::collections::HashMap;
use std::ops::Range;

pub struct Hunk {
    pub header: String,
    /// Body lines; their positions match the patch builder in gitcraft-core's files.rs.
    pub lines: Vec<DiffLine>,
}

pub fn split_hunks(lines: &[DiffLine]) -> (Vec<DiffLine>, Vec<Hunk>) {
    let mut pre = Vec::new();
    let mut hunks: Vec<Hunk> = Vec::new();
    for l in lines {
        match (&l.kind, hunks.last_mut()) {
            (DiffLineKind::Hunk, _) => hunks.push(Hunk { header: l.text.clone(), lines: Vec::new() }),
            (_, Some(h)) => h.lines.push(l.clone()),
            (_, None) => pre.push(l.clone()),
        }
    }
    (pre, hunks)
}

/// Removed lines paired with the added lines that replace them, when the counts match.
pub fn replacement_lines(lines: &[DiffLine]) -> HashMap<usize, usize> {
    let mut pairs = HashMap::new();
    let mut start = 0;
    while start < lines.len() {
        if lines[start].kind != DiffLineKind::Removed {
            start += 1;
            continue;
        }
        let mut added = start;
        while added < lines.len() && lines[added].kind == DiffLineKind::Removed {
            added += 1;
        }
        let mut end = added;
        while end < lines.len() && lines[end].kind == DiffLineKind::Added {
            end += 1;
        }
        if added - start == end - added {
            for i in 0..added - start {
                pairs.insert(start + i, added + i);
                pairs.insert(added + i, start + i);
            }
        }
        start = end.max(start + 1);
    }
    pairs
}

fn tokens(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((i, ch)) = chars.next() {
        let word = |c: char| c.is_alphanumeric() || c == '_';
        let mut end = i + ch.len_utf8();
        if word(ch) {
            while let Some(&(j, c)) = chars.peek().filter(|(_, c)| word(*c)) {
                end = j + c.len_utf8();
                chars.next();
            }
        } else if ch.is_whitespace() {
            while let Some(&(j, c)) = chars.peek().filter(|(_, c)| c.is_whitespace()) {
                end = j + c.len_utf8();
                chars.next();
            }
        }
        out.push(i..end);
    }
    out
}

/// The changed words of `line` compared with `other` (byte ranges), when the lines are similar
/// enough for that to read well.
pub fn changed_words(line: &str, other: &str) -> Option<Vec<Range<usize>>> {
    if line == other || line.len() > 1000 || other.len() > 1000 {
        return None;
    }
    let words = tokens(line);
    let compare = tokens(other);
    if words.len() > 100 || compare.len() > 100 {
        return None;
    }
    let (a_n, b_n) = (words.len(), compare.len());
    let stride = b_n + 1;
    let mut score = vec![0u8; (a_n + 1) * stride];
    for a in (0..a_n).rev() {
        for b in (0..b_n).rev() {
            let cell = a * stride + b;
            score[cell] = if line[words[a].clone()] == other[compare[b].clone()] { 1 + score[cell + stride + 1] } else { score[cell + stride].max(score[cell + 1]) };
        }
    }
    if (score[0] as usize) * 3 < a_n.max(b_n) {
        return None;
    }
    let mut changed = vec![true; a_n];
    let (mut a, mut b) = (0, 0);
    while a < a_n && b < b_n {
        if line[words[a].clone()] == other[compare[b].clone()] {
            changed[a] = false;
            a += 1;
            b += 1;
        } else if score[(a + 1) * stride + b] >= score[a * stride + b + 1] {
            a += 1;
        } else {
            b += 1;
        }
    }
    Some(words.into_iter().zip(changed).filter(|(_, c)| *c).map(|(r, _)| r).collect())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RowKind {
    Same,
    Removed,
    Added,
}

/// One row of the inline view: old and new line numbers (1-based) and which text it shows.
#[derive(Clone, Debug)]
pub struct Row {
    pub kind: RowKind,
    pub old: Option<usize>,
    pub new: Option<usize>,
    /// The other side's line it replaces, for word marks: (old index, new index).
    pub pair: Option<(usize, usize)>,
}

/// One row of the split view.
#[derive(Clone, Debug)]
pub struct SplitRow {
    pub left: Option<usize>,
    pub right: Option<usize>,
    pub changed: bool,
}

pub struct Comparison {
    pub old: Vec<String>,
    pub new: Vec<String>,
    pub rows: Vec<Row>,
    pub split: Vec<SplitRow>,
    /// Rows (inline) where a block of changes starts.
    pub changes: Vec<usize>,
    pub split_changes: Vec<usize>,
    pub added: usize,
    pub removed: usize,
}

pub fn compare(old: &str, new: &str, ignore_ws: bool) -> Comparison {
    let old_lines: Vec<String> = old.lines().map(str::to_owned).collect();
    let new_lines: Vec<String> = new.lines().map(str::to_owned).collect();
    let key = |l: &String| if ignore_ws { l.trim().to_owned() } else { l.clone() };
    let a: Vec<String> = old_lines.iter().map(key).collect();
    let b: Vec<String> = new_lines.iter().map(key).collect();
    let ops = similar::capture_diff_slices_deadline(similar::Algorithm::Myers, &a, &b, Some(std::time::Instant::now() + std::time::Duration::from_secs(2)));
    let mut rows = Vec::new();
    let mut split = Vec::new();
    let mut changes = Vec::new();
    let mut split_changes = Vec::new();
    let (mut added, mut removed) = (0, 0);
    for op in ops {
        use similar::DiffOp;
        match op {
            DiffOp::Equal { old_index, new_index, len } => {
                for k in 0..len {
                    rows.push(Row { kind: RowKind::Same, old: Some(old_index + k), new: Some(new_index + k), pair: None });
                    split.push(SplitRow { left: Some(old_index + k), right: Some(new_index + k), changed: false });
                }
            }
            DiffOp::Delete { old_index, old_len, .. } => {
                changes.push(rows.len());
                split_changes.push(split.len());
                removed += old_len;
                for k in 0..old_len {
                    rows.push(Row { kind: RowKind::Removed, old: Some(old_index + k), new: None, pair: None });
                    split.push(SplitRow { left: Some(old_index + k), right: None, changed: true });
                }
            }
            DiffOp::Insert { new_index, new_len, .. } => {
                changes.push(rows.len());
                split_changes.push(split.len());
                added += new_len;
                for k in 0..new_len {
                    rows.push(Row { kind: RowKind::Added, old: None, new: Some(new_index + k), pair: None });
                    split.push(SplitRow { left: None, right: Some(new_index + k), changed: true });
                }
            }
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                changes.push(rows.len());
                split_changes.push(split.len());
                removed += old_len;
                added += new_len;
                let paired = old_len == new_len;
                for k in 0..old_len {
                    rows.push(Row { kind: RowKind::Removed, old: Some(old_index + k), new: None, pair: paired.then_some((old_index + k, new_index + k)) });
                }
                for k in 0..new_len {
                    rows.push(Row { kind: RowKind::Added, old: None, new: Some(new_index + k), pair: paired.then_some((old_index + k, new_index + k)) });
                }
                for k in 0..old_len.max(new_len) {
                    split.push(SplitRow { left: (k < old_len).then_some(old_index + k), right: (k < new_len).then_some(new_index + k), changed: true });
                }
            }
        }
    }
    Comparison { old: old_lines, new: new_lines, rows, split, changes, split_changes, added, removed }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(kind: DiffLineKind, text: &str) -> DiffLine {
        DiffLine { kind, old_no: None, new_no: None, text: text.into() }
    }

    #[test]
    fn replaced_lines_pair_up_only_when_counts_match() {
        let l = [line(DiffLineKind::Context, "a"), line(DiffLineKind::Removed, "b"), line(DiffLineKind::Added, "c"), line(DiffLineKind::Removed, "d"), line(DiffLineKind::Added, "e"), line(DiffLineKind::Added, "f")];
        let p = replacement_lines(&l);
        assert_eq!(p.get(&1), Some(&2));
        assert!(!p.contains_key(&3));
    }

    #[test]
    fn words_that_changed() {
        let marks = changed_words("let total = sum(a, b);", "let total = add(a, b);").unwrap();
        assert_eq!(marks.len(), 1);
        assert_eq!(&"let total = sum(a, b);"[marks[0].clone()], "sum");
        assert!(changed_words("completely", "different words here").is_none());
    }

    #[test]
    fn comparisons_count_and_align() {
        let c = compare("a\nb\nc\n", "a\nB\nc\nd\n", false);
        assert_eq!((c.added, c.removed), (2, 1));
        assert_eq!(c.split.len(), 4);
        assert_eq!(c.changes.len(), 2);
        let ws = compare("  a\n", "a\n", true);
        assert_eq!(ws.added + ws.removed, 0);
    }
}
