//! The 30 px tabs bar: the start-screen and Launchpad buttons, the open tabs (drag to reorder,
//! drag the right edge to resize a repository tab), New Tab, the tabs list, notifications,
//! preferences and the profile menu.

use super::widgets::Tip;
use super::icons::{self, Icon};
use super::theme::{ACCENT, p};
use super::{App, TabKind};
use egui::{Color32, CornerRadius, FontId, Id, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};

const HEIGHT: f32 = 30.0;
const DEFAULT_TAB_WIDTH: f32 = 97.0;
const MIN_TAB_WIDTH: f32 = 76.0;
const MAX_TAB_WIDTH: f32 = 480.0;
const ICON_TAB: f32 = 38.0;
/// The colours of the profile marks, by the profile's icon number.
/// The colour of profile picture `i`.
pub(crate) fn profile_color(i: usize) -> Color32 {
    PROFILE_COLORS[i % PROFILE_COLORS.len()]
}

const PROFILE_COLORS: [Color32; 4] = [
    Color32::from_rgb(0x7b, 0xd9, 0x38),
    Color32::from_rgb(0x2e, 0xc4, 0xb6),
    Color32::from_rgb(0xb5, 0x7b, 0xff),
    Color32::from_rgb(0xff, 0x9f, 0x43),
];

/// A tab being dragged to a new place.
pub struct Drag {
    id: u64,
    /// Where on the tab it was picked up.
    grab: f32,
}

/// A repository tab's right edge being dragged.
#[derive(Clone, Copy)]
struct Resize {
    start_x: f32,
    start_width: f32,
}

pub fn tab_width(saved: Option<f32>) -> f32 {
    saved.filter(|w| w.is_finite()).unwrap_or(DEFAULT_TAB_WIDTH).clamp(MIN_TAB_WIDTH, MAX_TAB_WIDTH)
}

/// The thin dark line between tabs.
fn divider(ui: &Ui, x: f32, rect: Rect) {
    ui.painter().line_segment([pos2(x - 0.5, rect.top()), pos2(x - 0.5, rect.bottom())], Stroke::new(1.0, p().black(0.25)));
}

fn icon_tab(ui: &mut Ui, icon: Icon, tip: &str, active: bool) -> egui::Response {
    let c = p();
    let (rect, response) = ui.allocate_exact_size(vec2(ICON_TAB, HEIGHT), Sense::click());
    if active {
        ui.painter().rect_filled(rect, CornerRadius::ZERO, c.toolbar);
    }
    let ink = if active || response.hovered() { c.white(1.0) } else { c.white(0.8) };
    icons::paint(ui, rect, icon, 15.0, ink);
    divider(ui, rect.right(), rect);
    response.tip(tip)
}

/// The profile mark: a small pixel figure in the profile's colour.
pub(crate) fn profile_mark(ui: &Ui, rect: Rect, color: Color32) {
    let cell = rect.width() / 9.0;
    let at = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(rect.min + vec2(x * cell, y * cell), vec2(w * cell, h * cell));
    let paint = ui.painter();
    for (x, y, w, h) in [(2.0, 1.0, 5.0, 1.0), (1.0, 2.0, 7.0, 4.0), (1.0, 6.0, 2.0, 2.0), (6.0, 6.0, 2.0, 2.0)] {
        paint.rect_filled(at(x, y, w, h), CornerRadius::ZERO, color);
    }
    let eye = Color32::from_rgb(0x1c, 0x1e, 0x23);
    paint.rect_filled(at(2.0, 3.0, 1.0, 1.0), CornerRadius::ZERO, eye);
    paint.rect_filled(at(6.0, 3.0, 1.0, 1.0), CornerRadius::ZERO, eye);
}

