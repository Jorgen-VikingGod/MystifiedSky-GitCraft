//! Unicode decorations for file views, with the same character data used by the web editor.

use super::highlight;
use crate::app::fonts::emoji_cluster;
use egui::epaint::text::CharIndex;
use egui::text::CCursor;
use egui::{Color32, Galley, Id, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::LazyLock;
use unicode_segmentation::UnicodeSegmentation;

const MAX_MARKS: usize = 1000;

#[derive(Deserialize)]
struct CharacterData {
    invisible: Vec<u32>,
    confusable: Vec<(u32, u32)>,
    locale_exceptions: BTreeMap<String, Vec<u32>>,
}

static DATA: LazyLock<CharacterData> = LazyLock::new(|| {
    let (_, table) = include_str!("../../../assets/unicode/monaco-0.57.txt")
        .split_once("BEGIN CHARACTER TABLE")
        .expect("Unicode data follows its license");
    serde_json::from_str(table).expect("embedded Unicode data")
});

#[cfg(windows)]
fn system_locale() -> Option<String> {
    use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;
    let mut buffer = [0u16; 85]; // Windows LOCALE_NAME_MAX_LENGTH, including the terminator.
    // The writable buffer is valid for the supplied length. Windows returns the length with NUL.
    let length = unsafe { GetUserDefaultLocaleName(buffer.as_mut_ptr(), buffer.len() as i32) };
    let end = usize::try_from(length.checked_sub(1)?).ok()?;
    String::from_utf16(buffer.get(..end)?).ok()
}

#[cfg(not(windows))]
fn system_locale() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|key| std::env::var(key).ok().filter(|value| !value.is_empty()))
}

static LOCALE: LazyLock<String> = LazyLock::new(|| {
    let locale = system_locale().unwrap_or_default();
    locale
        .split(['.', '@'])
        .next()
        .unwrap_or("")
        .replace('_', "-")
        .to_ascii_lowercase()
});

