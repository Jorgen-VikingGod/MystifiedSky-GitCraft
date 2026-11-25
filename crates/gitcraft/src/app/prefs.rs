//! Preferences, profiles and workspaces, and the bell's notifications.

use super::App;
use super::icons::{self, Icon};
use super::theme::{WARN, p};
use super::widgets::Tip;
use super::widgets;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum NoticeFilter {
    #[default]
    All,
    Read,
    Unread,
}

impl NoticeFilter {
    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Read => "Read",
            Self::Unread => "Unread",
        }
    }

    fn matches(self, read: bool) -> bool {
        match self {
            Self::All => true,
            Self::Read => read,
            Self::Unread => !read,
        }
    }

    fn empty_message(self) -> &'static str {
        match self {
            Self::All => "You have no notifications",
            Self::Read => "You have no read notifications",
            Self::Unread => "You have no unread notifications",
        }
    }

    fn mark_all(self, notices: &mut [super::Notice], read: bool) {
        for notice in notices.iter_mut().filter(|notice| self.matches(notice.read)) {
            notice.read = read;
        }
    }
}

impl App {
    /// Tell the backend the preferences that change how it works: the Git program, SSH, logging,
    /// encodings, the services and AI. Runs when the preferences changed.
    pub(crate) fn sync_backend(&mut self) {
        if self.synced == self.settings.revision {
            return;
        }
        self.synced = self.settings.revision;
        let s = &self.settings;
        let encodings: serde_json::Map<String, serde_json::Value> = s
            .get::<serde_json::Map<String, serde_json::Value>>("repoPrefs")
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(path, p)| p.get("encoding").and_then(|e| e.as_str()).filter(|e| !e.is_empty()).map(|e| (path, serde_json::Value::String(e.to_owned()))))
            .collect();
        let runtime = serde_json::json!({
            "gitPath": s.string("gitPath", ""),
            "sshUseAgent": s.flag("sshUseAgent", true),
            "sshPrivateKey": s.string("sshPrivateKey", ""),
            "extendedLog": s.flag("extendedLog", false),
            "encodings": encodings,
        });
        if let Ok(r) = serde_json::from_value(runtime) {
            gitcraft_core::prefs::set_runtime(r);
        }
        let mut integrations = s.get::<serde_json::Value>("integrations").unwrap_or_else(|| serde_json::json!({}));
        if integrations.get("gitlabHost").and_then(|v| v.as_str()).is_none_or(str::is_empty) {
            integrations["gitlabHost"] = serde_json::Value::String(s.string("gitlabHost", ""));
        }
        if let Ok(c) = serde_json::from_value(integrations) {
            gitcraft_core::hosting::set_config(c);
        }
        gitcraft_core::ai::set_config(self.ai_config());
    }

    pub(crate) fn set_theme(&mut self, id: &str) {
        self.settings.set("theme", id);
        super::theme::choose(super::theme::Theme::from_id(id), &self.ctx);
    }

    pub(crate) fn repo_management(&mut self) {
        self.repo_management_dialog();
    }

    fn profiles(&self) -> Vec<serde_json::Value> {
        self.settings.get_root("profiles").unwrap_or_default()
    }

    /// Switch profile: keep this profile's settings, load the other's, and apply its Git identity.
    pub(crate) fn switch_profile(&mut self, index: usize) {
        let mut profiles = self.profiles();
        if profiles.is_empty() {
            return self.toast(false, "Add a profile in Preferences > Profiles first.");
        }
        let active = self.settings.active_profile_index();
        if index == active || index >= profiles.len() {
            return;
        }
        let current = self.settings.settings_value();
        let identity = gitcraft_core::prefs::config_get(None, "global", &["user.name".into(), "user.email".into()]);
        if let Some(p) = profiles.get_mut(active) {
            p["settings"] = current.clone();
            p["gitName"] = serde_json::json!(identity.get("user.name").cloned().unwrap_or_default());
            p["gitEmail"] = serde_json::json!(identity.get("user.email").cloned().unwrap_or_default());
        }
        let target = profiles[index].clone();
        let mut next = target.get("settings").cloned().filter(|v| v.is_object()).unwrap_or_else(|| serde_json::json!({}));
        next["recents"] = current.get("recents").cloned().unwrap_or_else(|| serde_json::json!([]));
        let keep = next.get("keepGitconfigUpdated").and_then(|v| v.as_bool()).unwrap_or(true);
        if keep {
            let (name, email) = (target["gitName"].as_str().unwrap_or("").to_owned(), target["gitEmail"].as_str().unwrap_or("").to_owned());
            let dir = std::path::Path::new(".");
            if let Err(e) = gitcraft_core::accounts::set(dir, "global", "user.name", &name).and_then(|_| gitcraft_core::accounts::set(dir, "global", "user.email", &email)) {
                let _ = gitcraft_core::accounts::set(dir, "global", "user.name", identity.get("user.name").map_or("", |s| s.as_str()));
                let _ = gitcraft_core::accounts::set(dir, "global", "user.email", identity.get("user.email").map_or("", |s| s.as_str()));
                return self.toast(false, format!("Could not switch profile: {e}"));
            }
        }
        self.settings.set_root("profiles", profiles);
        self.settings.set_root("activeProfile", target.get("id").cloned().unwrap_or_default());
        self.settings.replace_settings(next);
        let theme = self.settings.theme();
        super::theme::choose(super::theme::Theme::from_id(&theme), &self.ctx);
        self.ctx.set_zoom_factor(self.settings.zoom());
        self.prefs_data.account = None;
        self.prefs_data.config.clear();
        self.reload_all();
        self.toast(true, format!("Switched to profile {}", target["name"].as_str().unwrap_or("")));
    }

    pub(crate) fn remove_profile(&mut self, index: usize) {
        let mut profiles = self.profiles();
        if index == self.settings.active_profile_index() || profiles.len() <= 1 {
            return self.toast(false, "Switch to another profile before deleting this one");
        }
        if index < profiles.len() {
            profiles.remove(index);
            self.settings.set_root("profiles", profiles);
        }
    }

    pub(crate) fn add_profile_dialog(&mut self) {
        use super::dialogs::{Dialog, Field, FieldKind};
        let icons: Vec<(String, String)> = (0..4).map(|i| (i.to_string(), format!("Icon {}", i + 1))).collect();
        let fields = vec![
            Field::new("icon", "Select a profile icon", FieldKind::Select(icons)),
            Field::new("name", "Profile Name", FieldKind::Text).placeholder("New Profile"),
            Field::new("gitName", "Author Name", FieldKind::Text),
            Field::new("gitEmail", "Author Email", FieldKind::Text),
            Field::new("copy", "Copy settings from current profile", FieldKind::Check),
        ];
        let d = Dialog::form("Add Profile", fields, "Add Profile", |app, v| {
            let mut profiles = app.profiles();
            if profiles.is_empty() {
                // The profile in use becomes the first one, so it can be switched back to.
                let identity = gitcraft_core::prefs::config_get(None, "global", &["user.name".into(), "user.email".into()]);
                profiles.push(serde_json::json!({"id": "default", "name": "Default Profile", "gitName": identity.get("user.name").cloned().unwrap_or_default(), "gitEmail": identity.get("user.email").cloned().unwrap_or_default(), "icon": 0, "settings": app.settings.settings_value()}));
                app.settings.set_root("activeProfile", "default");
            }
            let id = format!("p{}", chrono::Local::now().timestamp_millis());
            let settings = if v.flag("copy") { app.settings.settings_value() } else { serde_json::Value::Null };
            let name = if v.text("name").trim().is_empty() { "Profile".to_owned() } else { v.text("name").trim().to_owned() };
            profiles.push(serde_json::json!({"id": id, "name": name, "gitName": v.text("gitName").trim(), "gitEmail": v.text("gitEmail").trim(), "icon": v.text("icon").parse::<u32>().unwrap_or(0), "settings": settings}));
            app.settings.set_root("profiles", profiles);
        })
        .width(584.0)
        .text_value("icon", "0")
        .flag_value("copy", true)
        .checked(|v| v.text("name").trim().is_empty().then(String::new));
        self.dialog = Some(d);
    }

    pub(crate) fn open_workspace(&mut self, index: usize) {
        let ws: Vec<serde_json::Value> = self.settings.get("workspaces").unwrap_or_default();
        let Some(w) = ws.get(index) else { return };
        let repos: Vec<String> = w["repos"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default();
        self.spawn(move || {
            let checked: Vec<_> = repos.into_iter().map(|path| {
                gitcraft_core::git::repo_root(std::path::Path::new(&path))
                    .map(|root| root.to_string_lossy().into_owned())
                    .ok_or_else(|| format!("{path} is not a Git repository"))
            }).collect();
            super::Msg::Apply(Box::new(move |app| {
                for repo in checked {
                    match repo {
                        Ok(path) => app.open_repo(&path),
                        Err(error) => app.toast(false, error),
                    }
                }
            }))
        });
    }

    pub(crate) fn workspaces_dialog(&mut self) {
        use super::dialogs::{Body, Dialog};
        use super::widgets::Kind;
        let mut filter = String::new();
        let draw = move |app: &mut App, ui: &mut egui::Ui| {
            let mut workspaces: Vec<serde_json::Value> = app.settings.get("workspaces").unwrap_or_default();
            widgets::field(ui, &mut filter, "Filter workspaces", ui.available_width(), 30.0, 13.0);
            ui.add_space(8.0);
            if workspaces.is_empty() {
                ui.label("No workspaces yet.");
            }
            let query = filter.trim().to_lowercase();
            let mut any_shown = false;
            let mut deleted = None;
            for (index, workspace) in workspaces.iter().enumerate() {
                let name = workspace["name"].as_str().unwrap_or("Workspace");
                if !name.to_lowercase().contains(&query) {
                    continue;
                }
                any_shown = true;
                let count = workspace["repos"].as_array().map_or(0, Vec::len);
                let mut open = false;
                ui.push_id(index, |ui| {
                    ui.horizontal(|ui| {
                        ui.set_height(34.0);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if widgets::button(ui, Kind::Danger, None, "Delete", 26.0, true)
                                .tip(format!("Delete the {name} workspace")).clicked()
                            {
                                deleted = Some(index);
                            }
                            open = widgets::button(ui, Kind::Primary, None, "Open all", 26.0, true)
                                .tip(format!("Open repositories in {name}")).clicked();
                            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                ui.add(egui::Label::new(format!("{name} · {count} repositories")).truncate());
                            });
                        });
                    });
                    ui.separator();
                });
                if open {
                    app.open_workspace(index);
                    return true;
                }
            }
            if let Some(index) = deleted {
                workspaces.remove(index);
                app.settings.set("workspaces", workspaces);
            } else if !workspaces.is_empty() && !any_shown {
                ui.label("No matching workspaces.");
            }
            false
        };
        let d = Dialog::new("Workspaces", Body::Custom(Box::new(draw)))
            .width(560.0)
            .cancel_label("Close")
            .choice("New workspace…", Kind::Primary, |app, _| app.new_workspace_dialog());
        self.dialog = Some(d);
    }

    fn new_workspace_dialog(&mut self) {
        use super::dialogs::{Dialog, Field, FieldKind};
        let mut candidates: Vec<String> = self.tabs.iter().filter_map(|t| t.path().map(str::to_owned)).collect();
        for r in self.settings.recents() {
            if !candidates.iter().any(|path| super::same_folder(path, &r)) {
                candidates.push(r);
            }
        }
        let mut fields = vec![Field::new("name", "Workspace name", FieldKind::Text)];
        if candidates.is_empty() {
            fields.push(Field::new("", "Open some repositories first.", FieldKind::Note));
        }
        for (i, c) in candidates.iter().enumerate() {
            fields.push(Field::new(&format!("repo{i}"), c, FieldKind::Check));
        }
        let list = candidates.clone();
        let d = Dialog::form("New workspace", fields, "Create Workspace", move |app, v| {
            let repos: Vec<String> = list.iter().enumerate().filter(|(i, _)| v.flag(&format!("repo{i}"))).map(|(_, p)| p.clone()).collect();
            let mut ws: Vec<serde_json::Value> = app.settings.get("workspaces").unwrap_or_default();
            ws.push(serde_json::json!({"id": format!("w{}", chrono::Local::now().timestamp_millis()), "name": v.text("name").trim(), "repos": repos}));
            app.settings.set("workspaces", ws);
            app.workspaces_dialog();
        })
        .checked(move |v| (v.text("name").trim().is_empty() || !(0..candidates.len()).any(|i| v.flag(&format!("repo{i}")))).then(String::new));
        self.dialog = Some(d);
    }

    pub(crate) fn ssh_key_dialog(&mut self, email: String) {
        use super::dialogs::{Dialog, Field, FieldKind};
        let name = format!("gitcraft_{}", chrono::Local::now().timestamp());
        let fields = vec![
            Field::new("name", "Key filename", FieldKind::Text),
            Field::new("passphrase", "Passphrase (optional)", FieldKind::Password),
            Field::new("agent", "Add to the local SSH agent", FieldKind::Check),
        ];
        let d = Dialog::form("Generate SSH key", fields, "Generate", move |app, v| {
            let (name, pass, agent) = (v.text("name"), v.text("passphrase"), v.flag("agent"));
            let email = email.clone();
            app.spawn(move || {
                let r = gitcraft_core::accounts::generate_ssh_key(&email, &name, &pass, agent);
                super::Msg::Apply(Box::new(move |app: &mut App| match r {
                    Ok(key) => {
                        app.settings.set("sshUseAgent", key.agent_added);
                        app.settings.set("sshPrivateKey", key.private_path.clone());
                        app.settings.set("sshPublicKey", format!("{}.pub", key.private_path));
                        app.ctx.copy_text(key.public_key);
                        match key.agent_error {
                            Some(e) => app.toast(false, e),
                            None => app.toast(true, "SSH key created and its public key copied"),
                        }
                        app.prefs_data.account = None;
                    }
                    Err(e) => app.toast(false, e),
                }))
            });
        })
        .text_value("name", &name)
        .flag_value("agent", true);
        self.dialog = Some(d);
    }

    /// File > Open Repo in New Window: this tab's repository, or one picked now.
    pub(crate) fn open_in_new_window(&mut self) {
        match self.active_tab().and_then(|t| t.path().map(str::to_owned)) {
            Some(path) => self.repository_window(&path),
            None => self.pick_folder("Choose a repository to open in a new window", super::dialogs::Pick::NewWindow),
        }
    }

    /// Start GitCraft again with only this repository, in a window of its own.
    pub(crate) fn repository_window(&mut self, path: &str) {
        if self.screenshot.is_some() {
            return;
        }
        let path = path.to_owned();
        self.spawn(move || {
            let r = (|| {
                let root = gitcraft_core::git::repo_root(std::path::Path::new(&path)).ok_or("Choose the folder of an existing Git repository.")?;
                let exe = std::env::current_exe().map_err(|e| e.to_string())?;
                std::process::Command::new(exe).arg("--new-window").arg("--").arg(&root).spawn().map_err(|e| format!("Could not open a new window: {e}"))?;
                Ok::<_, String>(())
            })();
            super::Msg::Apply(Box::new(move |app: &mut App| {
                if let Err(e) = r {
                    app.toast(false, e);
                }
            }))
        });
    }

    pub(crate) fn sign_out_all(&mut self) {
        let gitlab = self.settings.string("gitlabHost", "");
        self.spawn(move || {
            let failed = gitcraft_core::hosting::SERVICES.iter().filter(|id| gitcraft_core::hosting::save_secret_manually(&gitcraft_core::hosting::secret_name(id), "").is_err()).count();
            gitcraft_core::hosting::sync_git_credentials(&gitlab);
            super::Msg::Apply(Box::new(move |app: &mut App| {
                app.hosting_changed();
                if failed > 0 {
                    app.toast(false, format!("Could not remove credentials for {failed} integration(s). Try again."));
                } else {
                    app.toast(true, "Signed out of every integration");
                }
            }))
        });
    }

    /// Whether the bell lists the update: while it downloads and once it is ready to install.
    pub(crate) fn update_row(&self) -> bool {
        matches!(self.update.phase, super::updater::Phase::Downloading | super::updater::Phase::Ready)
    }

    /// The bell's menu. It keeps its own open state, as egui holds one open popup at a time and
    /// its filter and options drop-downs open on top of it. It closes on the bell, Escape, or a
    /// click outside it and its drop-downs.
    pub(crate) fn notifications(&mut self, ui: &mut egui::Ui, bell: &egui::Response) {
        let open_id = egui::Id::NULL.with("notifications_open");
        let mut open = ui.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false);
        if bell.clicked() {
            open = !open;
        }
        if open {
            let mut drops = Vec::new();
            let mut close = false;
            let shown = egui::Popup::menu(bell)
                .open(true)
                .close_behavior(egui::PopupCloseBehavior::IgnoreClicks)
                .show(|ui| self.notification_menu(ui, &mut drops, &mut close));
            let menu = shown.map(|s| s.response.rect);
            let (clicked, at, escape) = ui.input(|i| (i.pointer.any_click(), i.pointer.interact_pos(), i.key_pressed(egui::Key::Escape)));
            let inside = |p: egui::Pos2| bell.rect.contains(p) || menu.is_some_and(|r| r.contains(p)) || drops.iter().any(|r: &egui::Rect| r.contains(p));
            let outside = clicked && at.is_some_and(|p| !inside(p));
            if close || outside || (escape && drops.is_empty()) {
                open = false;
            }
        }
        ui.data_mut(|d| d.insert_temp(open_id, open));
    }

    /// The bell menu's content: notifications newest first, a filter, and mark all read or unread.
    /// `drops` gets the rectangles of its open drop-downs; `close` is set to close the menu.
    fn notification_menu(&mut self, ui: &mut egui::Ui, drops: &mut Vec<egui::Rect>, close: &mut bool) {
        let c = p();
        ui.set_width((ui.ctx().content_rect().width() - 24.0).min(340.0));
        let filter_id = egui::Id::NULL.with("notice_filter");
        let mut filter: NoticeFilter = ui.data(|d| d.get_temp(filter_id)).unwrap_or_default();
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Notifications").font(super::fonts::semibold(13.0)).color(c.fg));
            ui.add_space(8.0);
            let pick = ui.add(egui::Button::new(egui::RichText::new(format!("{} ▾", filter.label())).size(12.5).color(c.white(0.8))).fill(egui::Color32::TRANSPARENT).stroke(egui::Stroke::new(1.0, c.white(0.15))).min_size(egui::vec2(0.0, 22.0))).tip("Filter notifications");
            let drop = egui::Popup::menu(&pick).show(|ui| {
                for choice in [NoticeFilter::All, NoticeFilter::Read, NoticeFilter::Unread] {
                    if ui.selectable_label(choice == filter, choice.label()).clicked() {
                        filter = choice;
                    }
                }
            });
            drops.extend(drop.map(|d| d.response.rect));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let more = ui.add(egui::Button::new(egui::RichText::new("⋮").size(15.0).color(c.white(0.7))).frame_when_inactive(false).min_size(egui::vec2(22.0, 22.0))).tip("Notification options");
                let drop = egui::Popup::menu(&more).align(egui::RectAlign::BOTTOM_END).show(|ui| {
                    let any_unread = filter.matches(false) && self.notices.iter().any(|n| !n.read);
                    let any_read = filter.matches(true) && self.notices.iter().any(|n| n.read);
                    if ui.add_enabled(any_unread, egui::Button::new("Mark all as read")).clicked() {
                        filter.mark_all(&mut self.notices, true);
                    }
                    if ui.add_enabled(any_read, egui::Button::new("Mark all as unread")).clicked() {
                        filter.mark_all(&mut self.notices, false);
                    }
                    if ui.button("Notification Settings").clicked() {
                        self.open_prefs(Some("notifications"));
                        *close = true;
                    }
                });
                drops.extend(drop.map(|d| d.response.rect));
            });
        });
        ui.data_mut(|d| d.insert_temp(filter_id, filter));
        ui.separator();
        // The update, above the notifications while one is downloading or ready to install.
        let update_row = self.update_row() && filter.matches(false);
        if update_row {
            let ready = self.update.phase == super::updater::Phase::Ready;
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(14.0, 16.0), egui::Sense::hover());
                icons::paint(ui, r, Icon::TriangleAlert, 14.0, WARN);
                ui.label(egui::RichText::new(if ready { "Restart and Install Update" } else { "Downloading Update..." }).size(12.5).color(c.white(1.0)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if ready { "Restart" } else { "Update now" };
                    let link = ui.add_enabled(ready, egui::Label::new(egui::RichText::new(label).size(12.5).color(c.link).underline()).sense(egui::Sense::click()));
                    if link.clicked() {
                        let ctx = ui.ctx().clone();
                        self.install_update(&ctx);
                        *close = true;
                    }
                });
            });
            ui.separator();
        }
        let shown: Vec<usize> = (0..self.notices.len()).rev().filter(|i| filter.matches(self.notices[*i].read)).collect();
        if shown.is_empty() {
            if update_row {
                return;
            }
            ui.vertical_centered(|ui| ui.label(egui::RichText::new(filter.empty_message()).size(12.5).color(c.fg3)));
            return;
        }
        let mut remove = None;
        egui::ScrollArea::vertical().max_height((ui.ctx().content_rect().height() - 168.0).max(120.0)).show(ui, |ui| {
            for i in shown {
                let n = &mut self.notices[i];
                ui.horizontal_top(|ui| {
                    if !n.read {
                        let (r, _) = ui.allocate_exact_size(egui::vec2(7.0, 16.0), egui::Sense::hover());
                        ui.painter().circle_filled(r.center(), 3.5, egui::Color32::from_rgb(0x2f, 0x81, 0xf7));
                    }
                    let text: String = if n.text.chars().count() > 240 { n.text.chars().take(239).collect::<String>() + "…" } else { n.text.clone() };
                    ui.add(egui::Label::new(egui::RichText::new(text).size(12.5).color(if n.read { c.white(0.6) } else { c.white(1.0) })).wrap());
                });
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(widgets::format_date(n.at, "MM/DD/YYYY h:mm A")).size(11.0).color(c.fg3));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if notice_button(ui, Icon::Trash2, "Remove notification").clicked() {
                            remove = Some(i);
                        }
                        let (icon, label) = if n.read { (Icon::Undo2, "Mark as unread") } else { (Icon::Check, "Mark as read") };
                        if notice_button(ui, icon, label).clicked() {
                            n.read = !n.read;
                        }
                    });
                });
                ui.separator();
            }
        });
        if let Some(i) = remove {
            self.notices.remove(i);
        }
    }
}

