//! The external editor, terminal and file manager (Preferences > External Tools).

use super::App;
use std::path::Path;

pub const EDITORS: [(&str, &str); 11] = [
    ("vscode", "Visual Studio Code"),
    ("cursor", "Cursor"),
    ("windsurf", "Windsurf"),
    ("zed", "Zed"),
    ("sublime", "Sublime Text"),
    ("notepadpp", "Notepad++"),
    ("idea", "IntelliJ IDEA"),
    ("webstorm", "WebStorm"),
    ("rider", "Rider"),
    ("vs", "Visual Studio"),
    ("custom", "Other (custom)"),
];

pub fn editor_name(tool: &str) -> &'static str {
    EDITORS.iter().find(|(id, _)| *id == tool).map_or("External Editor", |(_, n)| n)
}

impl App {
    fn tab_path(&self, tab: u64) -> Option<String> {
        self.repo(tab).map(|r| r.path.clone())
    }

    pub(crate) fn open_in_editor(&mut self, tab: u64, file: Option<&str>) {
        let Some(path) = self.tab_path(tab) else { return };
        let s = &self.settings;
        let result = gitcraft_core::external::open_editor(&path, file, &s.string("externalEditor", "vscode"), &s.string("editorPath", ""), &s.string("editorFileArgs", ""), &s.string("editorRepoArgs", ""));
        if let Err(e) = result {
            self.toast(false, e);
        }
    }

    pub(crate) fn open_terminal(&mut self, tab: u64) {
        let Some(path) = self.tab_path(tab) else { return };
        if self.settings.flag("useInAppTerminal", false) {
            self.settings.set("showTerminal", true);
            return;
        }
        let (tool, exe, args) = (self.settings.string("externalTerminal", "default"), self.settings.string("terminalPath", ""), self.settings.string("terminalArgs", ""));
        self.spawn(move || match gitcraft_core::external::open_terminal(&path, &tool, &exe, &args) {
            Ok(()) => super::Msg::Nothing,
            Err(e) => super::Msg::Toast(false, e),
        });
    }

    pub(crate) fn open_default(&mut self, tab: u64, file: &str) {
        let Some(path) = self.tab_path(tab) else { return };
        let result = gitcraft_core::files::repo_file(Path::new(&path), file).and_then(|p| if p.exists() { gitcraft_core::shell::open_path(&p) } else { Err(gitcraft_core::shell::OPEN_PATH_FAILED.into()) });
        if let Err(e) = result {
            self.toast(false, e);
        }
    }

    pub(crate) fn reveal_file(&mut self, tab: u64, file: &str) {
        let Some(path) = self.tab_path(tab) else { return };
        let file = file.to_owned();
        self.spawn(move || {
            let result = gitcraft_core::files::repo_file(Path::new(&path), &file).and_then(|p| {
                if !p.exists() {
                    return gitcraft_core::shell::open_path(p.parent().unwrap_or(Path::new(&path)));
                }
                gitcraft_core::shell::show_item_in_folder(&p)
            });
            match result {
                Ok(()) => super::Msg::Nothing,
                Err(e) => super::Msg::Toast(false, e),
            }
        });
    }

    pub(crate) fn copy_file_text(&mut self, tab: u64, file: &str, rev: Option<String>) {
        let Some(path) = self.tab_path(tab) else { return };
        let file = file.to_owned();
        self.spawn(move || match gitcraft_core::files::file_text(Path::new(&path), &file, rev.as_deref()) {
            Ok(text) => super::Msg::Copy(text),
            Err(e) => super::Msg::Toast(false, e),
        });
    }
}