fn exceptions(locale: &str) -> &[u32] {
    let language = locale.split('-').next().unwrap_or(locale);
    let script = match locale {
        "zh-cn" | "zh-sg" => "zh-hans",
        "zh-tw" | "zh-hk" | "zh-mo" => "zh-hant",
        _ => language,
    };
    DATA.locale_exceptions
        .get(locale)
        .or_else(|| DATA.locale_exceptions.get(script))
        .map_or(&[], Vec::as_slice)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reason {
    Invisible,
    Confusable(char),
}

fn invisible(c: char) -> bool {
    DATA.invisible.binary_search(&(c as u32)).is_ok()
}

fn confusable(c: char, allowed: &[u32]) -> Option<char> {
    let code = c as u32;
    if c.is_ascii() || allowed.binary_search(&code).is_ok() {
        return None;
    }
    let i = DATA
        .confusable
        .binary_search_by_key(&code, |&(from, _)| from)
        .ok()?;
    char::from_u32(DATA.confusable[i].1)
}

fn reason(c: char, allowed: &[u32]) -> Option<Reason> {
    if !matches!(c, ' ' | '\t' | '\n' | '\r') && invisible(c) {
        Some(Reason::Invisible)
    } else {
        confusable(c, allowed).map(Reason::Confusable)
    }
}

fn separator(c: char) -> bool {
    (c.is_ascii_punctuation() && c != '_')
        || (c.is_whitespace() && c != '\u{85}')
        || c == '\u{feff}'
}

fn natural_word(word: &str, allowed: &[u32]) -> bool {
    !word.bytes().any(|b| b.is_ascii())
        && word
            .chars()
            .any(|c| confusable(c, allowed).is_none() && !invisible(c))
}

#[derive(Debug)]
pub(super) struct Mark {
    column: usize,
    byte: usize,
    scalar: char,
    reason: Reason,
    in_comment: bool,
}

fn line_marks(text: &str, allowed: &[u32], limit: usize) -> Vec<Mark> {
    if text.is_ascii() && !text.bytes().any(|b| matches!(b, 0x0b | 0x0c | 0x7f)) {
        return Vec::new();
    }
    if !text.chars().any(|c| reason(c, allowed).is_some()) {
        return Vec::new();
    }
    let mut emoji = text
        .grapheme_indices(true)
        .filter(|(_, cluster)| !cluster.is_ascii() && emoji_cluster(cluster))
        .peekable();
    let mut result = Vec::new();
    let mut byte = 0;
    let mut column = 0;
    for part in text.split_inclusive(separator) {
        let word = part.trim_end_matches(separator);
        let natural = !word.is_ascii() && natural_word(word, allowed);
        for (relative, scalar) in part.char_indices() {
            let offset = byte + relative;
            let pos = column;
            column += 1;
            if (natural && relative < word.len()) || scalar.is_ascii_graphic() {
                continue;
            }
            let Some(reason) = reason(scalar, allowed) else {
                continue;
            };
            while emoji
                .peek()
                .is_some_and(|(start, cluster)| *start + cluster.len() <= offset)
            {
                emoji.next();
            }
            if emoji.peek().is_some_and(|(start, _)| *start <= offset) {
                continue;
            }
            result.push(Mark {
                column: pos,
                byte: offset,
                scalar,
                reason,
                in_comment: false,
            });
            if result.len() >= limit {
                return result;
            }
        }
        byte += part.len();
    }
    result
}

#[derive(Default)]
pub(in crate::app) struct Report {
    lines: BTreeMap<usize, Vec<Mark>>,
    pub(super) more: bool,
}

impl Report {
    pub(super) fn scan(file: &str, text: &str) -> Self {
        Self::with_locale(file, text, &LOCALE)
    }

    fn with_locale(file: &str, text: &str, locale: &str) -> Self {
        let allowed = exceptions(locale);
        let mut report = Self::default();
        let mut count = 0;
        for (line, text) in text.lines().enumerate() {
            let mut marks = line_marks(text, allowed, MAX_MARKS + 1 - count);
            if marks.is_empty() {
                continue;
            }
            if count + marks.len() > MAX_MARKS {
                report.more = true;
                marks.truncate(MAX_MARKS - count);
            }
            if !marks.is_empty() {
                count += marks.len();
                report.lines.insert(line, marks);
            }
            if report.more {
                break;
            }
        }
        if let Some((&last, _)) = report.lines.last_key_value() {
            highlight::comment_spans(file, text, last, |line, range| {
                if let Some(marks) = report.lines.get_mut(&line) {
                    let start = marks.partition_point(|mark| mark.byte < range.start);
                    for mark in marks[start..]
                        .iter_mut()
                        .take_while(|mark| mark.byte < range.end)
                    {
                        mark.in_comment = true;
                    }
                }
            });
        }
        report
    }

    pub(super) fn line(&self, row: usize) -> &[Mark] {
        self.lines.get(&row).map_or(&[], Vec::as_slice)
    }
}

pub(super) fn caption(ui: &mut Ui) {
    ui.label("More Unicode characters in this file. Showing the first 1,000 matches.");
}

pub(super) fn ink(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(255, 188, 82)
    } else {
        Color32::from_rgb(127, 71, 0)
    }
}

