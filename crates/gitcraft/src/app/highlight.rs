//! Syntax colours for diffs, the file viewer and the editor, in the editor colours the desktop
//! app has always used (Dark+ and Light+).

use egui::Color32;
use std::collections::BTreeMap;
use std::sync::LazyLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Color as SyntaxColor, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};

/// Files with more lines than this are shown without colours.
pub const LINE_LIMIT: usize = 20_000;
/// Longer lines are left uncoloured: they are usually minified or generated.
const LENGTH_LIMIT: usize = 2000;

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static DARK: LazyLock<Theme> = LazyLock::new(|| theme(true));
static LIGHT: LazyLock<Theme> = LazyLock::new(|| theme(false));

/// Coloured runs of one line: colour and byte range.
pub type Runs = Vec<(Color32, std::ops::Range<usize>)>;

fn sc(c: Color32) -> SyntaxColor {
    SyntaxColor { r: c.r(), g: c.g(), b: c.b(), a: 255 }
}

const fn rgb(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

fn theme(dark: bool) -> Theme {
    let (text, comment, string, number, keyword, control, function, ty, variable, tag, attr, regex) = if dark {
        (rgb(0xd4d4d4), rgb(0x6a9955), rgb(0xce9178), rgb(0xb5cea8), rgb(0x569cd6), rgb(0xc586c0), rgb(0xdcdcaa), rgb(0x4ec9b0), rgb(0x9cdcfe), rgb(0x569cd6), rgb(0x9cdcfe), rgb(0xd16969))
    } else {
        (rgb(0x000000), rgb(0x008000), rgb(0xa31515), rgb(0x098658), rgb(0x0000ff), rgb(0xaf00db), rgb(0x795e26), rgb(0x267f99), rgb(0x001080), rgb(0x800000), rgb(0xe50000), rgb(0x811f3f))
    };
    let rules: [(&str, Color32); 12] = [
        ("comment, punctuation.definition.comment", comment),
        ("string, constant.other.symbol, string.quoted", string),
        ("constant.numeric, constant.language.boolean", number),
        ("keyword, storage, storage.type, storage.modifier, constant.language", keyword),
        ("keyword.control, keyword.operator.new, keyword.other.using", control),
        ("entity.name.function, support.function, meta.function-call", function),
        ("entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, support.type, support.class, entity.other.inherited-class", ty),
        ("variable, variable.parameter, variable.other, meta.definition.variable", variable),
        ("entity.name.tag, meta.tag", tag),
        ("entity.other.attribute-name, support.type.property-name, meta.object-literal.key", attr),
        ("string.regexp", regex),
        ("markup.heading, markup.bold", keyword),
    ];
    Theme {
        name: None,
        author: None,
        settings: ThemeSettings { foreground: Some(sc(text)), ..ThemeSettings::default() },
        scopes: rules
            .iter()
            .map(|(selector, color)| ThemeItem {
                scope: selector.parse::<ScopeSelectors>().expect("the selectors above are valid"),
                style: StyleModifier { foreground: Some(sc(*color)), background: None, font_style: None },
            })
            .collect(),
    }
}

/// The language of a file, by extension or name; None for plain text.
pub fn syntax_for(path: &str) -> Option<&'static SyntaxReference> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name.rsplit_once('.').map_or(name, |(_, e)| e);
    let s = SYNTAXES.find_syntax_by_extension(ext).or_else(|| SYNTAXES.find_syntax_by_extension(name)).or_else(|| SYNTAXES.find_syntax_by_extension(&ext.to_lowercase()))?;
    (s.name != "Plain Text").then_some(s)
}

/// A highlighter that carries its state from line to line.
pub struct Lines {
    inner: HighlightLines<'static>,
    /// A line was too long to colour, so the state after it is unknown: the rest stays plain.
    lost: bool,
}

impl Lines {
    pub fn new(syntax: &'static SyntaxReference, dark: bool) -> Lines {
        Lines { inner: HighlightLines::new(syntax, if dark { &DARK } else { &LIGHT }), lost: false }
    }

