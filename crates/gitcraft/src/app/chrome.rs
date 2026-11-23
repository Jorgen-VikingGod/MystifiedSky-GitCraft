//! The status bar, the screen of the tab in front (start screen, repository, Launchpad) and
//! the toasts.

use super::widgets::Tip;
use super::icons::{self, Icon};
use super::menu::RELEASES_URL;
use super::theme::{BAD, GOOD, p};
use super::{App, Msg, TabKind, VERSION, folder_name};
use egui::{CornerRadius, FontId, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

impl App {
    /// The 26 px bar along the bottom of the window.
    pub(super) fn status_bar(&mut self, ui: &mut Ui) {
        let c = p();
        let corners = super::frame::bottom_corners(ui.ctx());
        egui::Panel::bottom("status_bar")
            .exact_size(26.0)
            .frame(egui::Frame::new().fill(c.panel).corner_radius(corners))
            .show_separator_line(false)
            .show(ui, |ui| {
                let bar = ui.max_rect();
                ui.painter().line_segment([bar.left_top() + vec2(0.0, 0.5), bar.right_top() + vec2(0.0, 0.5)], Stroke::new(1.0, c.black(0.3)));
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                ui.horizontal_centered(|ui| {
                    ui.add_space(22.0);
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                    let working = self.busy.is_some() || self.job.is_some() || self.ai_label.is_some();
                    if working {
                        let t = ui.input(|i| i.time) as f32;
                        ui.painter().add(egui::Shape::line((0..=20).map(|k| {
                            let a = t * 6.0 + k as f32 / 20.0 * std::f32::consts::TAU * 0.75;
                            pos2(rect.center().x + 6.0 * a.cos(), rect.center().y + 6.0 * a.sin())
                        }).collect(), Stroke::new(1.5, c.link)));
                        ui.ctx().request_repaint();
                    } else {
                        icons::paint(ui, rect, Icon::Rocket, 14.0, c.white(0.8));
                    }
                    ui.add_space(8.0);
                    let hint = if working { None } else { self.integration_hint() };
                    if let Some((hint, launchpad)) = &hint {
                        let launchpad = *launchpad;
                        let resp = ui.add(egui::Label::new(RichText::new(hint.as_str()).size(13.0).color(c.white(0.9))).truncate().sense(Sense::click())).tip(if launchpad { "Open the Launchpad" } else { "Connect GitHub, GitLab, Bitbucket and more" });
                        if resp.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if resp.clicked() {
                            if launchpad {
                                self.open_launchpad();
                            } else {
                                self.open_prefs(Some("integrations"));
                            }
                        }
                    }
                    let text = self.job.as_ref().map(|j| j.text.clone()).filter(|t| !t.is_empty()).or_else(|| self.busy.clone()).or_else(|| (!self.ai_reads.is_empty()).then(|| "Waiting for AI…".to_owned())).or_else(|| self.ai_label.clone()).unwrap_or_else(|| "Ready".into());
                    if hint.is_none() {
                        ui.add(egui::Label::new(RichText::new(text).size(13.0).color(c.white(0.9))).truncate());
                    }
                    if let Some(pct) = self.job.as_ref().and_then(|j| j.percent) {
                        ui.add_space(8.0);
                        let (bar, _) = ui.allocate_exact_size(vec2(90.0, 5.0), Sense::hover());
                        ui.painter().rect_filled(bar, CornerRadius::same(2), c.white(0.1));
                        ui.painter().rect_filled(Rect::from_min_size(bar.min, vec2(90.0 * pct.min(100) as f32 / 100.0, 5.0)), CornerRadius::same(2), c.link);
                    }
                    if !self.ai_reads.is_empty() {
                        ui.add_space(8.0);
                        if ui.add(egui::Label::new(RichText::new("Stop AI").size(12.0).color(super::widgets::REMOVED)).sense(Sense::click())).clicked() {
                            self.stop_ai();
                        }
                    }
                    if let Some(job) = self.job.clone() {
                        ui.add_space(8.0);
                        let resp = ui.add(egui::Label::new(RichText::new("Cancel").size(12.0).color(super::widgets::REMOVED)).sense(Sense::click())).tip("Stop this job");
                        if resp.clicked() {
                            self.engine.jobs.cancel(&job.id);
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Version badge, kept clear of the window's resize edge.
                        ui.add_space(18.0);
                        let font = FontId::monospace(12.0);
                        let galley = ui.painter().layout_no_wrap(VERSION.to_owned(), font, c.white(0.9));
                        let (rect, _) = ui.allocate_exact_size(vec2(galley.size().x + 12.0, 20.0), Sense::hover());
                        ui.painter().rect_stroke(rect, CornerRadius::same(3), Stroke::new(1.0, c.white(0.25)), StrokeKind::Inside);
                        ui.painter().galley(rect.center() - galley.size() / 2.0, galley, c.white(0.9));
                        ui.add_space(8.0);
                        let support = ui.add(egui::Label::new(RichText::new("Support").color(c.link)).sense(Sense::click())).tip("Report a problem or ask for help");
                        if support.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if support.clicked() {
                            self.open_url(&RELEASES_URL.replace("/releases", "/issues"));
                        }
                        ui.add_space(12.0);
                        let zoom = format!("{}%", (self.settings.zoom() * 100.0).round());
                        if status_button(ui, Icon::ZoomIn, Some(&zoom), "Reset zoom").clicked() {
                            self.set_zoom(ui.ctx(), 1.0);
                        }
                        if status_button(ui, Icon::Gift, None, "What's new (release notes)").clicked() {
                            self.open_url(RELEASES_URL);
                        }
                        if status_button(ui, Icon::Map, None, "Feature plan").clicked() {
                            self.plan_dialog();
                        }
                        if status_button(ui, Icon::Keyboard, None, "Keyboard shortcuts").clicked() {
                            self.shortcuts_dialog();
                        }
                        if status_button(ui, Icon::List, None, "Activity log").clicked() {
                            self.log_dialog();
                        }
                        match self.update.phase {
                            super::updater::Phase::Ready => {
                                let version = self.update.update.as_ref().map(|u| u.version.clone()).unwrap_or_default();
                                let resp = ui.add(egui::Label::new(RichText::new("Update Ready (Restart GitCraft)").font(super::fonts::semibold(12.5)).color(super::theme::GOOD)).sense(Sense::click())).tip(format!("Install GitCraft {version} and restart"));
                                if resp.hovered() {
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                }
                                if resp.clicked() {
                                    let ctx = ui.ctx().clone();
                                    self.install_update(&ctx);
                                }
                                ui.add_space(12.0);
                            }
                            super::updater::Phase::Downloading => {
                                let text = self.update.percent().map_or("Downloading update…".to_owned(), |p| format!("Downloading update {p}%"));
                                ui.label(RichText::new(text).size(12.0).color(c.fg2));
                                ui.add_space(12.0);
                                ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
                            }
                            _ => {}
                        }
                    });
                });
            });
    }

    /// The screen of the tab in front.
    pub(super) fn screen(&mut self, ui: &mut Ui) {
        let c = p();
        let kind = self.active_tab().map(|t| t.kind.clone());
        let id = self.active;
        egui::CentralPanel::default().frame(egui::Frame::new().fill(c.app)).show(ui, |ui| match kind {
            Some(TabKind::Repo(path)) => self.repo_screen(ui, id, &path),
            Some(TabKind::Launchpad) => self.launchpad(ui),
            _ => self.start_screen(ui),
        });
    }

    fn start_screen(&mut self, ui: &mut Ui) {
        let c = p();
        egui::ScrollArea::vertical().id_salt("start").show(ui, |ui| {
            let full = ui.available_width();
            let left = (full * 0.15).round();
            let right = (full * 0.08).round();
            ui.add_space(30.0);
            ui.horizontal_top(|ui| {
                ui.add_space(left);
                let room = (full - left - right).max(200.0);
                let repos_width = ((room - 25.0) * 2.6 / 3.75).max(200.0);
                ui.vertical(|ui| {
                    ui.set_width(repos_width - 24.0);
                    self.repositories(ui);
                });
                ui.add_space(24.0 + 12.0);
                let (line, _) = ui.allocate_exact_size(vec2(1.0, 360.0), Sense::hover());
                ui.painter().rect_filled(line, CornerRadius::ZERO, c.white(0.08));
                ui.add_space(12.0 + 12.0);
                ui.vertical(|ui| {
                    ui.set_width((room - repos_width - 25.0).max(200.0));
                    self.integrations(ui);
                });
            });
        });
    }

    fn repositories(&mut self, ui: &mut Ui) {
        let c = p();
        ui.label(RichText::new("Repositories").size(15.0).color(c.white(0.9)));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if big_button(ui, Icon::FolderOpen, "Open", true).clicked() {
                self.open_repo_dialog();
            }
            if big_button(ui, Icon::CloudDownload, "Clone", true).clicked() {
                self.clone_dialog("");
            }
            if big_button(ui, Icon::SquarePlus, "Create", true).clicked() {
                self.init_dialog();
            }
        });
        ui.add_space(8.0);
        let filter_id = ui.scope_id().with("start_filter");
        let mut filter: String = ui.data(|d| d.get_temp(filter_id)).unwrap_or_default();
        // Ctrl+Alt+F (or its binding) goes to this field while the Start screen is in front.
        let focus = self.focus_left_filter.replace(false);
        let hint = match self.keys_of("ui:filterLeft") {
            Some(keys) => format!("Search repositories ({})", super::keys::label(&keys)),
            None => "Search repositories".to_owned(),
        };
        search_field(ui, &mut filter, &hint, focus);
        ui.data_mut(|d| d.insert_temp(filter_id, filter.clone()));
        ui.add_space(10.0);
        let needle = filter.to_lowercase();
        let favorites: Vec<String> = self.settings.get::<Vec<String>>("favorites").unwrap_or_default().into_iter().filter(|r| r.to_lowercase().contains(&needle)).collect();
        if !favorites.is_empty() {
            ui.label(RichText::new("Favorites").size(12.0).color(c.fg));
            for path in favorites {
                self.repo_row(ui, &path);
            }
            ui.add_space(12.0);
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("Recent").size(12.0).color(c.fg));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled(false, egui::Label::new(RichText::new("Repository Management").size(12.0).color(c.link)));
            });
        });
        let recents: Vec<String> = self.settings.recents().into_iter().filter(|r| r.to_lowercase().contains(&needle)).collect();
        if recents.is_empty() {
            ui.horizontal_wrapped(|ui| {
                let text = if filter.trim().is_empty() { "No recent repositories yet.".to_owned() } else { format!("No repositories match “{filter}”.") };
                ui.label(RichText::new(text).size(12.5).color(c.fg3));
                if link(ui, "Try the sample project", 12.5).clicked() {
                    self.open_sample();
                }
            });
        }
        for path in recents {
            self.repo_row(ui, &path);
        }
    }

    /// A repository's name, which opens it, and its folder.
    fn repo_row(&mut self, ui: &mut Ui, path: &str) {
        let c = p();
        ui.horizontal(|ui| {
            ui.set_height(17.0);
            ui.spacing_mut().item_spacing.x = 10.0;
            if link(ui, &folder_name(path), 12.0).clicked() {
                self.open_repo(path);
            }
            ui.add(egui::Label::new(RichText::new(path).size(12.0).color(c.fg2)).truncate());
        });
    }

    fn integrations(&mut self, ui: &mut Ui) {
        let c = p();
        ui.label(RichText::new("Connect More Integrations").size(15.0).color(c.white(0.9)));
        ui.add_space(8.0);
        ui.label(RichText::new("Connect a hosting or issue tracking account to see its pull requests and issues in the left panel and Launchpad.").size(12.0).color(c.fg));
        ui.add_space(10.0);
        if ui.add(egui::Button::new(RichText::new("Connect Integrations").size(13.0)).min_size(vec2(0.0, 34.0))).clicked() {
            self.open_prefs(Some("integrations"));
        }
        ui.add_space(20.0);
        ui.label(RichText::new("Workspaces").size(12.0).color(c.fg));
        let ws: Vec<serde_json::Value> = self.settings.get("workspaces").unwrap_or_default();
        for (i, w) in ws.iter().enumerate() {
            let repos = w["repos"].as_array().map(|a| a.iter().filter_map(|r| r.as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default();
            if link(ui, &format!("{} ({})", w["name"].as_str().unwrap_or("Workspace"), w["repos"].as_array().map_or(0, Vec::len)), 12.0).tip(repos).clicked() {
                self.open_workspace(i);
            }
        }
        if link(ui, if ws.is_empty() { "Create a workspace" } else { "Manage workspaces" }, 12.0).clicked() {
            self.workspaces_dialog();
        }
        ui.add_space(20.0);
        ui.label(RichText::new("Resources").size(12.0).color(c.fg));
        if link(ui, "Try the sample project", 12.0).clicked() {
            self.open_sample();
        }
        if link(ui, "Intro Tutorials", 12.0).clicked() {
            self.open_url("https://git-scm.com/book/en/v2/Getting-Started-What-is-Git%3F");
        }
        if link(ui, "Release Notes", 12.0).clicked() {
            self.open_url(RELEASES_URL);
        }
    }

    /// Make the sample repository away from the window (it runs Git), then open it.
    fn open_sample(&mut self) {
        self.spawn(|| match gitcraft_core::sample::ensure_sample() {
            Ok(path) => Msg::OpenRepo(path.to_string_lossy().into_owned()),
            Err(e) => Msg::Toast(false, e),
        });
    }

    pub(super) fn draw_toasts(&mut self, ctx: &egui::Context) {
        let now = std::time::Instant::now();
        self.toasts.retain(|t| t.until > now);
        if let Some(next) = self.toasts.iter().map(|t| t.until).min() {
            ctx.request_repaint_after(next - now);
        }
        let c = p();
        let screen = ctx.content_rect();
        let mut y = screen.bottom() - 26.0 - 12.0;
        for (i, toast) in self.toasts.iter().enumerate().rev() {
            let galley = ctx.fonts_mut(|f| f.layout(toast.text.clone(), FontId::proportional(13.0), c.fg, 360.0));
            let size = galley.size() + vec2(36.0, 16.0);
            let rect = Rect::from_min_size(pos2(screen.right() - 16.0 - size.x, y - size.y), size);
            y = rect.top() - 8.0;
            egui::Area::new(egui::Id::NULL.with(("toast", i))).order(egui::Order::Foreground).fixed_pos(rect.min).show(ctx, |ui| {
                let painter = ui.painter();
                painter.rect(rect, CornerRadius::same(6), c.panel2, Stroke::new(1.0, c.line2), StrokeKind::Inside);
                painter.circle_filled(pos2(rect.left() + 14.0, rect.center().y), 4.0, if toast.ok { GOOD } else { BAD });
                painter.galley(pos2(rect.left() + 26.0, rect.top() + 8.0), galley, c.fg);
            });
        }
    }
}

/// A status-bar button: an icon, maybe a label, the full height of the bar.
fn status_button(ui: &mut Ui, icon: Icon, label: Option<&str>, tip: &str) -> egui::Response {
    let c = p();
    let galley = label.map(|l| ui.painter().layout_no_wrap(l.to_owned(), FontId::proportional(13.0), c.white(0.85)));
    let width = 9.0 + 15.0 + galley.as_ref().map_or(0.0, |g| 6.0 + g.size().x) + 9.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, ui.available_height()), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::ZERO, c.white(0.05));
    }
    let ink = if response.hovered() { c.white(1.0) } else { c.white(0.85) };
    icons::paint(ui, Rect::from_min_size(pos2(rect.left() + 9.0, rect.center().y - 7.5), Vec2::splat(15.0)), icon, 15.0, ink);
    if let Some(galley) = galley {
        ui.painter().galley(pos2(rect.left() + 30.0, rect.center().y - galley.size().y / 2.0), galley, ink);
    }
    response.tip(tip)
}