/// Paint independently of syntax, Find, and diff backgrounds. A zero-width character gets a
/// small marker at its caret location; the layout, clipboard, and document still see its bytes.
#[allow(clippy::too_many_arguments)]
pub(super) fn paint(
    ui: &Ui,
    id: Id,
    painter: &egui::Painter,
    galley: &Galley,
    pos: Pos2,
    marks: &[Mark],
    start: usize,
    visible: Range<usize>,
    hide_comments: bool,
    color: Color32,
) {
    for mark in marks {
        let at = start + mark.column;
        if !visible.contains(&at) || (hide_comments && mark.in_comment) {
            continue;
        }
        let point = CCursor {
            index: CharIndex(at),
            prefer_next_row: true,
        };
        let cursor = galley.layout_from_cursor(point);
        let Some(row) = galley.rows.get(cursor.row) else {
            continue;
        };
        if cursor.column >= row.char_count_excluding_newline() {
            continue;
        }
        let ranges = row.x_ranges(cursor.column..cursor.column + CharIndex(1));
        let (mut left, mut right) = ranges.first().map_or_else(
            || {
                let at = galley.pos_from_cursor(point).left();
                (at, at)
            },
            |x| (row.pos.x + x.min, row.pos.x + x.max),
        );
        if right - left < 3.0 {
            let at = (left + right) / 2.0;
            left = at - 2.0;
            right = at + 2.0;
        }
        let rect = Rect::from_min_max(
            pos + pos2(left, row.min_y() + 1.0).to_vec2(),
            pos + pos2(right, row.max_y() - 1.0).to_vec2(),
        );
        if !painter.clip_rect().intersects(rect) {
            continue;
        }
        painter.rect_filled(rect, 1.0, color.gamma_multiply(0.17));
        painter.rect_stroke(rect, 1.0, Stroke::new(1.0, color), StrokeKind::Inside);
        if ui.rect_contains_pointer(rect) {
            let code = mark.scalar as u32;
            let label = match mark.reason {
                Reason::Invisible => format!("Invisible Unicode character U+{code:04X}"),
                Reason::Confusable(other) if other.is_ascii_graphic() => {
                    format!("Unicode character U+{code:04X} resembles {other:?}")
                }
                Reason::Confusable(other) => {
                    format!(
                        "Unicode character U+{code:04X} resembles U+{:04X}",
                        other as u32
                    )
                }
            };
            ui.interact(rect, id.with(at), Sense::hover())
                .on_hover_text(label);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accents_emoji_and_natural_languages_are_not_warnings() {
        for sample in [
            "café résumé naïve über español português français ß Ł á à ä å Å ø Ø æ æther",
            "Cafe\u{301} man\u{303}ana Đà Nẵng",
            "κόσμος мир 漢字 العربية",
            "😀 🍕 😮‍💨 👩🏽‍💻 🧑‍🤝‍🧑 🏳️‍🌈 1️⃣ ☀️",
            "text👩🏽‍💻more #️⃣ *️⃣ 1️⃣ text©️more 🇺🇸",
            "flags: \u{1f3f4}\u{e0067}\u{e0062}\u{e0065}\u{e006e}\u{e0067}\u{e007f}",
        ] {
            let report = Report::with_locale("text.txt", sample, "en-us");
            assert!(
                report.line(0).is_empty(),
                "{sample:?}: {:?}",
                report.line(0)
            );
        }
        assert!(
            Report::with_locale("text.txt", "sıra", "tr-tr")
                .line(0)
                .is_empty()
        );
    }

    #[test]
    fn detects_lookalikes_invisibles_and_isolated_emoji_components() {
        let doc = "U+0430: payp\u{430}l; x\u{202e}y; a\u{200b}b; no\u{a0}break;\r\n\
            notAnEmoji\u{200d}here vs\u{fe0f}x 1\u{fe0f} 😀\u{200d}tail\r\n";
        let report = Report::with_locale("text.txt", doc, "en-us");
        let marks = |row| {
            report
                .line(row)
                .iter()
                .map(|mark| (mark.scalar, mark.reason))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            marks(0),
            vec![
                ('\u{430}', Reason::Confusable('a')),
                ('\u{202e}', Reason::Invisible),
                ('\u{200b}', Reason::Invisible),
                ('\u{a0}', Reason::Invisible),
            ]
        );
        assert_eq!(
            marks(1),
            vec![
                ('\u{200d}', Reason::Invisible),
                ('\u{fe0f}', Reason::Invisible),
                ('\u{fe0f}', Reason::Invisible),
                ('\u{200d}', Reason::Invisible),
            ]
        );
        assert!(marks(2).is_empty());
        let unicode = Report::with_locale("notes.txt", "😀ax\u{200b}y", "en-us");
        assert_eq!((unicode.line(0)[0].column, unicode.line(0)[0].byte), (3, 6));
    }

    #[test]
    fn comment_scope_is_preserved_across_lines_and_does_not_hide_strings() {
        let doc = "/* first line\npayp\u{430}l */ let example = \"a\u{200b}b\"; // x\u{202e}y\n";
        let report = Report::with_locale("code.rs", doc, "en-us");
        let comment = report
            .line(1)
            .iter()
            .map(|mark| mark.in_comment)
            .collect::<Vec<_>>();
        assert_eq!(comment, [true, false, true]);
        let plain = Report::with_locale("plain.txt", doc, "en-us");
        assert!(plain.line(1).iter().all(|mark| !mark.in_comment));
    }

    #[test]
    fn excessive_warnings_are_bounded_even_on_one_long_line() {
        let text = "g\u{200b}".repeat(MAX_MARKS + 10);
        let report = Report::with_locale("plain.txt", &text, "en-us");
        assert_eq!(report.line(0).len(), MAX_MARKS);
        assert!(report.more);
    }
}
