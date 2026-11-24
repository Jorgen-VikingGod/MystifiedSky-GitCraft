//! The in-app terminal under the graph: terminal tabs, each a shell in the repository folder,
//! drawn from alacritty_terminal's character grid in GitCraft's terminal colours.

use super::widgets::Tip;
use super::App;
use super::ctxmenu::Later;
use super::icons::{self, Icon};
use super::repo::RepoTab;
use super::theme::{ACCENT, p};
use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Boundary, Column, Direction, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::search::RegexSearch;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Processor, Rgb};
use egui::{Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Output that arrived from the shells, by terminal id, not yet drawn.
pub type Pending = Arc<Mutex<HashMap<u32, Vec<u8>>>>;

/// The terminal's colour palette.
const PALETTE: [u32; 16] = [0x000000, 0xF24A4A, 0x0DBC79, 0xE5E510, 0x4A98EE, 0xE063E0, 0x11A8CD, 0xE5E5E5, 0x686868, 0xFF5656, 0x23D18B, 0xF5F543, 0x51A4FF, 0xD670D6, 0x29B8DB, 0xE5E5E5];
const FG: Color32 = Color32::WHITE;
const BG: Color32 = Color32::BLACK;
const SELECTION: Color32 = Color32::from_rgb(0x30, 0x46, 0x76);

#[derive(Clone)]
struct Listener {
    replies: Arc<Mutex<Vec<String>>>,
    title: Arc<Mutex<Option<String>>>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(text) => self.replies.lock().unwrap_or_else(|e| e.into_inner()).push(text),
            Event::Title(t) => *self.title.lock().unwrap_or_else(|e| e.into_inner()) = Some(t),
            _ => {}
        }
    }
}

struct Size {
    cols: usize,
    rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

pub struct Session {
    pub id: Option<u32>,
    pub title: String,
    term: Term<Listener>,
    parser: Processor,
    listener: Listener,
    pub exited: bool,
    cols: usize,
    rows: usize,
    /// A command to type once the shell starts (coding agent sessions).
    run: Option<String>,
    cwd: String,
    env: Option<HashMap<String, String>>,
    error: Option<String>,
    /// Why the last input could not be sent (the shell closed, its input is full).
    input_error: Option<String>,
}

impl Session {
    fn new(title: String, cwd: String, run: Option<String>, env: Option<HashMap<String, String>>) -> Session {
        let listener = Listener { replies: Default::default(), title: Default::default() };
        let term = Term::new(Config { scrolling_history: 10_000, ..Config::default() }, &Size { cols: 80, rows: 24 }, listener.clone());
        Session { id: None, title, term, parser: Processor::new(), listener, exited: false, cols: 80, rows: 24, run, cwd, env, error: None, input_error: None }
    }
}

#[derive(Default)]
pub struct Panel {
    pub sessions: Vec<Session>,
    pub active: usize,
    next: usize,
    focused: bool,
    /// The find bar over the screen (Ctrl+Shift+F), while it is open.
    find: Option<Find>,
}

#[derive(Default)]
struct Find {
    needle: String,
    /// Nothing matched the last search.
    missing: bool,
    /// Put the keyboard in the field on the next frame.
    focus: bool,
}

/// A search for `needle` as plain text, ignoring case.
fn search_for(needle: &str) -> Option<RegexSearch> {
    let mut pattern = String::from("(?i)");
    for ch in needle.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(ch) {
            pattern.push('\\');
        }
        pattern.push(ch);
    }
    RegexSearch::new(&pattern).ok()
}

/// Select and show the next match of `needle` after the current one (`back`: before it).
/// Starts from the newest output; the search wraps around. Returns whether it found one.
fn find_in<T: EventListener>(term: &mut Term<T>, needle: &str, back: bool, again: bool) -> bool {
    let Some(mut regex) = search_for(needle).filter(|_| !needle.is_empty()) else {
        term.selection = None;
        return !needle.is_empty();
    };
    let current = term.selection.as_ref().and_then(|s| s.to_range(term));
    let last = Point::new(Line(term.screen_lines() as i32 - 1), Column(term.columns() - 1));
    // Typing more tries again from where the shown match starts; Next and Previous step from the
    // cell after or before it. Without a match shown, the search goes up from the newest line.
    let (origin, direction) = match (&current, again, back) {
        (Some(r), false, _) => (r.start, Direction::Right),
        (Some(r), true, false) => (r.end.add(term, Boundary::None, 1), Direction::Right),
        (Some(r), true, true) => (r.start.sub(term, Boundary::None, 1), Direction::Left),
        (None, _, _) => (last, Direction::Left),
    };
    let side = if direction == Direction::Left { Side::Right } else { Side::Left };
    let found = term.search_next(&mut regex, origin, direction, side, None);
    match found {
        Some(m) => {
            let mut sel = Selection::new(SelectionType::Simple, *m.start(), Side::Left);
            sel.update(*m.end(), Side::Right);
            term.selection = Some(sel);
            term.scroll_to_point(*m.start());
            true
        }
        None => {
            term.selection = None;
            false
        }
    }
}

