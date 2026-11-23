//! Syntax colours for diffs, the file viewer and the editor, in the editor colours the desktop
//! app has always used (Dark+ and Light+).

use egui::Color32;
use std::sync::LazyLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Color as SyntaxColor, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings};
use syntect::parsing::{SyntaxReference, SyntaxSet};

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
    let path = path.split('#').next().unwrap_or(path);
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name.rsplit_once('.').map_or(name, |(_, e)| e);
    let s = SYNTAXES.find_syntax_by_extension(ext).or_else(|| SYNTAXES.find_syntax_by_extension(name)).or_else(|| SYNTAXES.find_syntax_by_extension(&ext.to_lowercase()))?;
    (s.name != "Plain Text").then_some(s)
}

/// A highlighter that carries its state from line to line.
pub struct Lines {
    inner: HighlightLines<'static>,
}

impl Lines {
    pub fn new(syntax: &'static SyntaxReference, dark: bool) -> Lines {
        Lines { inner: HighlightLines::new(syntax, if dark { &DARK } else { &LIGHT }) }
    }

    /// Colours of one line (without its newline).
    pub fn line(&mut self, text: &str) -> Runs {
        if text.len() > LENGTH_LIMIT {
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
    if text.lines().count() > LINE_LIMIT {
        return Vec::new();
    }
    let mut h = Lines::new(syntax, dark);
    text.lines().map(|l| h.line(l)).collect()
}

/// A layout job for one line: its colours, or `base` where it has none; `marks` gets a background.
pub fn job(text: &str, runs: Option<&Runs>, base: Color32, font: egui::FontId, marks: &[(std::ops::Range<usize>, Color32)]) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let mut cuts: Vec<usize> = vec![0, text.len()];
    if let Some(runs) = runs {
        for (_, r) in runs {
            cuts.extend([r.start, r.end]);
        }
    }
    for (r, _) in marks {
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
    fn colours_known_languages_and_covers_each_line_exactly() {
        let runs = whole("src/main.rs", "fn main() {\n    let x = \"grüße\";\n}", true);
        assert_eq!(runs.len(), 3);
        let line = "    let x = \"grüße\";";
        let covered: usize = runs[1].iter().map(|(_, r)| r.len()).sum();
        assert_eq!(covered, line.len());
        assert!(syntax_for("notes.txt").is_none());
    }
}