fn notice_button(ui: &mut egui::Ui, icon: Icon, label: &str) -> egui::Response {
    let c = p();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
    icons::paint(ui, rect, icon, 13.0, if response.hovered() || response.has_focus() { c.fg } else { c.fg2 });
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    response.tip(label)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Notice, StartOptions};

    #[test]
    fn failure_toasts_stay_in_the_notification_inbox() {
        let ctx = eframe::CreationContext::_new_kittest(egui::Context::default());
        let mut app = App::new(&ctx, StartOptions::default(), crate::settings::Store::in_memory());
        app.toast(true, "success");
        app.toast(false, "operation failed");
        assert_eq!(app.notices.len(), 1);
        assert_eq!(app.notices[0].text, "operation failed");
        assert!(!app.notices[0].read);
    }

    #[test]
    fn notification_bulk_actions_use_the_list_filter() {
        let mut notices = [
            Notice { text: "already read".into(), at: 0, read: true },
            Notice { text: "unread".into(), at: 1, read: false },
        ];
        NoticeFilter::Read.mark_all(&mut notices, true);
        assert_eq!(notices.each_ref().map(|notice| notice.read), [true, false]);
        NoticeFilter::Unread.mark_all(&mut notices, false);
        assert_eq!(notices.each_ref().map(|notice| notice.read), [true, false]);
        NoticeFilter::Unread.mark_all(&mut notices, true);
        assert_eq!(notices.each_ref().map(|notice| notice.read), [true, true]);
        NoticeFilter::Read.mark_all(&mut notices, false);
        assert_eq!(notices.each_ref().map(|notice| notice.read), [false, false]);
    }

    #[test]
    fn workspace_rows_fit_together_and_delete_only_the_selected_group() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = App::new(&cc, StartOptions::default(), crate::settings::Store::in_memory());
        app.settings.set("workspaces", serde_json::json!([
            {"id": "alpha", "name": "Alpha", "repos": ["/alpha"]},
            {"id": "beta", "name": "Beta", "repos": ["/beta"]},
        ]));
        app.workspaces_dialog();
        let render = |app: &mut App, events| {
            let mut output = ctx.run_ui(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(930.0, 669.0))),
                events,
                ..Default::default()
            }, |_| app.draw_dialog(&ctx));
            output.textures_delta.clear();
            output
        };
        render(&mut app, vec![]);
        let output = render(&mut app, vec![]);
        let mut buttons: Vec<_> = output.shapes.iter().filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "Delete" => {
                let bounds = text.visual_bounding_rect();
                shape.clip_rect.contains(bounds.center()).then_some(bounds.center())
            }
            _ => None,
        }).collect();
        buttons.sort_by(|a, b| a.y.total_cmp(&b.y));
        assert_eq!(buttons.len(), 2);
        let row_gap = buttons[1].y - buttons[0].y;
        assert!((26.0..=45.0).contains(&row_gap), "workspace rows are {row_gap} points apart");
        let pos = buttons[1];
        for pressed in [true, false] {
            render(&mut app, vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE },
            ]);
        }
        let workspaces: Vec<serde_json::Value> = app.settings.get("workspaces").unwrap();
        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0]["id"], "alpha");
    }
}