fn rgb(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

fn color(c: Color, colors: &alacritty_terminal::term::color::Colors, fg: bool) -> Color32 {
    let from = |r: Rgb| Color32::from_rgb(r.r, r.g, r.b);
    match c {
        Color::Spec(r) => from(r),
        Color::Indexed(i) => {
            if let Some(r) = colors[i as usize] {
                return from(r);
            }
            match i {
                0..=15 => rgb(PALETTE[i as usize]),
                16..=231 => {
                    let i = i - 16;
                    let step = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                    Color32::from_rgb(step(i / 36), step((i / 6) % 6), step(i % 6))
                }
                _ => {
                    let g = 8 + (i - 232) * 10;
                    Color32::from_gray(g)
                }
            }
        }
        Color::Named(n) => {
            let i = n as usize;
            if let Some(r) = colors[i] {
                return from(r);
            }
            match n {
                NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::Cursor => FG,
                NamedColor::Background => BG,
                NamedColor::DimForeground => FG.gamma_multiply(0.7),
                _ if i < 16 => rgb(PALETTE[i]),
                _ if (256 + 3..256 + 11).contains(&i) => rgb(PALETTE[i - 259]).gamma_multiply(0.7),
                _ => if fg { FG } else { BG },
            }
        }
    }
}

/// The bytes a key press sends to the shell.
fn key_bytes(key: egui::Key, mods: egui::Modifiers, app_cursor: bool) -> Option<Vec<u8>> {
    use egui::Key as K;
    let csi = |s: &str| Some(format!("\x1b[{s}").into_bytes());
    let arrow = |c: char| if app_cursor { Some(format!("\x1bO{c}").into_bytes()) } else { Some(format!("\x1b[{c}").into_bytes()) };
    if mods.ctrl && !mods.alt {
        if let Some(ch) = key.name().chars().next().filter(|_| key.name().len() == 1)
            && ch.is_ascii_alphabetic()
        {
            return Some(vec![(ch.to_ascii_uppercase() as u8) - b'A' + 1]);
        }
    }
    match key {
        K::Enter => Some(b"\r".to_vec()),
        K::Backspace => Some(if mods.ctrl { vec![0x08] } else { vec![0x7f] }),
        K::Tab => Some(if mods.shift { b"\x1b[Z".to_vec() } else { b"\t".to_vec() }),
        K::Escape => Some(b"\x1b".to_vec()),
        K::ArrowUp => arrow('A'),
        K::ArrowDown => arrow('B'),
        K::ArrowRight => arrow('C'),
        K::ArrowLeft => arrow('D'),
        K::Home => arrow('H'),
        K::End => arrow('F'),
        K::Insert => csi("2~"),
        K::Delete => csi("3~"),
        K::PageUp => csi("5~"),
        K::PageDown => csi("6~"),
        K::F1 => Some(b"\x1bOP".to_vec()),
        K::F2 => Some(b"\x1bOQ".to_vec()),
        K::F3 => Some(b"\x1bOR".to_vec()),
        K::F4 => Some(b"\x1bOS".to_vec()),
        _ => None,
    }
}

/// A web address under column `col` of `line`, if any.
/// A row's cells (column, whether it is drawn on its own) grouped into runs drawn as one piece of
/// text: neighbouring ordinary cells. A wide character or one with combining marks is a run of
/// its own, and a gap in the columns starts a new run.
fn cell_runs(cells: &[(usize, bool)]) -> Vec<std::ops::Range<usize>> {
    let mut runs: Vec<std::ops::Range<usize>> = Vec::new();
    for (i, &(col, alone)) in cells.iter().enumerate() {
        let joins = !alone && i > 0 && !cells[i - 1].1 && cells[i - 1].0 + 1 == col && runs.last().is_some_and(|r| r.end == i);
        if joins {
            runs.last_mut().expect("run").end = i + 1;
        } else {
            runs.push(i..i + 1);
        }
    }
    runs
}

pub fn link_at(line: &str, col: usize) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();
    let text: String = chars.iter().collect();
    for start in ["https://", "http://"] {
        let mut from = 0;
        while let Some(i) = text[from..].find(start) {
            let s = from + i;
            let begin = text[..s].chars().count();
            let mut end = begin;
            while end < chars.len() && !chars[end].is_whitespace() && !"\"'<>`".contains(chars[end]) {
                end += 1;
            }
            while end > begin && ".,;:)]}".contains(chars[end - 1]) {
                end -= 1;
            }
            if (begin..end).contains(&col) {
                return Some(chars[begin..end].iter().collect());
            }
            from = s + start.len();
        }
    }
    None
}

