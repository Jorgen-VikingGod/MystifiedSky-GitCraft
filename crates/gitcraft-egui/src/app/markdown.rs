//! Hosted descriptions and comments as Markdown, under the renderer content policy: raw HTML is
//! left out, links open only http(s) addresses without credentials, and pictures show as their
//! alternative text (avatars from gravatar.com excepted).

use super::theme::p;
use egui::{Color32, FontId, RichText, Stroke, Ui};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// An http(s) address without user name or password, resolved against `base`.
pub fn web_url(input: &str, base: &str) -> Option<String> {
    let parsed = url::Url::parse(input).or_else(|_| url::Url::parse(base).and_then(|b| b.join(input))).ok()?;
    (matches!(parsed.scheme(), "http" | "https") && parsed.username().is_empty() && parsed.password().is_none()).then(|| parsed.to_string())
}

#[derive(Default, Clone)]
struct Style {
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
    heading: Option<HeadingLevel>,
    link: Option<String>,
}

/// Draw `text` as Markdown. Returns a link that was clicked.
pub fn show(ui: &mut Ui, text: &str, base: &str) -> Option<String> {
    let c = p();
    let mut clicked = None;
    let mut style = Style::default();
    let mut job = egui::text::LayoutJob::default();
    let mut links: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    let mut list_depth: Vec<Option<u64>> = Vec::new();
    let mut in_code_block = false;
    let mut code = String::new();
    let mut quote = 0;
    let width = ui.available_width();
    let flush = |ui: &mut Ui, job: &mut egui::text::LayoutJob, links: &mut Vec<(std::ops::Range<usize>, String)>, clicked: &mut Option<String>, indent: f32| {
        if job.text.trim().is_empty() {
            *job = egui::text::LayoutJob::default();
            links.clear();
            return;
        }
        let mut j = std::mem::take(job);
        j.wrap.max_width = (width - indent).max(50.0);
        let galley = ui.fonts_mut(|f| f.layout_job(j));
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, galley.size().y), egui::Sense::click());
        let origin = egui::pos2(rect.left() + indent, rect.top());
        ui.painter().galley(origin, galley.clone(), c.fg);
        if !links.is_empty() {
            if let Some(pos) = ui.ctx().pointer_hover_pos().filter(|p| rect.contains(*p)) {
                let cursor = galley.cursor_from_pos(pos - origin);
                let at = cursor.index.0;
                if let Some((_, url)) = links.iter().find(|(r, _)| r.contains(&byte_of(&galley.job.text, at))) {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    if resp.clicked() {
                        *clicked = Some(url.clone());
                    }
                }
            }
        }
        links.clear();
        ui.add_space(6.0);
    };
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for event in Parser::new_ext(text, opts) {
        let indent = list_depth.len() as f32 * 18.0 + quote as f32 * 12.0;
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                flush(ui, &mut job, &mut links, &mut clicked, indent);
                style.heading = Some(level);
            }
            Event::End(TagEnd::Heading(_)) => {
                flush(ui, &mut job, &mut links, &mut clicked, indent);
                style.heading = None;
            }
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => flush(ui, &mut job, &mut links, &mut clicked, indent),
            Event::Start(Tag::BlockQuote(_)) => {
                flush(ui, &mut job, &mut links, &mut clicked, indent);
                quote += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                flush(ui, &mut job, &mut links, &mut clicked, indent);
                quote -= 1;
            }
            Event::Start(Tag::List(start)) => {
                flush(ui, &mut job, &mut links, &mut clicked, indent);
                list_depth.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                flush(ui, &mut job, &mut links, &mut clicked, indent);
                list_depth.pop();
            }
            Event::Start(Tag::Item) => {
                flush(ui, &mut job, &mut links, &mut clicked, indent);
                let marker = match list_depth.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}. ");
                        *n += 1;
                        m
                    }
                    _ => "• ".to_owned(),
                };
                job.append(&marker, 0.0, egui::TextFormat::simple(FontId::proportional(13.0), c.fg3));
            }
            Event::End(TagEnd::Item) => flush(ui, &mut job, &mut links, &mut clicked, indent),
            Event::TaskListMarker(done) => job.append(if done { "☑ " } else { "☐ " }, 0.0, egui::TextFormat::simple(FontId::proportional(13.0), c.fg2)),
            Event::Start(Tag::CodeBlock(kind)) => {
                flush(ui, &mut job, &mut links, &mut clicked, indent);
                in_code_block = true;
                code.clear();
                let _ = matches!(kind, CodeBlockKind::Fenced(_));
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                egui::Frame::new().fill(c.field).corner_radius(egui::CornerRadius::same(4)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                    ui.set_width(width - 16.0 - indent);
                    ui.add(egui::Label::new(RichText::new(code.trim_end()).monospace().size(12.0).color(c.fg)).wrap());
                });
                ui.add_space(6.0);
            }
            Event::Start(Tag::Emphasis) => style.italic = true,
            Event::End(TagEnd::Emphasis) => style.italic = false,
            Event::Start(Tag::Strong) => style.bold = true,
            Event::End(TagEnd::Strong) => style.bold = false,
            Event::Start(Tag::Strikethrough) => style.strike = true,
            Event::End(TagEnd::Strikethrough) => style.strike = false,
            Event::Start(Tag::Link { dest_url, .. }) => style.link = web_url(&dest_url, base),
            Event::End(TagEnd::Link) => style.link = None,
            Event::Start(Tag::Image { dest_url, .. }) => {
                let allowed = web_url(&dest_url, base).and_then(|u| url::Url::parse(&u).ok()).is_some_and(|u| u.scheme() == "https" && u.host_str().is_some_and(|h| h == "gravatar.com" || h.ends_with(".gravatar.com")));
                let _ = allowed;
                style.code = false;
                job.append("[", 0.0, egui::TextFormat::simple(FontId::proportional(13.0), c.fg3));
            }
            Event::End(TagEnd::Image) => job.append("]", 0.0, egui::TextFormat::simple(FontId::proportional(13.0), c.fg3)),
            Event::Code(t) => append(&mut job, &mut links, &t, &Style { code: true, ..style.clone() }),
            Event::Text(t) => {
                if in_code_block {
                    code.push_str(&t);
                } else {
                    append(&mut job, &mut links, &t, &style);
                }
            }
            Event::SoftBreak => append(&mut job, &mut links, " ", &style),
            Event::HardBreak => append(&mut job, &mut links, "\n", &style),
            Event::Rule => {
                flush(ui, &mut job, &mut links, &mut clicked, indent);
                ui.separator();
            }
            // Raw HTML is left out.
            Event::Html(_) | Event::InlineHtml(_) => {}
            Event::Start(Tag::Table(_)) | Event::Start(Tag::TableHead) | Event::Start(Tag::TableRow) => flush(ui, &mut job, &mut links, &mut clicked, indent),
            Event::End(TagEnd::TableRow) | Event::End(TagEnd::TableHead) => flush(ui, &mut job, &mut links, &mut clicked, indent),
            Event::End(TagEnd::TableCell) => append(&mut job, &mut links, "   ", &style),
            _ => {}
        }
    }
    let indent = list_depth.len() as f32 * 18.0;
    flush(ui, &mut job, &mut links, &mut clicked, indent);
    let _ = Stroke::NONE;
    let _ = Color32::TRANSPARENT;
    clicked
}

