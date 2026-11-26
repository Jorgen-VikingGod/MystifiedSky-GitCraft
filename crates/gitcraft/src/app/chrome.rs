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
                        // The zoom level opens a list of presets.
                        let zr = status_button(ui, Icon::ZoomIn, Some(&zoom), "Zoom");
                        let now = self.settings.zoom();
                        let mut pick = None;
                        egui::Popup::menu(&zr).show(|ui| {
                            let mut factors = crate::settings::ZOOM_FACTORS;
                            factors.sort_by(f32::total_cmp);
                            for z in factors {
                                let label = format!("{}{}%", if (z - now).abs() < 0.001 { "✓ " } else { "   " }, (z * 100.0).round());
                                if ui.button(label).clicked() {
                                    pick = Some(z);
                                }
                            }
                            ui.separator();
                            if ui.button("Reset zoom").clicked() {
                                pick = Some(1.0);
                            }
                        });
                        if let Some(z) = pick {
                            self.set_zoom(ui.ctx(), z);
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
        let full = ui.available_width();
        let height = ui.available_height();
        egui::ScrollArea::vertical().id_salt("start").show(ui, |ui| {
            if full <= 760.0 {
                egui::Frame::new().inner_margin(egui::Margin::symmetric(20, 24)).show(ui, |ui| {
                    ui.set_width(ui.available_width().max(0.0));
                    self.repositories(ui);
                    ui.add_space(24.0);
                    self.integrations(ui);
                });
                return;
            }
            let left = (full * 0.15).round();
            let right = (full * 0.08).round();
            ui.add_space(30.0);
            let (repo_width, integration_width) = start_columns(ui.available_width() - left - right);
            let row = ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.add_space(left);
                ui.vertical(|ui| {
                    ui.set_width(repo_width);
                    self.repositories(ui);
                });
                let divider_x = ui.cursor().left() + 36.5;
                ui.add_space(61.0);
                ui.vertical(|ui| {
                    ui.set_width(integration_width);
                    self.integrations(ui);
                });
                divider_x
            });
            let top = row.response.rect.top();
            let bottom = row.response.rect.bottom().max(top + height - 30.0);
            ui.painter().line_segment([pos2(row.inner, top), pos2(row.inner, bottom)], Stroke::new(1.0, p().white(0.08)));
        });
    }

    fn repositories(&mut self, ui: &mut Ui) {
        let c = p();
        let locale = self.locale();
        ui.label(RichText::new(locale.text("Repositories")).size(15.0).color(c.white(0.9)));
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if big_button(ui, Icon::FolderOpen, locale.text("Open")).clicked() {
                self.open_repo_dialog();
            }
            if big_button(ui, Icon::CloudDownload, locale.text("Clone")).clicked() {
                self.clone_dialog("");
            }
            if big_button(ui, Icon::SquarePlus, locale.text("Create")).clicked() {
                self.init_dialog();
            }
        });
        ui.add_space(8.0);
        let filter_id = egui::Id::unique("start_filter");
        let mut filter: String = ui.data(|d| d.get_temp(filter_id)).unwrap_or_default();
        // Ctrl+Alt+F (or its binding) goes to this field while the Start screen is in front.
        let focus = self.focus_left_filter.replace(false);
        let search = locale.text("Search repositories");
        let hint = match self.keys_of("ui:filterLeft") {
            Some(keys) => format!("{search} ({})", super::keys::label(&keys)),
            None => search.to_owned(),
        };
        search_field(ui, &mut filter, &hint, focus);
        ui.data_mut(|d| d.insert_temp(filter_id, filter.clone()));
        ui.add_space(10.0);
        let needle = filter.to_lowercase();
        let favorites: Vec<String> = self.settings.get::<Vec<String>>("favorites").unwrap_or_default().into_iter().filter(|r| r.to_lowercase().contains(&needle)).collect();
        if !favorites.is_empty() {
            ui.label(RichText::new(locale.text("Favorites")).size(12.0).color(c.fg));
            for path in favorites {
                self.repo_row(ui, &path);
            }
            ui.add_space(12.0);
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(locale.text("Recent")).size(12.0).color(c.fg));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if link(ui, locale.text("Repository Management"), 12.0).clicked() {
                    self.repo_management_dialog();
                }
            });
        });
        let recents: Vec<String> = self.settings.recents().into_iter().filter(|r| r.to_lowercase().contains(&needle)).collect();
        if recents.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let text = if filter.trim().is_empty() { locale.text("No recent repositories yet.").to_owned() } else { format!("No repositories match “{filter}”.") };
                ui.label(RichText::new(text).size(12.5).color(c.fg3));
                let label = if self.sample_loading { "Preparing the sample project…" } else { "Try the sample project" };
                if ui.add_enabled_ui(!self.sample_loading, |ui| link(ui, locale.text(label), 12.5)).inner.clicked() {
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
        let locale = self.locale();
        ui.label(RichText::new(locale.text("Connect More Integrations")).size(15.0).color(c.white(0.9)));
        ui.add_space(8.0);
        ui.label(RichText::new(locale.text("Connect a hosting or issue tracking account to see its pull requests and issues in the left panel and Launchpad.")).size(12.0).color(c.fg));
        ui.add_space(6.0);
        if big_button(ui, Icon::ExternalLink, locale.text("Connect Integrations")).clicked() {
            self.open_prefs(Some("integrations"));
        }
        ui.add_space(20.0);
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.label(RichText::new(locale.text("Workspaces")).size(12.0).color(c.fg));
        ui.add_space(4.0);
        let ws: Vec<serde_json::Value> = self.settings.get("workspaces").unwrap_or_default();
        for (i, w) in ws.iter().enumerate() {
            let repos = w["repos"].as_array().map(|a| a.iter().filter_map(|r| r.as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default();
            if link(ui, &format!("{} ({})", w["name"].as_str().unwrap_or("Workspace"), w["repos"].as_array().map_or(0, Vec::len)), 12.0).tip(repos).clicked() {
                self.open_workspace(i);
            }
        }
        if link(ui, locale.text(if ws.is_empty() { "Create a workspace" } else { "Manage workspaces" }), 12.0).clicked() {
            self.workspaces_dialog();
        }
        ui.add_space(20.0);
        ui.label(RichText::new(locale.text("Resources")).size(12.0).color(c.fg));
        ui.add_space(4.0);
        let sample = if self.sample_loading { "Preparing…" } else { "Try the sample project" };
        if ui.add_enabled_ui(!self.sample_loading, |ui| link(ui, locale.text(sample), 12.0)).inner.clicked() {
            self.open_sample();
        }
        if link(ui, locale.text("Intro Tutorials"), 12.0).clicked() {
            self.open_url("https://git-scm.com/book/en/v2/Getting-Started-What-is-Git%3F");
        }
        if link(ui, locale.text("Release Notes"), 12.0).clicked() {
            self.open_url(RELEASES_URL);
        }
        if link(ui, &format!("{} ↗", locale.text("Documentation")), 12.0).clicked() {
            self.open_url("https://git-scm.com/docs");
        }
        if link(ui, locale.text("Keyboard Shortcuts"), 12.0).clicked() {
            self.shortcuts_dialog();
        }
        if link(ui, locale.text("Feature Plan"), 12.0).clicked() {
            self.plan_dialog();
        }
        if link(ui, locale.text("About GitCraft"), 12.0).clicked() {
            self.about_dialog();
        }
    }

    /// Make the sample repository away from the window (it runs Git), then open it.
    fn open_sample(&mut self) {
        if self.sample_loading {
            return;
        }
        self.sample_loading = true;
        self.spawn(|| {
            let result = gitcraft_core::sample::ensure_sample();
            Msg::Apply(Box::new(move |app| {
                app.sample_loading = false;
                match result {
                    Ok(path) => app.open_repo(&path.to_string_lossy()),
                    Err(e) => app.toast(false, e),
                }
            }))
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
        let location = self.settings.string("notificationLocation", "bottom-left");
        let top = matches!(location.as_str(), "top-left" | "top-right");
        let right = matches!(location.as_str(), "top-right" | "bottom-right");
        let width = (screen.width() - 24.0).clamp(1.0, 300.0);
        let x = if right { screen.right() - 12.0 - width } else { screen.left() + 12.0 };
        let mut y = if top { self.below_tabs + 14.0 } else { screen.bottom() - 36.0 };
        let mut dismissed = None;
        for (i, toast) in self.toasts.iter().enumerate().rev() {
            let status = if toast.ok { GOOD } else { BAD };
            let anchor = if top { egui::Align2::LEFT_TOP } else { egui::Align2::LEFT_BOTTOM };
            let area = egui::Area::new(egui::Id::NULL.with(("toast", toast.until)))
                .order(egui::Order::Foreground)
                .pivot(anchor)
                .fixed_pos(pos2(x, y))
                .show(ctx, |ui| {
                    let frame = egui::Frame::new().fill(c.panel2).stroke(Stroke::new(1.0, status.gamma_multiply(0.7)))
                        .corner_radius(CornerRadius::same(6)).inner_margin(egui::Margin::symmetric(11, 7));
                    frame.show(ui, |ui| {
                        ui.set_width(width - frame.total_margin().sum().x);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                            let (rect, button) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::click());
                            button.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Dismiss notification"));
                            icons::paint(ui, rect, Icon::X, 14.0, if button.hovered() { c.fg } else { c.fg2 });
                            if button.tip("Dismiss notification").clicked() {
                                dismissed = Some(i);
                            }
                            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                                    let sign = if toast.ok { "✓" } else { "×" };
                                    let columns = format!("{sign} {}", toast.text);
                                    ui.add(egui::Label::new(RichText::new(columns).size(12.5).color(c.fg)).wrap());
                                });
                                if !toast.ok && link(ui, "Copy error message", 11.5).clicked() {
                                    ctx.copy_text(toast.text.clone());
                                }
                            });
                        });
                    });
            });
            y = if top { area.response.rect.bottom() + 8.0 } else { area.response.rect.top() - 8.0 };
        }
        if let Some(index) = dismissed {
            self.toasts.remove(index);
        }
    }
}

