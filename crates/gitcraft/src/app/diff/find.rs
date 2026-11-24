use super::{ACCENT, Color32, Icon, Ui, widgets};
use crate::app::widgets::Tip;
use regex::{Regex, RegexBuilder};
use std::ops::Range;

#[derive(Default)]
pub struct FileFind {
    pub open: bool,
    focus: bool,
    query: String,
    pattern: Option<Regex>,
    case: bool,
    whole: bool,
    regex: bool,
    invalid: bool,
    replace: bool,
    replacement: String,
    resume: Option<usize>,
    revision: Option<(u64, Option<u64>)>,
    hit: Option<Match>,
}

pub struct Match {
    bytes: Range<usize>,
    pub line: usize,
    in_line: Range<usize>,
}

impl Match {
    pub fn mark(&self, line: usize, text: &str) -> Option<(Range<usize>, Color32)> {
        (line == self.line && text.get(self.in_line.clone()).is_some())
            .then(|| (self.in_line.clone(), ACCENT.gamma_multiply(0.45)))
    }

    pub fn start(&self) -> usize {
        self.bytes.start
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }
}

impl FileFind {
    pub fn show(&mut self) {
        self.open = true;
        self.focus = true;
    }

    pub fn show_replace(&mut self) {
        self.show();
        self.replace = true;
    }

    pub fn source_changed(&mut self) {
        self.revision = None;
        self.resume = None;
    }

    pub fn replaced(&mut self, text: &str, at: usize) {
        self.hit = None;
        self.resume = Some(
            text.char_indices()
                .nth(at)
                .map_or(text.len(), |(byte, _)| byte),
        );
    }

    pub fn hit(&self) -> Option<&Match> {
        self.open.then_some(self.hit.as_ref()).flatten()
    }

    fn update_query(&mut self) {
        self.invalid = false;
        self.pattern = if self.query.is_empty() {
            None
        } else {
            let source = if self.regex {
                self.query.clone()
            } else {
                regex::escape(&self.query)
            };
            let source = if self.whole {
                format!(r"\b(?:{source})\b")
            } else {
                source
            };
            let pattern = RegexBuilder::new(&source)
                .case_insensitive(!self.case)
                .size_limit(2 * 1024 * 1024)
                .build()
                .ok();
            self.invalid = pattern.is_none();
            pattern
        };
    }

    fn select(&mut self, text: &str, step: i32) {
        let found = self
            .pattern
            .as_ref()
            .and_then(|pat| match (step, &self.hit) {
                (1, Some(hit)) => pat
                    .find_at(text, hit.bytes.end)
                    .filter(|next| next.range() != hit.bytes)
                    .or_else(|| {
                        pat.find_iter(text)
                            .find(|next| next.start() > hit.bytes.start)
                    })
                    .or_else(|| pat.find(text)),
                (-1, Some(hit)) => pat
                    .find_iter(text)
                    .take_while(|previous| previous.start() < hit.bytes.start)
                    .last()
                    .or_else(|| pat.find_iter(text).last()),
                (-1, None) => pat.find_iter(text).last(),
                _ => self
                    .resume
                    .take()
                    .and_then(|at| pat.find_at(text, at))
                    .or_else(|| pat.find(text)),
            });
        self.hit = found.map(|found| {
            let before = &text[..found.start()];
            let column = before.rsplit('\n').next().map_or(0, str::len);
            Match {
                bytes: found.range(),
                line: before.bytes().filter(|b| *b == b'\n').count(),
                in_line: column..column + found.len(),
            }
        });
    }