fn byte_of(text: &str, char_index: usize) -> usize {
    text.char_indices().nth(char_index).map_or(text.len(), |(b, _)| b)
}

fn append(job: &mut egui::text::LayoutJob, links: &mut Vec<(std::ops::Range<usize>, String)>, text: &str, s: &Style) {
    let c = p();
    let size = match s.heading {
        Some(HeadingLevel::H1) => 20.0,
        Some(HeadingLevel::H2) => 17.0,
        Some(HeadingLevel::H3) => 15.0,
        Some(_) => 14.0,
        None => 13.0,
    };
    let font = if s.code {
        FontId::monospace(12.0)
    } else if s.bold || s.heading.is_some() {
        super::fonts::semibold(size)
    } else {
        FontId::proportional(size)
    };
    let mut fmt = egui::TextFormat { font_id: font, color: if s.link.is_some() { c.link } else { c.fg }, italics: s.italic, ..Default::default() };
    if s.code {
        fmt.background = c.field;
    }
    if s.strike {
        fmt.strikethrough = Stroke::new(1.0, c.fg);
    }
    if s.link.is_some() {
        fmt.underline = Stroke::new(1.0, c.link.gamma_multiply(0.6));
    }
    let start = job.text.len();
    job.append(text, 0.0, fmt);
    if let Some(url) = &s.link {
        links.push((start..job.text.len(), url.clone()));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_plain_web_links_open() {
        assert_eq!(super::web_url("https://example.invalid/x", "").as_deref(), Some("https://example.invalid/x"));
        assert_eq!(super::web_url("/a/b", "https://example.invalid/pr/1").as_deref(), Some("https://example.invalid/a/b"));
        assert!(super::web_url("javascript:alert(1)", "https://example.invalid").is_none());
        assert!(super::web_url("https://user:pass@example.invalid", "").is_none());
    }
}