/// The start screen's Open, Clone and Create buttons.
fn big_button(ui: &mut Ui, icon: Icon, label: &str, enabled: bool) -> egui::Response {
    let c = p();
    let galley = ui.painter().layout_no_wrap(label.to_owned(), FontId::proportional(13.0), c.white(0.9));
    let size = vec2(10.0 + 14.0 + 8.0 + galley.size().x + 12.0, 30.0);
    let (rect, response) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let fill = if enabled && response.hovered() { c.white(0.1) } else { c.white(0.06) };
    ui.painter().rect(rect, CornerRadius::same(3), fill, Stroke::new(1.0, c.white(0.1)), StrokeKind::Inside);
    let fade = if enabled { 1.0 } else { 0.5 };
    icons::paint(ui, Rect::from_min_size(pos2(rect.left() + 10.0, rect.center().y - 7.0), Vec2::splat(14.0)), icon, 14.0, c.white(0.9).gamma_multiply(fade));
    ui.painter().galley(pos2(rect.left() + 32.0, rect.center().y - galley.size().y / 2.0), galley, c.white(0.9).gamma_multiply(fade));
    response
}

/// The 24 px search field over the repositories list.
fn search_field(ui: &mut Ui, text: &mut String, hint: &str, focus: bool) {
    let c = p();
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 24.0), Sense::hover());
    let edit_rect = Rect::from_min_max(pos2(rect.left() + 22.0, rect.top()), rect.right_bottom());
    let id = ui.scope_id().with("start_search");
    let focused = ui.memory(|m| m.has_focus(id));
    ui.painter().rect(rect, CornerRadius::same(2), c.field, Stroke::new(1.0, if focused { super::theme::ACCENT } else { c.white(0.1) }), StrokeKind::Inside);
    icons::paint(ui, Rect::from_min_size(pos2(rect.left() + 6.0, rect.center().y - 6.0), Vec2::splat(12.0)), Icon::Search, 12.0, c.fg3);
    let edit = egui::TextEdit::singleline(text)
        .id(id)
        .frame(egui::Frame::NONE)
        .hint_text(RichText::new(hint).color(c.fg3))
        .font(FontId::proportional(11.5))
        .align(egui::Align2::LEFT_CENTER)
        .desired_width(edit_rect.width());
    let response = ui.put(edit_rect, edit);
    if focus {
        response.request_focus();
    }
}

/// Link-coloured text that opens something; underlined under the pointer.
fn link(ui: &mut Ui, text: &str, size: f32) -> egui::Response {
    let c = p();
    let response = ui.add(egui::Label::new(RichText::new(text).size(size).strong().color(c.link)).sense(Sense::click()));
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        let r = response.rect;
        ui.painter().line_segment([r.left_bottom(), r.right_bottom()], Stroke::new(1.0, c.link));
    }
    response
}
