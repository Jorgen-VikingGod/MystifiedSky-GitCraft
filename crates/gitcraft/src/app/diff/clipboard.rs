use crate::app::highlight::Runs;
use egui::{Color32, Ui};
use std::cell::RefCell;
use std::fmt::Write;
use std::ops::Range;

thread_local! {
    // X11 and Wayland serve the clipboard from the app while it owns the selection.
    static CLIPBOARD: RefCell<Option<arboard::Clipboard>> = const { RefCell::new(None) };
}

pub(super) fn copy(ui: &Ui, text: &str, ranges: &[Range<usize>], runs: &[Runs], ink: Color32) {
    let mut plain = String::new();
    for piece in ranges.iter().filter_map(|r| text.get(r.clone())) {
        // Selections are separated by a line break, unless the one before already ends in one.
        if !plain.is_empty() && !plain.ends_with('\n') {
            plain.push('\n');
        }
        plain.push_str(piece);
    }
    if plain.is_empty() {
        return;
    }
    if runs.is_empty() || plain.len() > 256 * 1024 {
        ui.copy_text(plain);
        return;
    }
    let html = highlighted_html(text, ranges, runs, ink);
    let stored = CLIPBOARD.with_borrow_mut(|clipboard| {
        if clipboard.is_none() {
            *clipboard = arboard::Clipboard::new().ok();
        }
        clipboard.as_mut().is_some_and(|clipboard| {
            clipboard
                .set_html(html.as_str(), Some(plain.as_str()))
                .is_ok()
        })
    });
    if !stored {
        ui.copy_text(plain);
    }
}

fn highlighted_html(text: &str, ranges: &[Range<usize>], runs: &[Runs], ink: Color32) -> String {
    let mut html = String::from("<pre style=\"white-space:pre;font-family:monospace\"><code>");
    let mut row = 0;
    let mut start = 0;
    // Whether the selection before this one ended in a line break; `None` before the first.
    let mut ended_in_break: Option<bool> = None;
    for range in ranges {
        let Some(piece) = text.get(range.clone()) else { continue };
        if ended_in_break == Some(false) {
            html.push('\n');
        }
        ended_in_break = Some(piece.ends_with('\n'));
        if range.start < start {
            start = 0;
            row = 0;
        }
        while start < range.end {
            let next = text[start..]
                .find('\n')
                .map_or(text.len(), |n| start + n + 1);
            let from = range.start.max(start);
            let to = range.end.min(next);
            if from < to {
                let mut cursor = from;
                for (color, run) in runs.get(row).into_iter().flatten() {
                    let a = (start + run.start).max(from).max(cursor);
                    let b = (start + run.end).min(to);
                    if a >= b {
                        continue;
                    }
                    push_span(&mut html, &text[cursor..a], ink);
                    push_span(&mut html, &text[a..b], *color);
                    cursor = b;
                }
                push_span(&mut html, &text[cursor..to], ink);
            }
            if range.end <= next {
                break;
            }
            start = next;
            row += 1;
        }
    }
    html.push_str("</code></pre>");
    html
}

fn push_span(out: &mut String, text: &str, color: Color32) {
    if text.is_empty() {
        return;
    }
    let [r, g, b, _] = color.to_array();
    let _ = write!(out, "<span style=\"color:#{r:02x}{g:02x}{b:02x}\">");
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
    out.push_str("</span>");
}

#[cfg(test)]
#[test]
fn headless_rich_copy_preserves_selection_and_escapes_source() {
    let source = "skip\r\n<é>&\nlast";
    let range = source.find('<').unwrap()..source.find("last").unwrap();
    let syntax = vec![Vec::new(), vec![(Color32::RED, 1..3)]];
    let result = highlighted_html(source, &[range], &syntax, Color32::WHITE);
    assert!(result.contains("color:#ff0000\">é</span>"));
    assert!(result.contains("&gt;&amp;\n"));
    assert!(result.contains("&lt;"));
    assert!(!result.contains("skip") && !result.contains("last"));
}