    /// Colours of one line (without its newline).
    pub fn line(&mut self, text: &str) -> Runs {
        if self.lost || text.len() > LENGTH_LIMIT {
            // The parser didn't read the long line (a comment or string may end in it), so
            // colouring later lines could be wrong: they stay plain text.
            self.lost = true;
            return Vec::new();
        }
        let with_newline = format!("{text}\n");
        let Ok(regions) = self.inner.highlight_line(&with_newline, &SYNTAXES) else { return Vec::new() };
        let mut at = 0;
        let mut runs = Vec::with_capacity(regions.len());
        for (style, piece) in regions {
            let end = (at + piece.len()).min(text.len());
            if end > at {
                let c = style.foreground;
                runs.push((Color32::from_rgb(c.r, c.g, c.b), at..end));
            }
            at += piece.len();
        }
        runs
    }
}

/// Colours for every line of `text`, or nothing when the file is too long or plain.
pub fn whole(path: &str, text: &str, dark: bool) -> Vec<Runs> {
    let Some(syntax) = syntax_for(path) else { return Vec::new() };
    if text.lines().take(LINE_LIMIT + 1).count() > LINE_LIMIT {
        return Vec::new();
    }
    let mut h = Lines::new(syntax, dark);
    text.lines().map(|l| h.line(l)).collect()
}

/// Visit comment byte ranges only as far as the last Unicode mark, with the usual
/// syntax work limits. Beyond those limits there is no syntax context to hide decorations.
pub(super) fn comment_spans(path: &str, text: &str, last: usize, mut mark: impl FnMut(usize, std::ops::Range<usize>)) {
    static COMMENT: LazyLock<ScopeSelectors> =
        LazyLock::new(|| "comment".parse().expect("valid scope selector"));
    let Some(syntax) = syntax_for(path) else { return };
    let mut parser = ParseState::new(syntax);
    let mut scopes = ScopeStack::new();
    for (row, line) in text.split_inclusive('\n').take((last + 1).min(LINE_LIMIT)).enumerate() {
        if line.len() > LENGTH_LIMIT {
            break;
        }
        let Ok(operations) = parser.parse_line(line, &SYNTAXES) else { break };
        let mut start = 0;
        for (end, operation) in operations {
            if end > start && COMMENT.does_match(scopes.as_slice()).is_some() {
                mark(row, start..end);
            }
            if scopes.apply(&operation).is_err() {
                return;
            }
            start = end;
        }
        if start < line.len() && COMMENT.does_match(scopes.as_slice()).is_some() {
            mark(row, start..line.len());
        }
    }
}

/// Byte offsets of matching delimiters, using the language grammar to ignore strings and comments.
pub fn brackets(path: &str, text: &str) -> BTreeMap<usize, usize> {
    static IGNORED: LazyLock<ScopeSelectors> =
        LazyLock::new(|| "comment, string".parse().expect("valid scope selector"));
    let syntax = syntax_for(path).unwrap_or_else(|| SYNTAXES.find_syntax_plain_text());
    let mut parser = ParseState::new(syntax);
    let mut scopes = ScopeStack::new();
    let mut stack = Vec::new();
    let mut pairs = BTreeMap::new();
    let mut offset = 0;
    for (row, line) in text.split_inclusive('\n').enumerate() {
        if row >= LINE_LIMIT || line.len() > LENGTH_LIMIT {
            break;
        }
        let Ok(operations) = parser.parse_line(line, &SYNTAXES) else { break };
        let mut from = 0;
        for (to, operation) in operations.into_iter().map(Some).chain(std::iter::once(None))
            .map(|event| event.map_or((line.len(), None), |(at, op)| (at, Some(op))))
        {
            if IGNORED.does_match(scopes.as_slice()).is_none() {
                for (i, byte) in line.as_bytes()[from..to].iter().copied().enumerate() {
                    let at = offset + from + i;
                    match byte {
                        b'(' | b'[' | b'{' => stack.push((byte, at)),
                        b')' | b']' | b'}' => {
                            let open = match byte { b')' => b'(', b']' => b'[', _ => b'{' };
                            if let Some((kind, first)) = stack.pop() && open == kind {
                                pairs.insert(first, at);
                                pairs.insert(at, first);
                            }
                        }
                        _ => {}
                    }
                }
            }
            if let Some(op) = operation && scopes.apply(&op).is_err() {
                return pairs;
            }
            from = to;
        }
        offset += line.len();
    }
    pairs
}

