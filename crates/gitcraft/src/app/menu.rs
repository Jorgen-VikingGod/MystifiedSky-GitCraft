//! The File, Edit, View and Help menus (craft-menu), with GitCraft's items, order and
//! shortcuts. Items run the same commands as the keys and the command palette (see `keys.rs`).

use super::App;
use craft_menu::{CommandRef, Gate, Item, ItemRole, LayoutOptions, Menu, MenuBar, MenuBarUi, MenuRole, Node, Target};

pub const MOD: &str = if cfg!(target_os = "macos") { "⌘" } else { "Ctrl" };
pub const RELEASES_URL: &str = "https://github.com/MystifiedSky/GitCraft/releases";
const DISCORD_URL: &str = "https://discord.gg/BsUgBu65YG";

impl App {
    /// A menu item running `command`, with its key when it has one.
    fn item(&self, command: &str, label: &str) -> Item {
        let it = Item::new(CommandRef::new(command), label);
        match self.keys_of(command).and_then(|k| super::keys::chord(&k)) {
            Some(c) => it.shortcut(c),
            None => it,
        }
    }

    fn menus(&self) -> MenuBar {
        let repo = self.active_tab().and_then(|t| t.path()).is_some();
        let s = &self.settings;
        let editor = super::external_tools::editor_name(&s.string("externalEditor", "vscode"));
        let i = |id: &str, label: &str| -> Node { self.item(id, label).into() };
        let on = |id: &str, label: &str, enabled: bool| -> Node { self.item(id, label).enabled(enabled).into() };
        let file = vec![
            i("nav:newTab", "New Tab"),
            i("nav:closeTab", "Close Tab"),
            on("nav:reopenTab", "Reopen Closed Tab", !self.closed.is_empty()),
            Node::Separator,
            i("nav:clone", "Clone Repo..."),
            i("nav:init", "Init Repo..."),
            i("nav:openRepo", "Open Repo..."),
            i("app.window", "Open Repo in New Window…"),
            i("nav:repoManagement", "Open Repo Management"),
            on("repo:openInEditor", &format!("Open Repo in {editor}"), repo),
            Node::Separator,
            on("repo:terminal", "Open External Terminal", repo),
            on("repo:fileManager", "Open in File Manager", repo),
            Node::Separator,
            on("repo:history", "File History / Blame...", repo),
            Node::Separator,
            i("ui:settings", "Preferences..."),
            Node::Separator,
            i("app.update", "Check for Updates"),
            i("app.sign_out", "Sign Out"),
            self.item("app.exit", "Exit").role(ItemRole::Quit).into(),
        ];
        let edit = vec![on("repo:undo", "Undo", repo), on("repo:redo", "Redo", repo)];
        let view = vec![
            self.item("ui:leftPanel", "Show Left Panel").check(s.flag("showLeft", true)).into(),
            self.item("ui:detailPanel", "Show Commit Details Panel").check(s.flag("showRight", true)).into(),
            self.item("ui:toolbarLabels", "Show Toolbar Labels").check(s.flag("toolbarLabels", true)).into(),
            self.item("ui:terminalPanel", "Show Terminal Panel").check(s.flag("showTerminal", false)).into(),
            Node::Separator,
            i("ui:zoomIn", "Increase Zoom"),
            i("ui:zoomOut", "Decrease Zoom"),
            i("ui:zoomReset", "Reset Zoom"),
            Node::Separator,
            i("ui:fullscreen", "Toggle Full Screen"),
            i("ui:activityLog", "Activity Log"),
        ];
        let help = vec![
            i("palette:toggle", "Command Palette"),
            i("ui:shortcuts", "Keyboard Shortcuts"),
            i("help.plan", "Feature Plan"),
            i("help.diagnostics", "Copy diagnostics"),
            i("ui:activityLog", "View Activity Logs"),
            Node::Separator,
            i("help.release_notes", "View Release Notes"),
            i("help.discord", "Join Discord"),
            i("help.about", "About GitCraft"),
        ];
        MenuBar {
            menus: vec![
                Menu::new("File", file).role(MenuRole::File),
                Menu::new("Edit", edit).role(MenuRole::Edit),
                Menu::new("View", view).role(MenuRole::View),
                Menu::new("Help", help).role(MenuRole::Help),
            ],
            documents: Vec::new(),
        }
    }

    /// The 26 px menu row under the title bar.
    pub(super) fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let c = super::theme::p();
        let ctx = ui.ctx().clone();
        let chosen = egui::Panel::top("menu_bar")
            .exact_size(26.0)
            .frame(egui::Frame::new().fill(c.menubar).inner_margin(egui::Margin::symmetric(2, 0)))
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    // The menus stay in the window on every system, laid out as on Windows and Linux.
                    let target = if cfg!(windows) { Target::Windows } else { Target::Linux };
                    let bar = craft_menu::desktop_layout(&self.menus(), target, &LayoutOptions::new("GitCraft")).bar;
                    let gate = Gate { modal: self.dialog.is_some(), recording_shortcut: self.recording.is_some(), text_editing: ctx.egui_wants_keyboard_input() };
                    let style = craft_menu::BarStyle {
                        title_padding: egui::vec2(7.0, 3.0),
                        title_color: c.fg,
                        check_mark: "✓".into(),
                        radio_mark: "●".into(),
                        ..craft_menu::BarStyle::from_style(ui.style())
                    };
                    MenuBarUi::new(&bar).style(style).gate(gate).shrink_to_fit(f32::INFINITY).show(ui)
                })
                .inner
            })
            .inner;
        for command in chosen {
            self.run(&ctx, &command.id);
        }
    }

    /// Run a menu item: commands of the command list, and the items only the menus have.
    pub(super) fn run(&mut self, ctx: &egui::Context, id: &str) {
        match id {
            "repo.file_manager" | "repo:fileManager" => {
                if let Some(path) = self.active_tab().and_then(|t| t.path()).map(str::to_owned)
                    && let Err(e) = gitcraft_core::shell::open_path(std::path::Path::new(&path))
                {
                    self.toast(false, e);
                }
            }
            "app.exit" => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            "app.window" => self.open_in_new_window(),
            "app.update" => self.check_for_updates(true),
            "app.sign_out" => self.sign_out_all(),
            "help.plan" => self.plan_dialog(),
            "help.diagnostics" => self.copy_diagnostics(ctx),
            "help.release_notes" => self.open_url(RELEASES_URL),
            "help.discord" => self.open_url(DISCORD_URL),
            "help.about" => self.about_dialog(),
            other if super::keys::command(other).is_some() => self.run_command(other),
            _ => {}
        }
    }

    pub(super) fn open_url(&mut self, url: &str) {
        if let Err(e) = gitcraft_core::shell::open_external(url) {
            self.toast(false, e);
        }
    }

    /// Gather the diagnostics report away from the window (it runs Git) and copy it.
    fn copy_diagnostics(&mut self, ctx: &egui::Context) {
        let ctx = ctx.clone();
        let inbox = self.inbox.clone();
        std::thread::spawn(move || {
            let report = gitcraft_core::diagnostics::report(super::VERSION);
            ctx.copy_text(report);
            inbox.send(true, "Diagnostics copied");
            ctx.request_repaint();
        });
    }

    fn about_dialog(&mut self) {
        let text = format!("GitCraft\nVersion {}\n\nA graphical Git client. Every action runs the real Git command line.", super::VERSION);
        self.dialog = Some(super::dialogs::Dialog::info("About GitCraft", &text).width(380.0));
    }
}