impl App {
    /// Start a shell for a session that has none yet.
    fn start_session(&mut self, tab: u64, index: usize) {
        let shell = self.settings.string("termShell", "");
        let Some(panel) = self.term_panels.get_mut(&tab) else { return };
        let Some(s) = panel.sessions.get_mut(index) else { return };
        let pending = self.term_pending.clone();
        let ctx = self.ctx.clone();
        let ctx2 = self.ctx.clone();
        let exits = self.term_exits.clone();
        let result = self.terminals.open(
            "main",
            || true,
            &shell,
            &s.cwd,
            s.cols as u16,
            s.rows as u16,
            s.env.as_ref(),
            Arc::new(move |id, data| {
                pending.lock().unwrap_or_else(|e| e.into_inner()).entry(id).or_default().extend_from_slice(data.as_bytes());
                ctx.request_repaint();
            }),
            Arc::new(move |id| {
                exits.lock().unwrap_or_else(|e| e.into_inner()).push(id);
                ctx2.request_repaint();
            }),
        );
        match result {
            Ok(id) => {
                s.id = Some(id);
                if let Some(cmd) = s.run.take()
                    && let Err(e) = self.terminals.write("main", id, &format!("{cmd}\r"))
                {
                    s.input_error = Some(e);
                }
            }
            Err(e) => s.error = Some(e),
        }
    }

    /// Open a terminal tab that types `command` once its shell starts.
    pub(crate) fn run_in_terminal(&mut self, tab: u64, title: &str, command: &str, cwd: &str, env: Option<HashMap<String, String>>) {
        self.settings.set("showTerminal", true);
        let panel = self.term_panels.entry(tab).or_default();
        panel.next += 1;
        panel.sessions.push(Session::new(title.to_owned(), cwd.to_owned(), Some(command.to_owned()), env));
        panel.active = panel.sessions.len() - 1;
    }

    /// Feed what the shells wrote into their screens.
    pub(super) fn pump_terminals(&mut self) {
        let mut pending = std::mem::take(&mut *self.term_pending.lock().unwrap_or_else(|e| e.into_inner()));
        let exits = std::mem::take(&mut *self.term_exits.lock().unwrap_or_else(|e| e.into_inner()));
        for panel in self.term_panels.values_mut() {
            for s in &mut panel.sessions {
                let Some(id) = s.id else { continue };
                if let Some(bytes) = pending.remove(&id) {
                    s.parser.advance(&mut s.term, &bytes);
                    self.terminals.acknowledge("main", id, bytes.len());
                    for reply in std::mem::take(&mut *s.listener.replies.lock().unwrap_or_else(|e| e.into_inner())) {
                        let _ = self.terminals.write("main", id, &reply);
                    }
                    // Tabs keep their own names.
                    s.listener.title.lock().unwrap_or_else(|e| e.into_inner()).take();
                }
                if exits.contains(&id) {
                    s.exited = true;
                }
            }
        }
    }