impl App {
    pub(super) fn tabs_bar(&mut self, ui: &mut Ui) {
        let c = p();
        let shown = egui::Panel::top("tabs_bar")
            .exact_size(HEIGHT)
            .frame(egui::Frame::new().fill(c.tabs))
            .show_separator_line(false)
            .show(ui, |ui| {
                let bar = ui.max_rect();
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                ui.horizontal(|ui| {
                    if icon_tab(ui, Icon::Folder, "Repositories (start screen)", false).clicked() {
                        self.new_tab();
                    }
                    let on_launchpad = self.active_tab().is_some_and(|t| t.kind == TabKind::Launchpad);
                    if icon_tab(ui, Icon::Rocket, "Launchpad", on_launchpad).clicked() {
                        self.open_launchpad();
                    }
                    self.tab_strip(ui);
                    let new_tip = format!("New tab ({})", self.keys_of("nav:newTab").map(|k| super::keys::label(&k)).unwrap_or_default());
                    if icon_tab(ui, Icon::Plus, &new_tip, false).clicked() {
                        self.new_tab();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        self.profile_button(ui);
                        if icon_tab(ui, Icon::Settings, "Preferences", false).clicked() {
                            self.open_prefs(None);
                        }
                        let unread = self.notices.iter().filter(|n| !n.read).count() + usize::from(self.update_row());
                        let bell = icons::slot(ui, ICON_TAB, Icon::Bell, 15.0, c.white(0.8), c.white(1.0)).tip("Notifications");
                        if unread > 0 {
                            let text = if unread >= 50 { "50+".to_owned() } else { unread.to_string() };
                            let w = (text.len() as f32 * 6.0 + 6.0).max(14.0);
                            let br = egui::Rect::from_min_size(egui::pos2(bell.rect.center().x + 2.0, bell.rect.center().y - 12.0), egui::vec2(w, 13.0));
                            ui.painter().rect_filled(br, egui::CornerRadius::same(7), egui::Color32::from_rgb(0x2f, 0x81, 0xf7));
                            ui.painter().text(br.center(), egui::Align2::CENTER_CENTER, text, super::fonts::bold(9.0), egui::Color32::WHITE);
                        }
                        self.notifications(ui, &bell);
                        let list = icons::slot(ui, 33.0, Icon::ChevronDown, 16.0, c.white(0.8), c.white(1.0));
                        let list_tip = format!("Tabs list ({})", key_text(ui.ctx(), "Mod+Shift+A"));
                        let list = list.tip(list_tip);
                        egui::Popup::menu(&list).show(|ui| self.tabs_list(ui));
                    });
                });
                // The bar's lower edge.
                ui.painter().line_segment([bar.left_bottom() - vec2(0.0, 0.5), bar.right_bottom() - vec2(0.0, 0.5)], Stroke::new(1.0, c.black(0.3)));
            });
        // Preferences cover the window from here down, as the tabs stay in view.
        self.below_tabs = shown.response.rect.bottom();
    }

    /// The open tabs, scrolled sideways by the mouse wheel when they do not fit.
    fn tab_strip(&mut self, ui: &mut Ui) {
        let room = (ui.available_width() - ICON_TAB - 33.0 - 2.0 * ICON_TAB - 160.0).max(MIN_TAB_WIDTH);
        egui::ScrollArea::horizontal()
            .id_salt("tab_strip")
            .max_width(room)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                ui.horizontal(|ui| {
                    let shown: Vec<u64> = self.tabs.iter().filter(|t| t.kind != TabKind::Launchpad).map(|t| t.id).collect();
                    let mut rects = Vec::with_capacity(shown.len());
                    for (i, id) in shown.iter().enumerate() {
                        let first = i == 0;
                        let last = i + 1 == shown.len();
                        rects.push((*id, self.tab(ui, *id, first, last)));
                    }
                    self.drop_dragged_tab(ui, &rects);
                });
            });
    }

    /// One tab; returns where it was drawn.
    fn tab(&mut self, ui: &mut Ui, id: u64, first: bool, last: bool) -> Rect {
        let c = p();
        let Some(tab) = self.tabs.iter().find(|t| t.id == id).cloned() else { return Rect::NOTHING };
        let width = match tab.path() {
            Some(path) => tab_width(self.settings.tab_width(path)),
            None => DEFAULT_TAB_WIDTH,
        };
        let active = id == self.active;
        let (rect, response) = ui.allocate_exact_size(vec2(width, HEIGHT), Sense::click_and_drag());
        let dragging = self.tab_drag.as_ref().is_some_and(|d| d.id == id);
        let hovered = response.hovered() || response.contains_pointer();
        let fill = if active { c.toolbar } else if hovered { c.white(0.05) } else { Color32::TRANSPARENT };
        let fade = if dragging { 0.6 } else { 1.0 };
        ui.painter().rect_filled(rect, CornerRadius::ZERO, fill.gamma_multiply(fade));

        // Icon, title and close button.
        let ink = if active { c.white(1.0) } else { c.white(0.75) };
        let mut x = rect.left() + 12.0;
        if tab.path().is_some() {
            icons::paint(ui, Rect::from_center_size(pos2(x + 6.0, rect.center().y), Vec2::splat(12.0)), Icon::GitBranch, 12.0, ink.gamma_multiply(0.7));
            x += 18.0;
        }
        let close = Rect::from_center_size(pos2(rect.right() - 6.0 - 8.5, rect.center().y), Vec2::splat(17.0));
        let title_room = (close.left() - 4.0 - x).max(0.0);
        let font = FontId::proportional(13.0);
        let galley = ui.painter().layout_no_wrap(tab.title.clone(), font, ink.gamma_multiply(fade));
        let galley = if galley.size().x > title_room { truncated(ui, &tab.title, title_room, ink) } else { galley };
        ui.painter().galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, ink);

        let close_response = ui.interact(close, Id::NULL.with(("tab_close", id)), Sense::click());
        if active || hovered || close_response.hovered() {
            if close_response.hovered() {
                ui.painter().rect_filled(close, CornerRadius::same(4), c.white(0.15));
            }
            icons::paint(ui, close, Icon::X, 13.0, ink);
        }
        divider(ui, rect.right(), rect);
        let response = response.tip(tab.path().unwrap_or(&tab.title).to_owned());

        if close_response.clicked() || response.middle_clicked() {
            self.close_tab(id);
            return rect;
        }
        if response.clicked() {
            self.active = id;
            self.remember_tabs();
        }
        if response.drag_started() && !self.resize_hovered(ui, rect) {
            let grab = ui.ctx().pointer_interact_pos().map_or(0.0, |p| p.x - rect.left());
            self.tab_drag = Some(Drag { id, grab });
            self.active = id;
        }
        if dragging {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        if let Some(path) = tab.path() {
            self.resize_handle(ui, rect, path.to_owned(), &tab.title);
        }
        response.context_menu(|ui| {
            if ui.button("Close tab").clicked() {
                self.close_tab(id);
            }
            if ui.button("Close other tabs").clicked() {
                let others: Vec<u64> = self.tabs.iter().filter(|t| t.id != id).map(|t| t.id).collect();
                others.into_iter().for_each(|o| self.close_tab(o));
            }
            if ui.button("Close tabs to the right").clicked() {
                let at = self.tabs.iter().position(|t| t.id == id).unwrap_or(0);
                let right: Vec<u64> = self.tabs[at + 1..].iter().map(|t| t.id).collect();
                right.into_iter().for_each(|o| self.close_tab(o));
            }
            ui.separator();
            ui.add_enabled(false, egui::Button::new("Rename tab"));
            if ui.add_enabled(!self.closed.is_empty(), egui::Button::new("Reopen Closed Tab")).clicked() {
                self.reopen_tab();
            }
            ui.separator();
            if ui.add_enabled(!first, egui::Button::new("Move Tab Left").shortcut_text("Ctrl+Shift+PgUp")).clicked() {
                self.shift_tab(id, -1);
            }
            if ui.add_enabled(!last, egui::Button::new("Move Tab Right").shortcut_text("Ctrl+Shift+PgDn")).clicked() {
                self.shift_tab(id, 1);
            }
        });
        rect
    }

    fn resize_hovered(&self, ui: &Ui, rect: Rect) -> bool {
        ui.ctx().pointer_interact_pos().is_some_and(|p| p.x >= rect.right() - 5.0)
    }

    /// The 5 px handle on a repository tab's right edge. Escape cancels, a double click resets.
    fn resize_handle(&mut self, ui: &mut Ui, rect: Rect, path: String, title: &str) {
        let handle = Rect::from_min_max(pos2(rect.right() - 5.0, rect.top()), rect.right_bottom());
        let id = Id::NULL.with(("tab_resize", &path));
        let response = ui.interact(handle, id, Sense::click_and_drag()).tip(format!("Drag to resize the {title} tab. Escape cancels. Double-click to reset."));
        let memory: Option<Resize> = ui.data(|d| d.get_temp(id));
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
        }
        if response.drag_started() {
            let start_x = ui.ctx().pointer_interact_pos().map_or(rect.right(), |p| p.x);
            ui.data_mut(|d| d.insert_temp(id, Resize { start_x, start_width: rect.width() }));
        }
        if let Some(start) = memory {
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.settings.set_tab_width(&path, start.start_width);
                ui.data_mut(|d| d.remove::<Resize>(id));
            } else if response.dragged() {
                let x = ui.ctx().pointer_interact_pos().map_or(start.start_x, |p| p.x);
                self.settings.set_tab_width(&path, tab_width(Some(start.start_width + x - start.start_x)));
            } else if response.drag_stopped() {
                ui.data_mut(|d| d.remove::<Resize>(id));
            }
        }
        if response.double_clicked() {
            self.settings.set_tab_width(&path, DEFAULT_TAB_WIDTH);
        }
        if response.hovered() || memory.is_some() {
            ui.painter().line_segment([pos2(rect.right() - 1.0, rect.top()), pos2(rect.right() - 1.0, rect.bottom())], Stroke::new(2.0, ACCENT));
        }
    }

    /// While a tab is dragged, move it past each neighbour the pointer crosses the middle of.
    fn drop_dragged_tab(&mut self, ui: &Ui, rects: &[(u64, Rect)]) {
        let Some(drag) = &self.tab_drag else { return };
        let dragged = drag.id;
        if !ui.input(|i| i.pointer.primary_down()) {
            self.tab_drag = None;
            return self.remember_tabs();
        }
        let Some(pointer) = ui.ctx().pointer_interact_pos() else { return };
        let left = pointer.x - drag.grab;
        let Some(&(_, own)) = rects.iter().find(|(id, _)| *id == dragged) else { return };
        let right = left + own.width();
        for (id, rect) in rects {
            if *id == dragged {
                continue;
            }
            let passed_right = rect.left() > own.left() && right > rect.center().x;
            let passed_left = rect.left() < own.left() && left < rect.center().x;
            if passed_right || passed_left {
                let a = self.tabs.iter().position(|t| t.id == dragged);
                let b = self.tabs.iter().position(|t| t.id == *id);
                if let (Some(a), Some(b)) = (a, b) {
                    let tab = self.tabs.remove(a);
                    self.tabs.insert(b, tab);
                }
                break;
            }
        }
    }

    fn tabs_list(&mut self, ui: &mut Ui) {
        let shown: Vec<(u64, String)> = self.tabs.iter().filter(|t| t.kind != TabKind::Launchpad).map(|t| (t.id, t.title.clone())).collect();
        for (id, title) in shown {
            let mark = if id == self.active { "• " } else { "   " };
            if ui.button(format!("{mark}{title}")).clicked() {
                self.active = id;
                self.remember_tabs();
            }
        }
    }

    fn profile_button(&mut self, ui: &mut Ui) {
        let c = p();
        let (name, icon) = self.settings.active_profile();
        let font = FontId::proportional(13.0);
        let galley = ui.painter().layout_no_wrap(name, font, c.white(0.9));
        let width = 8.0 + 18.0 + 6.0 + galley.size().x + 6.0 + 13.0 + 8.0;
        let (rect, response) = ui.allocate_exact_size(vec2(width, HEIGHT), Sense::click());
        if response.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::ZERO, c.white(0.05));
        }
        let mut x = rect.left() + 8.0;
        profile_mark(ui, Rect::from_min_size(pos2(x, rect.center().y - 9.0), Vec2::splat(18.0)), PROFILE_COLORS[icon % PROFILE_COLORS.len()]);
        x += 24.0;
        let text_y = rect.center().y - galley.size().y / 2.0;
        let text_width = galley.size().x;
        ui.painter().galley(pos2(x, text_y), galley, c.white(0.9));
        x += text_width + 6.0;
        icons::paint(ui, Rect::from_min_size(pos2(x, rect.center().y - 6.5), Vec2::splat(13.0)), Icon::ChevronDown, 13.0, c.white(0.9));
        egui::Popup::menu(&response).align(egui::RectAlign::BOTTOM_END).show(|ui| {
            ui.label(egui::RichText::new("PROFILES").size(11.0).color(c.fg2));
            let active = self.settings.active_profile_index();
            for (i, profile) in self.settings.profile_names().into_iter().enumerate() {
                let mark = if i == active { "✓ " } else { "   " };
                if ui.button(format!("{mark}{profile}")).clicked() && i != active {
                    self.switch_profile(i);
                }
            }
            ui.separator();
            if ui.button("Manage profiles...").clicked() {
                self.open_prefs(Some("profiles"));
            }
            if ui.button("Accounts...").clicked() {
                self.open_prefs(Some("integrations"));
            }
            if ui.button("Preferences...").clicked() {
                self.open_prefs(None);
            }
        });
    }
}

/// `text` cut to fit `width`, ending in an ellipsis.
fn truncated(ui: &Ui, text: &str, width: f32, color: Color32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), FontId::proportional(13.0), color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width);
    ui.fonts_mut(|f| f.layout_job(job))
}

/// How a command's key reads on this system (`Ctrl+T`, `⌘T`).
/// How a key written in craft-keymap's notation reads on this system.
fn key_text(ctx: &egui::Context, chord: &str) -> String {
    craft_menu::Chord::parse(chord).map(|c| c.display(craft_keymap::egui::platform(ctx))).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_widths_stay_within_their_limits() {
        assert_eq!(tab_width(None), DEFAULT_TAB_WIDTH);
        assert_eq!(tab_width(Some(10.0)), MIN_TAB_WIDTH);
        assert_eq!(tab_width(Some(9000.0)), MAX_TAB_WIDTH);
        assert_eq!(tab_width(Some(f32::NAN)), DEFAULT_TAB_WIDTH);
    }
}