pub fn bracket_at(pairs: &BTreeMap<usize, usize>, caret: usize) -> Option<(usize, usize)> {
    pairs.get(&caret).map(|&other| (caret, other)).or_else(|| {
        let previous = caret.checked_sub(1)?;
        pairs.get(&previous).map(|&other| (previous, other))
    })
}

#[cfg(test)]
#[test]
fn headless_bracket_matching_skips_comments_and_strings() {
    let text = "fn café() { // {\n  let _ = \"[)]\";\n  run([1]);\n}\n";
    let pairs = brackets("code.rs", text);
    let block = text.find('{').unwrap();
    assert_eq!(bracket_at(&pairs, block), Some((block, text.rfind('}').unwrap())));
    assert!(!pairs.contains_key(&text.rfind('{').unwrap()));
    assert!(!pairs.contains_key(&text.find('[').unwrap()));
}

/// A layout job for one line: its colours, or `base` where it has none; `marks` gets a background.
pub fn job(text: &str, runs: Option<&Runs>, base: Color32, font: egui::FontId, marks: &[(std::ops::Range<usize>, Color32)]) -> egui::text::LayoutJob {
    use unicode_segmentation::UnicodeSegmentation;
    let mut job = egui::text::LayoutJob::default();
    // Sections end only between grapheme clusters, so an emoji sequence is never split between two sections (a mark
    // that starts or ends inside one covers all of it).
    let clusters: Option<Vec<usize>> = (!text.is_ascii()).then(|| text.grapheme_indices(true).map(|(at, _)| at).chain([text.len()]).collect());
    let floor = |at: usize| clusters.as_ref().map_or(at, |c| c[c.partition_point(|x| *x <= at).saturating_sub(1)]);
    let ceil = |at: usize| clusters.as_ref().map_or(at, |c| c.get(c.partition_point(|x| *x < at)).copied().unwrap_or(text.len()));
    let marks: Vec<(std::ops::Range<usize>, Color32)> = marks
        .iter()
        .filter(|(r, _)| r.start <= text.len() && text.is_char_boundary(r.start))
        .map(|(r, color)| (floor(r.start)..ceil(r.end.min(text.len())), *color))
        .collect();
    let mut cuts: Vec<usize> = vec![0, text.len()];
    if let Some(runs) = runs {
        for (_, r) in runs {
            cuts.extend([floor(r.start.min(text.len())), floor(r.end.min(text.len()))]);
        }
    }
    for (r, _) in &marks {
        cuts.extend([r.start, r.end]);
    }
    cuts.retain(|c| *c <= text.len() && text.is_char_boundary(*c));
    cuts.sort_unstable();
    cuts.dedup();
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a == b {
            continue;
        }
        let color = runs.and_then(|runs| runs.iter().find(|(_, r)| r.start <= a && a < r.end).map(|(c, _)| *c)).unwrap_or(base);
        let background = marks.iter().find(|(r, _)| r.start <= a && a < r.end).map_or(Color32::TRANSPARENT, |(_, c)| *c);
        job.append(&text[a..b], 0.0, egui::TextFormat { font_id: font.clone(), color, background, ..Default::default() });
    }
    if text.is_empty() {
        job.append("", 0.0, egui::TextFormat { font_id: font, color: base, ..Default::default() });
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_after_a_skipped_long_line_stay_plain() {
        // The comment closes on the long line; the code after it must not stay comment-coloured.
        let long = format!("{} */", "x".repeat(LENGTH_LIMIT + 10));
        let text = format!("/* start\n{long}\nlet a = 1;");
        let runs = whole("a.rs", &text, true);
        assert_eq!(runs.len(), 3);
        assert!(!runs[0].is_empty());
        assert!(runs[1].is_empty() && runs[2].is_empty());
        // Normal files are coloured as before.
        assert!(whole("a.rs", "let a = 1;\nlet b = 2;", true).iter().all(|r| !r.is_empty()));
    }

    #[test]
    fn colours_known_languages_and_covers_each_line_exactly() {
        let runs = whole("src/main.rs", "fn main() {\n    let x = \"grüße\";\n}", true);
        assert_eq!(runs.len(), 3);
        let line = "    let x = \"grüße\";";
        let covered: usize = runs[1].iter().map(|(_, r)| r.len()).sum();
        assert_eq!(covered, line.len());
        assert!(syntax_for("notes.txt").is_none());
    }
}