    pub(crate) fn terminal_panel(&mut self, ui: &mut Ui, tab: u64, r: &RepoTab, later: &mut Vec<Later>) {
        let c = p();
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, CornerRadius::ZERO, BG);
        ui.painter().line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, c.white(0.15)));
        let cwd = r.path.clone();
        let panel = self.term_panels.entry(tab).or_default();
        if panel.sessions.is_empty() {
            panel.next += 1;
            panel.sessions.push(Session::new(format!("Terminal {}", panel.next), cwd.clone(), None, None));
            panel.active = 0;
        }
        // Tabs.
        let bar = Rect::from_min_size(rect.min, vec2(rect.width(), 26.0));
        ui.painter().rect_filled(bar, CornerRadius::ZERO, c.panel);
        ui.painter().line_segment([bar.left_bottom(), bar.right_bottom()], Stroke::new(1.0, c.black(0.4)));
        icons::paint(ui, Rect::from_min_size(pos2(bar.left() + 8.0, bar.center().y - 6.5), Vec2::splat(13.0)), Icon::SquareTerminal, 13.0, c.fg2);
        let mut x = bar.left() + 29.0;
        let mut close: Option<usize> = None;
        let mut add = false;
        for (i, s) in panel.sessions.iter().enumerate() {
            let g = ui.painter().layout_no_wrap(s.title.clone(), FontId::proportional(12.0), c.fg);
            let w = g.size().x + 10.0 + 22.0;
            let tr = Rect::from_min_size(pos2(x, bar.top()), vec2(w, 26.0));
            let resp = ui.interact(tr, ui.scope_id().with(("term_tab", i)), Sense::click());
            let on = panel.active == i;
            if on {
                ui.painter().rect_filled(tr, CornerRadius::ZERO, BG);
            }
            ui.painter().galley(pos2(tr.left() + 10.0, tr.center().y - g.size().y / 2.0), g, if on || resp.hovered() { c.white(1.0) } else { c.fg2 });
            let xr = Rect::from_min_size(pos2(tr.right() - 20.0, tr.center().y - 7.0), Vec2::splat(14.0));
            let xresp = ui.interact(xr, ui.scope_id().with(("term_close", i)), Sense::click());
            if resp.hovered() || on || xresp.hovered() {
                icons::paint(ui, xr, Icon::X, 12.0, if xresp.hovered() { c.white(1.0) } else { c.fg3 });
            }
            ui.painter().line_segment([tr.right_top(), tr.right_bottom()], Stroke::new(1.0, c.black(0.3)));
            if xresp.clicked() {
                close = Some(i);
            } else if resp.clicked() {
                panel.active = i;
            }
            x += w;
        }
        let plus = Rect::from_min_size(pos2(x + 2.0, bar.top() + 2.0), Vec2::splat(22.0));
        let presp = ui.interact(plus, ui.scope_id().with("term_add"), Sense::click()).tip("New terminal");
        icons::paint(ui, plus, Icon::Plus, 13.0, if presp.hovered() { c.white(1.0) } else { c.fg2 });
        if presp.clicked() {
            add = true;
        }
        let find = Rect::from_min_size(pos2(bar.right() - 50.0, bar.top() + 2.0), Vec2::splat(22.0));
        let fresp = ui.interact(find, ui.scope_id().with("term_find"), Sense::click()).tip("Find in terminal (Ctrl+Shift+F)");
        icons::paint(ui, find, Icon::Search, 13.0, if fresp.hovered() || panel.find.is_some() { c.white(1.0) } else { c.fg2 });
        if fresp.clicked() {
            panel.find.get_or_insert_with(Find::default).focus = true;
        }
        let hide = Rect::from_min_size(pos2(bar.right() - 26.0, bar.top() + 2.0), Vec2::splat(22.0));
        let hresp = ui.interact(hide, ui.scope_id().with("term_hide"), Sense::click()).tip("Hide the terminal (Ctrl+`)");
        icons::paint(ui, hide, Icon::ChevronDown, 14.0, if hresp.hovered() { c.white(1.0) } else { c.fg2 });
        if hresp.clicked() {
            later.push(Box::new(|app| app.settings.set("showTerminal", false)));
        }
        if add {
            panel.next += 1;
            panel.sessions.push(Session::new(format!("Terminal {}", panel.next), cwd.clone(), None, None));
            panel.active = panel.sessions.len() - 1;
        }
        if let Some(i) = close {
            let s = panel.sessions.remove(i);
            if let Some(id) = s.id {
                self.terminals.close("main", id);
            }
            let panel = self.term_panels.get_mut(&tab).expect("panel");
            if panel.sessions.is_empty() {
                later.push(Box::new(|app| app.settings.set("showTerminal", false)));
                return;
            }
            panel.active = panel.active.min(panel.sessions.len() - 1);
        }
        let panel = self.term_panels.get_mut(&tab).expect("panel");
        let index = panel.active;
        // The screen.
        let screen = Rect::from_min_max(pos2(rect.left() + 6.0, bar.bottom() + 4.0), pos2(rect.right() - 6.0, rect.bottom() - 4.0));
        let size = self.settings.num("termFontSize", 12.0) as f32;
        let line_height = (self.settings.num("termLineHeight", 1.2) as f32).clamp(1.0, 2.0);
        let family = self.settings.string("termFontFamily", "");
        let font = super::fonts::preferred(ui.ctx(), &family, size);
        let cell_w = ui.fonts_mut(|f| f.glyph_width(&font, 'M')).max(1.0);
        let cell_h = (size * line_height).round().max(1.0);
        let cols = ((screen.width() / cell_w).floor() as usize).max(10);
        let rows = ((screen.height() / cell_h).floor() as usize).max(2);
        let needs_start = {
            let s = &mut panel.sessions[index];
            if s.cols != cols || s.rows != rows {
                s.cols = cols;
                s.rows = rows;
                s.term.resize(Size { cols, rows });
                if let Some(id) = s.id {
                    let _ = self.terminals.resize("main", id, cols as u16, rows as u16);
                }
            }
            s.id.is_none() && s.error.is_none() && !s.exited
        };
        if needs_start {
            self.start_session(tab, index);
        }
        let panel = self.term_panels.get_mut(&tab).expect("panel");
        let s = &mut panel.sessions[index];
        let id = ui.scope_id().with(("term_screen", tab, index));
        let resp = ui.interact(screen, id, Sense::click_and_drag());
        if resp.clicked() || resp.drag_started() || resp.secondary_clicked() {
            resp.request_focus();
        }
        let focused = ui.memory(|m| m.has_focus(id));
        panel.focused = focused;
        if focused {
            ui.memory_mut(|m| m.set_focus_lock_filter(id, egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true }));
        }
        // Input.
        if focused && let Some(tid) = s.id {
            let app_cursor = s.term.mode().contains(TermMode::APP_CURSOR);
            let events = ui.input(|i| i.events.clone());
            let mut out = String::new();
            for e in events {
                match e {
                    egui::Event::Text(t) => out.push_str(&t),
                    egui::Event::Paste(t) => {
                        if s.term.mode().contains(TermMode::BRACKETED_PASTE) {
                            out.push_str(&format!("\x1b[200~{t}\x1b[201~"));
                        } else {
                            out.push_str(&t.replace("\r\n", "\r").replace('\n', "\r"));
                        }
                    }
                    // Cmd+C on macOS copies; elsewhere Ctrl+C goes to the shell (Ctrl+Shift+C copies).
                    egui::Event::Copy => {
                        if cfg!(target_os = "macos")
                            && let Some(text) = s.term.selection_to_string()
                        {
                            ui.ctx().copy_text(text);
                        }
                    }
                    egui::Event::Key { key, pressed: true, modifiers, .. } => {
                        if modifiers.ctrl && modifiers.shift && key == egui::Key::C {
                            if let Some(text) = s.term.selection_to_string() {
                                ui.ctx().copy_text(text);
                            }
                            continue;
                        }
                        if modifiers.command && modifiers.shift && key == egui::Key::F {
                            panel.find.get_or_insert_with(Find::default).focus = true;
                            continue;
                        }
                        if let Some(b) = key_bytes(key, modifiers, app_cursor) {
                            out.push_str(&String::from_utf8_lossy(&b));
                        }
                    }
                    _ => {}
                }
            }
            if !out.is_empty() {
                s.term.scroll_display(Scroll::Bottom);
                s.input_error = self.terminals.write("main", tid, &out).err();
            }
        }
        // Scrollback.
        if resp.hovered() {
            let dy = ui.input(|i| i.smooth_scroll_delta.y);
            if dy.abs() > 0.5 {
                let lines = (dy / cell_h).round() as i32;
                if lines != 0 {
                    s.term.scroll_display(Scroll::Delta(lines));
                }
            }
        }
        // Selecting with the pointer: drag for characters, double-click for a word, triple-click
        // for lines; dragging past the top or bottom scrolls. Ctrl/Cmd+click is for links.
        if !ui.input(|i| i.modifiers.command) {
            let at = |pos: egui::Pos2, term: &Term<Listener>| -> (Point, Side) {
                let offset = term.grid().display_offset() as i32;
                let row = ((pos.y - screen.top()) / cell_h).floor().clamp(0.0, rows as f32 - 1.0) as i32;
                let x = ((pos.x - screen.left()) / cell_w).max(0.0);
                let col = (x.floor() as usize).min(cols - 1);
                (Point::new(Line(row - offset), Column(col)), if x - col as f32 > 0.5 { Side::Right } else { Side::Left })
            };
            let pos = resp.interact_pointer_pos();
            if let Some(pos) = pos {
                let (point, side) = at(pos, &s.term);
                if resp.triple_clicked() {
                    s.term.selection = Some(Selection::new(SelectionType::Lines, point, side));
                } else if resp.double_clicked() {
                    s.term.selection = Some(Selection::new(SelectionType::Semantic, point, side));
                } else if resp.drag_started_by(egui::PointerButton::Primary) {
                    s.term.selection = Some(Selection::new(SelectionType::Simple, point, side));
                } else if resp.dragged_by(egui::PointerButton::Primary) {
                    if pos.y < screen.top() {
                        s.term.scroll_display(Scroll::Delta(1));
                    } else if pos.y > screen.bottom() {
                        s.term.scroll_display(Scroll::Delta(-1));
                    }
                    let (point, side) = at(pos, &s.term);
                    if let Some(sel) = s.term.selection.as_mut() {
                        sel.update(point, side);
                    }
                    ui.ctx().request_repaint();
                } else if resp.clicked() {
                    s.term.selection = None;
                }
            }
        }
        // Draw.
        let dim = self.settings.flag("termDim", true) && !focused;
        let fade = if dim { 0.7 } else { 1.0 };
        let content = s.term.renderable_content();
        let painter = ui.painter().with_clip_rect(screen);
        let offset = content.display_offset as i32;
        let colors = content.colors;
        // Each row's characters by column (a wide character's second column is a space), for links.
        let mut rows_text: Vec<Vec<char>> = vec![Vec::new(); rows];
        // Each row's cells to draw: column, text, format and whether it needs a place of its own.
        let mut cells: Vec<Vec<(usize, String, egui::TextFormat, bool)>> = vec![Vec::new(); rows];
        for cell in content.display_iter {
            let line = cell.point.line.0 + offset;
            if line < 0 || line as usize >= rows {
                continue;
            }
            let (row, col) = (line as usize, cell.point.column.0);
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let mut fg = color(cell.fg, colors, true);
            let mut bg = color(cell.bg, colors, false);
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if cell.flags.contains(Flags::DIM) {
                fg = fg.gamma_multiply(0.7);
            }
            let selected = content.selection.as_ref().is_some_and(|sel| sel.contains(cell.point));
            if selected {
                bg = SELECTION;
            }
            let wide = cell.flags.contains(Flags::WIDE_CHAR);
            let x0 = screen.left() + col as f32 * cell_w;
            let y0 = screen.top() + row as f32 * cell_h;
            if bg != BG {
                let w = if wide { cell_w * 2.0 } else { cell_w };
                painter.rect_filled(Rect::from_min_size(pos2(x0, y0), vec2(w, cell_h)), CornerRadius::ZERO, bg.gamma_multiply(fade));
            }
            let ch = if cell.c == '\0' { ' ' } else { cell.c };
            let text = rows_text.get_mut(row).expect("row");
            text.resize(text.len().max(col), ' ');
            text.push(ch);
            if wide {
                text.push(' ');
            }
            let hidden = cell.flags.contains(Flags::HIDDEN);
            let marks = cell.zerowidth().filter(|m| !m.is_empty());
            let mut shown = if hidden { ' ' } else { ch }.to_string();
            if !hidden && let Some(m) = marks {
                shown.extend(m.iter());
            }
            let mut fmt = egui::TextFormat { font_id: font.clone(), color: fg.gamma_multiply(fade), ..Default::default() };
            if cell.flags.contains(Flags::UNDERLINE) {
                fmt.underline = Stroke::new(1.0, fg.gamma_multiply(fade));
            }
            if cell.flags.contains(Flags::ITALIC) {
                fmt.italics = true;
            }
            if cell.flags.contains(Flags::STRIKEOUT) {
                fmt.strikethrough = Stroke::new(1.0, fg.gamma_multiply(fade));
            }
            cells[row].push((col, shown, fmt, wide || marks.is_some()));
        }
        // Text is drawn in runs that start at their own column, so a wide character or one with
        // combining marks (whose drawn width needn't match its columns) never shifts the rest of
        // the row away from the grid the caret and selection use.
        for (row, row_cells) in cells.into_iter().enumerate() {
            let y0 = screen.top() + row as f32 * cell_h;
            let places: Vec<(usize, bool)> = row_cells.iter().map(|c| (c.0, c.3)).collect();
            for run in cell_runs(&places) {
                if row_cells[run.clone()].iter().all(|c| c.1.trim().is_empty()) {
                    continue;
                }
                let mut job = egui::text::LayoutJob::default();
                for (_, text, fmt, _) in &row_cells[run.clone()] {
                    job.append(text, 0.0, fmt.clone());
                }
                let g = ui.fonts_mut(|f| f.layout_job(job));
                let x0 = screen.left() + row_cells[run.start].0 as f32 * cell_w;
                painter.galley(pos2(x0, y0 + (cell_h - g.size().y) / 2.0), g, FG);
            }
        }
        let rows_text: Vec<String> = rows_text.into_iter().map(|r| r.into_iter().collect()).collect();
        // Cursor.
        let cur = content.cursor;
        let cline = cur.point.line.0 + offset;
        if cur.shape != CursorShape::Hidden && cline >= 0 && (cline as usize) < rows {
            let cx = screen.left() + cur.point.column.0 as f32 * cell_w;
            let cy = screen.top() + cline as f32 * cell_h;
            let style = self.settings.string("termCursor", "bar");
            let ink = Color32::WHITE.gamma_multiply(if focused { 1.0 } else { 0.5 });
            match style.as_str() {
                "block" if focused => {
                    painter.rect_filled(Rect::from_min_size(pos2(cx, cy), vec2(cell_w, cell_h)), CornerRadius::ZERO, ink.gamma_multiply(0.7));
                }
                "block" => {
                    painter.rect_stroke(Rect::from_min_size(pos2(cx, cy), vec2(cell_w, cell_h)), CornerRadius::ZERO, Stroke::new(1.0, ink), egui::StrokeKind::Inside);
                }
                "underline" => {
                    painter.rect_filled(Rect::from_min_size(pos2(cx, cy + cell_h - 2.0), vec2(cell_w, 2.0)), CornerRadius::ZERO, ink);
                }
                _ => {
                    painter.rect_filled(Rect::from_min_size(pos2(cx, cy), vec2(2.0, cell_h)), CornerRadius::ZERO, ink);
                }
            }
        }
        if let Some(e) = &s.error {
            painter.text(screen.left_top() + vec2(4.0, 4.0), Align2::LEFT_TOP, format!("The terminal could not start: {e}"), FontId::proportional(12.0), super::widgets::REMOVED);
        } else if s.exited {
            painter.text(pos2(screen.left() + 4.0, screen.bottom() - 4.0), Align2::LEFT_BOTTOM, "The shell has exited. Open a new terminal with +.", FontId::proportional(12.0), c.fg3);
        } else if let Some(e) = &s.input_error {
            painter.text(pos2(screen.left() + 4.0, screen.bottom() - 4.0), Align2::LEFT_BOTTOM, format!("Input not sent: {e}"), FontId::proportional(12.0), super::widgets::REMOVED);
        }
        // Ctrl+click opens a link.
        if resp.clicked() && ui.input(|i| i.modifiers.command)
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let row = ((pos.y - screen.top()) / cell_h) as usize;
            let col = ((pos.x - screen.left()) / cell_w) as usize;
            if let Some(url) = rows_text.get(row).and_then(|line| link_at(line, col)) {
                later.push(Box::new(move |app| app.open_url(&url)));
            }
        }
        // Context menu: copy and paste.
        let tid = s.id;
        let mut open_find = false;
        resp.context_menu(|ui| {
            if ui.button("Copy").clicked() {
                if let Some(text) = s.term.selection_to_string() {
                    ui.ctx().copy_text(text);
                }
                ui.close();
            }
            if ui.button("Paste").clicked() {
                if tid.is_some() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                }
                ui.close();
            }
            if ui.button("Clear").clicked() {
                s.term.grid_mut().clear_history();
                ui.close();
            }
            if ui.button("Find in terminal").clicked() {
                open_find = true;
                ui.close();
            }
        });
        if open_find {
            panel.find.get_or_insert_with(Find::default).focus = true;
        }
        // The find bar, top right over the screen.
        let mut close_find = false;
        if let Some(f) = panel.find.as_mut() {
            let bar_rect = Rect::from_min_size(pos2(screen.right() - 300.0, screen.top() + 2.0), vec2(296.0, 28.0));
            ui.painter().rect(bar_rect, CornerRadius::same(4), c.panel, Stroke::new(1.0, c.line), egui::StrokeKind::Inside);
            let field = Rect::from_min_size(bar_rect.min + vec2(6.0, 4.0), vec2(if f.missing { 120.0 } else { 186.0 }, 20.0));
            ui.painter().rect_filled(field, CornerRadius::same(2), c.field);
            let edit = ui.put(
                field.shrink2(vec2(4.0, 0.0)),
                egui::TextEdit::singleline(&mut f.needle).id(ui.scope_id().with(("term_find_edit", tab))).frame(egui::Frame::NONE).font(FontId::proportional(12.0)).hint_text("Find in terminal").align(Align2::LEFT_CENTER),
            );
            if std::mem::take(&mut f.focus) {
                edit.request_focus();
            }
            let (enter, shift, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.modifiers.shift, i.key_pressed(egui::Key::Escape)));
            let mut step: Option<bool> = None;
            if edit.changed() {
                f.missing = !find_in(&mut s.term, &f.needle, false, false);
            }
            if edit.lost_focus() && enter {
                step = Some(shift);
                edit.request_focus();
            }
            if (edit.has_focus() || edit.lost_focus()) && escape {
                close_find = true;
            }
            if f.missing {
                ui.painter().text(pos2(field.right() + 8.0, bar_rect.center().y), Align2::LEFT_CENTER, "No match", FontId::proportional(12.0), c.fg3);
            }
            let button = |i: f32, icon: Icon, tip: &str| {
                let r = Rect::from_min_size(pos2(bar_rect.right() - 6.0 - (3.0 - i) * 24.0, bar_rect.top() + 3.0), Vec2::splat(22.0));
                let resp = ui.interact(r, ui.scope_id().with(("term_find_button", i as u8)), Sense::click()).tip(tip);
                if resp.hovered() {
                    ui.painter().rect_filled(r, CornerRadius::same(3), c.white(0.08));
                }
                icons::paint(ui, r, icon, 14.0, if resp.hovered() { c.white(1.0) } else { c.fg2 });
                resp.clicked()
            };
            if button(0.0, Icon::ChevronUp, "Previous match (Shift+Enter)") {
                step = Some(true);
            }
            if button(1.0, Icon::ChevronDown, "Next match (Enter)") {
                step = Some(false);
            }
            if button(2.0, Icon::X, "Close terminal search (Esc)") {
                close_find = true;
            }
            if let Some(back) = step {
                f.missing = !find_in(&mut s.term, &f.needle, back, true);
            }
        }
        if close_find {
            panel.find = None;
            s.term.selection = None;
            resp.request_focus();
        }
        if focused {
            ui.painter().rect_stroke(rect.shrink(1.0), CornerRadius::ZERO, Stroke::new(1.0, ACCENT.gamma_multiply(0.0)), egui::StrokeKind::Inside);
        }
        let _ = (Line(0), Column(0), Point::new(Line(0), Column(0)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term_with(text: &str) -> Term<Listener> {
        let listener = Listener { replies: Default::default(), title: Default::default() };
        let mut term = Term::new(Config { scrolling_history: 100, ..Config::default() }, &Size { cols: 40, rows: 5 }, listener);
        Processor::<alacritty_terminal::vte::ansi::StdSyncHandler>::new().advance(&mut term, text.as_bytes());
        term
    }

    fn shown(term: &Term<Listener>) -> Option<(i32, usize)> {
        term.selection.as_ref().and_then(|s| s.to_range(term)).map(|r| (r.start.line.0, r.start.column.0))
    }

    #[test]
    fn wide_and_combining_characters_start_runs_at_their_columns() {
        // "a", "b", a wide "漢" at 2 (3 is its spacer, not listed), "c" at 4, "e" with a mark at 5, "f".
        let cells = [(0, false), (1, false), (2, true), (4, false), (5, true), (6, false)];
        assert_eq!(cell_runs(&cells), vec![0..2, 2..3, 3..4, 4..5, 5..6]);
        assert_eq!(cell_runs(&[(0, false), (1, false), (2, false)]), vec![0..3]);
        // The grid gives every column a cell, so each run starts where its first cell is.
        let term = term_with("a漢b\r\ne\u{301}x");
        let content = term.renderable_content();
        let mut wide = None;
        let mut marked = None;
        for cell in content.display_iter {
            if cell.flags.contains(Flags::WIDE_CHAR) {
                wide = Some((cell.point.line.0, cell.point.column.0));
            }
            if cell.zerowidth().is_some() {
                marked = Some((cell.point.line.0, cell.point.column.0));
            }
        }
        assert_eq!(wide, Some((0, 1)));
        assert_eq!(marked, Some((1, 0)));
    }

    #[test]
    fn find_starts_at_the_newest_match_and_steps_both_ways() {
        // Eight lines on a five-line screen: the first three are in the scrollback.
        let mut term = term_with("alpha one\r\nbeta\r\nALPHA two\r\nc\r\nd\r\ne\r\nalpha three\r\nf");
        assert!(find_in(&mut term, "alpha", false, false));
        let newest = shown(&term).unwrap();
        assert_eq!(newest.1, 0);
        // Previous goes up to the case-insensitive "ALPHA two", then "alpha one".
        assert!(find_in(&mut term, "alpha", true, true));
        let middle = shown(&term).unwrap();
        assert!(middle.0 < newest.0);
        assert!(find_in(&mut term, "alpha", true, true));
        assert!(shown(&term).unwrap().0 < middle.0);
        // Next comes back down.
        assert!(find_in(&mut term, "alpha", false, true));
        assert_eq!(shown(&term).unwrap(), middle);
        // Typing on keeps the shown match while it still matches.
        assert!(find_in(&mut term, "alpha t", false, false));
        assert_eq!(shown(&term).unwrap(), middle);
        assert!(!find_in(&mut term, "zeta", false, false));
        assert!(term.selection.is_none());
        // Regular expression characters are plain text.
        let mut term = term_with("a.b axb");
        assert!(find_in(&mut term, "a.b", false, false));
        assert_eq!(shown(&term), Some((0, 0)));
        assert!(find_in(&mut term, "a.b", false, true));
        assert_eq!(shown(&term), Some((0, 0)));
    }

    #[test]
    fn links_are_found_under_the_pointer() {
        let line = "see https://example.invalid/a, then";
        assert_eq!(super::link_at(line, 6).as_deref(), Some("https://example.invalid/a"));
        assert!(super::link_at(line, 1).is_none());
    }
}