fn start_columns(room: f32) -> (f32, f32) {
    let content = (room - 61.0).max(0.0);
    let integrations = (content * 1.15 / 3.75).max(188.0).min(content);
    (content - integrations, integrations)
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
fn big_button(ui: &mut Ui, icon: Icon, label: &str) -> egui::Response {
    let c = p();
    let ink = c.white(0.9);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), FontId::proportional(13.0), ink);
    let size = vec2(42.0 + galley.size().x, 34.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let fill = c.white(if response.hovered() { 0.1 } else { 0.06 });
    ui.painter().rect(rect, CornerRadius::same(3), fill, Stroke::new(1.0, c.white(0.1)), StrokeKind::Inside);
    let (icon_x, text_x) = if icon == Icon::ExternalLink { (rect.right() - 24.0, rect.left() + 10.0) } else { (rect.left() + 10.0, rect.left() + 32.0) };
    icons::paint(ui, Rect::from_min_size(pos2(icon_x, rect.center().y - 7.0), Vec2::splat(14.0)), icon, 14.0, ink);
    ui.painter().galley(pos2(text_x, rect.center().y - galley.size().y / 2.0), galley, ink);
    response
}

/// The 24 px search field over the repositories list.
fn search_field(ui: &mut Ui, text: &mut String, hint: &str, focus: bool) {
    let c = p();
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 24.0), Sense::hover());
    let edit_rect = Rect::from_min_max(pos2(rect.left() + 22.0, rect.top()), rect.right_bottom());
    let id = egui::Id::unique("start_search");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_columns_share_the_available_space() {
        for room in [585.0, 715.0, 1478.0] {
            let (repos, integrations) = start_columns(room);
            assert!(repos > integrations);
            assert!(integrations >= 188.0);
            assert!((repos + 61.0 + integrations - room).abs() < 0.01);
        }
    }

    #[test]
    fn toast_position_follows_the_preferred_corner() {
        for (location, top, right) in [
            ("top-left", true, false),
            ("top-right", true, true),
            ("bottom-left", false, false),
            ("bottom-right", false, true),
        ] {
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let mut settings = crate::settings::Store::in_memory();
            settings.set("notificationLocation", location);
            let mut app = App::new(&cc, crate::app::StartOptions::default(), settings);
            app.below_tabs = 86.0;
            app.toast(false, "Fixture failed");
            let render = || egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(930.0, 669.0))),
                ..Default::default()
            };
            ctx.run_ui(render(), |_| app.draw_toasts(&ctx)).textures_delta.clear();
            let mut output = ctx.run_ui(render(), |_| app.draw_toasts(&ctx));
            output.textures_delta.clear();
            let text = output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text().contains("Fixture failed") => Some(text.pos),
                _ => None,
            }).expect("toast text is rendered");
            assert_eq!(text.y < 334.0, top, "{location}");
            assert_eq!(text.x > 465.0, right, "{location}");
        }
    }
}
