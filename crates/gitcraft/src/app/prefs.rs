//! Preferences, profiles and workspaces, and the bell's notifications.

use super::App;
use super::theme::p;
use super::widgets;

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
        if let Some(p) = self.prefs.as_mut() {
            p.account = None;
            p.config.clear();
        }
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
        for r in repos {
            self.open_repo_checked(&r);
        }
    }

    pub(crate) fn workspaces_dialog(&mut self) {
        use super::dialogs::{Body, Dialog};
        let ws: Vec<serde_json::Value> = self.settings.get("workspaces").unwrap_or_default();
        let rows = ws.iter().map(|w| (w["name"].as_str().unwrap_or("Workspace").to_owned(), format!("{} repositories", w["repos"].as_array().map_or(0, Vec::len)))).collect();
        let on_pick = std::rc::Rc::new(|app: &mut App, i: usize| {
            app.dialog = None;
            app.open_workspace(i);
        });
        let d = Dialog::new("Workspaces", Body::Pick { hint: "Filter workspaces".into(), filter: String::new(), rows, button: "Open all".into(), on_pick })
            .width(560.0)
            .cancel_label("Close")
            .choice("New workspace…", super::widgets::Kind::Primary, |app, _| app.new_workspace_dialog());
        self.dialog = Some(d);
    }

    fn new_workspace_dialog(&mut self) {
        use super::dialogs::{Dialog, Field, FieldKind};
        let mut candidates: Vec<String> = self.tabs.iter().filter_map(|t| t.path().map(str::to_owned)).collect();
        for r in self.settings.recents() {
            if !candidates.contains(&r) {
                candidates.push(r);
            }
        }
        let mut fields = vec![Field::new("name", "Workspace name", FieldKind::Text)];
        for (i, c) in candidates.iter().enumerate() {
            fields.push(Field::new(&format!("repo{i}"), c, FieldKind::Check));
        }
        let list = candidates.clone();
        let d = Dialog::form("New workspace", fields, "Create Workspace", move |app, v| {
            let repos: Vec<String> = list.iter().enumerate().filter(|(i, _)| v.flag(&format!("repo{i}"))).map(|(_, p)| p.clone()).collect();
            let mut ws: Vec<serde_json::Value> = app.settings.get("workspaces").unwrap_or_default();
            ws.push(serde_json::json!({"id": format!("w{}", chrono::Local::now().timestamp_millis()), "name": v.text("name").trim(), "repos": repos}));
            app.settings.set("workspaces", ws);
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
                        if let Some(p) = app.prefs.as_mut() {
                            p.account = None;
                        }
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

    /// The bell's menu: notifications newest first, a filter, and mark all read or unread.
    pub(crate) fn notification_menu(&mut self, ui: &mut egui::Ui) {
        let c = p();
        ui.set_width(340.0);
        let filter_id = egui::Id::NULL.with("notice_filter");
        let mut filter: u8 = ui.data(|d| d.get_temp(filter_id)).unwrap_or(0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Notifications").font(super::fonts::semibold(13.0)).color(c.fg));
            ui.add_space(8.0);
            let names = ["All", "Read", "Unread"];
            egui::ComboBox::from_id_salt("notice_filter_combo").selected_text(names[filter as usize]).width(80.0).show_ui(ui, |ui| {
                for (i, n) in names.iter().enumerate() {
                    ui.selectable_value(&mut filter, i as u8, *n);
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button("⋮", |ui| {
                    if ui.add_enabled(self.notices.iter().any(|n| !n.read), egui::Button::new("Mark all as read")).clicked() {
                        self.notices.iter_mut().for_each(|n| n.read = true);
                    }
                    if ui.add_enabled(self.notices.iter().any(|n| n.read), egui::Button::new("Mark all as unread")).clicked() {
                        self.notices.iter_mut().for_each(|n| n.read = false);
                    }
                    if ui.button("Notification Settings").clicked() {
                        self.open_prefs(Some("notifications"));
                    }
                });
            });
        });
        ui.data_mut(|d| d.insert_temp(filter_id, filter));
        ui.separator();
        let shown: Vec<usize> = (0..self.notices.len()).rev().filter(|i| match filter {
            1 => self.notices[*i].read,
            2 => !self.notices[*i].read,
            _ => true,
        }).collect();
        if shown.is_empty() {
            let text = ["You have no notifications", "You have no read notifications", "You have no unread notifications"][filter as usize];
            ui.label(egui::RichText::new(text).size(12.5).color(c.fg3));
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
                    if widgets::link(ui, if n.read { "Mark unread" } else { "Mark read" }, 11.0).clicked() {
                        n.read = !n.read;
                    }
                    if widgets::link(ui, "Remove", 11.0).clicked() {
                        remove = Some(i);
                    }
                });
                ui.separator();
            }
        });
        if let Some(i) = remove {
            self.notices.remove(i);
        }
    }
}