    pub fn bar(&mut self, ui: &mut Ui, text: &str, revision: (u64, Option<u64>)) -> bool {
        if !self.open {
            return false;
        }
        let mut reveal = false;
        ui.horizontal_wrapped(|ui| {
            ui.add_space(8.0);
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .char_limit(512)
                    .desired_width(180.0)
                    .hint_text("Find in file"),
            );
            if std::mem::take(&mut self.focus) {
                response.request_focus();
            }
            let enter = (response.has_focus() || response.lost_focus())
                && ui.input(|input| input.key_pressed(egui::Key::Enter));
            let prev = ui
                .button("Previous")
                .tip("Previous match (Shift+Enter)")
                .clicked();
            let next = ui.button("Next").tip("Next match (Enter)").clicked();
            let options_changed = ui
                .toggle_value(&mut self.case, "Aa")
                .tip("Match case")
                .changed()
                | ui.toggle_value(&mut self.whole, "Word")
                    .tip("Match whole word")
                    .changed()
                | ui.toggle_value(&mut self.regex, ".*")
                    .tip("Use regular expression")
                    .changed();
            if response.changed() || options_changed {
                self.resume = None;
                self.update_query();
            }
            let step = if prev || (enter && ui.input(|input| input.modifiers.shift)) {
                -1
            } else if next || enter {
                1
            } else {
                0
            };
            let changed = self.revision.replace(revision) != Some(revision)
                || response.changed()
                || options_changed;
            if changed || step != 0 {
                if changed {
                    self.hit = None;
                }
                self.select(text, if changed { 0 } else { step });
                reveal = self.hit.is_some();
            }
            if self.invalid {
                ui.label("Invalid regular expression");
            } else if self.hit.is_none() && !self.query.is_empty() {
                ui.label("No matches");
            }
            if widgets::icon_button(ui, Icon::X, 14.0, "Close find").clicked() {
                self.open = false;
            }
            if enter {
                response.request_focus();
            }
        });
        reveal && self.open
    }

    fn replace_text(&self, content: &str, all: bool) -> Option<(String, usize)> {
        let pattern = self.pattern.as_ref()?;
        let first = self.hit.as_ref()?;
        let matches: Box<dyn Iterator<Item = regex::Captures<'_>> + '_> = if all {
            Box::new(pattern.captures_iter(content))
        } else {
            Box::new(
                pattern
                    .captures_at(content, first.bytes.start)
                    .into_iter()
                    .filter(|caps| caps.get(0).is_some_and(|m| m.range() == first.bytes)),
            )
        };
        let mut result = String::with_capacity(content.len());
        let mut wrote = 0;
        let mut caret = first.bytes.start;
        for caps in matches {
            let hit = caps.get(0)?;
            result.push_str(&content[wrote..hit.start()]);
            if self.regex {
                caps.expand(&self.replacement, &mut result);
            } else {
                result.push_str(&self.replacement);
            }
            if wrote == 0 || hit.start() == first.bytes.start {
                caret = result.len();
            }
            wrote = hit.end();
            if result.len() > 8 * 1024 * 1024 {
                return None;
            }
        }
        result.push_str(&content[wrote..]);
        let changed = result != content;
        let caret = result[..caret.min(result.len())].chars().count();
        changed.then_some((result, caret))
    }

    pub fn replace_bar(&mut self, ui: &mut Ui, content: &str) -> Option<(String, usize)> {
        if !self.open {
            return None;
        }
        let mut result = None;
        ui.horizontal_wrapped(|ui| {
            ui.add_space(8.0);
            ui.toggle_value(&mut self.replace, "Replace")
                .tip("Show replacement controls");
            if self.replace {
                ui.add(
                    egui::TextEdit::singleline(&mut self.replacement)
                        .desired_width(180.0)
                        .char_limit(2048)
                        .hint_text("Replace with"),
                );
                let enabled = self.hit.is_some();
                let one = ui
                    .add_enabled(enabled, egui::Button::new("Replace Match"))
                    .clicked();
                let all = ui
                    .add_enabled(enabled, egui::Button::new("Replace All"))
                    .clicked();
                if one || all {
                    result = self.replace_text(content, all);
                }
            }
        });
        result
    }
}

#[cfg(test)]
#[test]
fn headless_find_replace_checks_whole_words_and_expands_regex_groups() {
    let mut find = FileFind {
        query: "undo".into(),
        whole: true,
        replacement: "$1".into(),
        ..Default::default()
    };
    find.update_query();
    find.select("Undo undoes undo", 0);
    assert_eq!(
        find.replace_text("Undo undoes undo", true).unwrap().0,
        "$1 undoes $1"
    );
    find.regex = true;
    find.whole = false;
    find.query = r"(\p{L}+)-(\d+)".into();
    find.replacement = "$2/$1".into();
    find.update_query();
    find.select("π-12", 0);
    assert_eq!(find.replace_text("π-12", false).unwrap().0, "12/π");
}

#[cfg(test)]
#[test]
fn headless_file_find_tracks_unicode_and_wraps() {
    let text = "π Grüße [?]\r\nSecond GRÜẞE [?]";
    let mut find = FileFind {
        query: "grüße [?]".into(),
        ..Default::default()
    };
    find.update_query();
    find.select(text, 0);
    let first = find.hit.as_ref().unwrap();
    assert_eq!(first.line, 0);
    assert_eq!(&text[first.bytes.clone()], "Grüße [?]");
    assert!(first.mark(0, text.lines().next().unwrap()).is_some());
    find.select(text, -1);
    let last = find.hit.as_ref().unwrap();
    assert_eq!(last.line, 1);
    assert_eq!(&text[last.bytes.clone()], "GRÜẞE [?]");
    find.select(text, 1);
    assert_eq!(find.hit.unwrap().line, 0);
}

#[cfg(test)]
#[test]
fn headless_file_find_bar_accepts_text_and_enter() {
    let ctx = egui::Context::default();
    let mut find = FileFind::default();
    find.show();
    let text = "one match\nanother match";
    let mut draw = |events| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| {
                find.bar(ui, text, (1, None));
            },
        );
        output.textures_delta.clear();
        find.hit().map(|hit| hit.line)
    };
    assert_eq!(draw(vec![]), None);
    assert_eq!(draw(vec![egui::Event::Text("match".into())]), Some(0));
    assert_eq!(
        draw(vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]),
        Some(1)
    );
}
