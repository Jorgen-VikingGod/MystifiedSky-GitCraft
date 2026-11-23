//! Patch files: save commits or uncommitted changes as a .patch, and apply one.

use super::App;
use super::dialogs::Pick;
use gitcraft_core::actions::Action;

/// A file name for a patch from a commit summary.
pub fn patch_name(summary: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in summary.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    let trimmed: String = out.trim_matches('-').chars().take(52).collect();
    format!("{}.patch", if trimmed.is_empty() { "changes" } else { &trimmed })
}

impl App {
    /// Ask where to save the commits `shas` (none: the uncommitted changes) as a patch.
    pub(crate) fn save_patch(&mut self, tab: u64, shas: Vec<String>, name: &str) {
        let dialog = craft_dialogs::FileDialog::save().title("Save patch").file_name(name).filter("Patch", &["patch", "diff"]);
        self.pickers.ask(dialog, Pick::SavePatch(tab, shas));
    }

    pub(crate) fn save_patch_to(&mut self, tab: u64, shas: Vec<String>, file: String) {
        let Some(path) = self.repo(tab).map(|r| r.path.clone()) else { return };
        self.spawn(move || {
            let result = gitcraft_core::extras::make_patch(std::path::Path::new(&path), &shas).and_then(|bytes| std::fs::write(&file, bytes).map_err(|e| e.to_string()));
            match result {
                Ok(()) => super::Msg::Toast(true, format!("Patch saved to {file}")),
                Err(e) => super::Msg::Toast(false, e),
            }
        });
    }

    pub(crate) fn apply_patch(&mut self, tab: u64) {
        let dialog = craft_dialogs::FileDialog::open().title("Apply a patch").filter("Patch", &["patch", "diff"]);
        self.pickers.ask(dialog, Pick::ApplyPatch(tab));
    }

    #[allow(dead_code)]
    pub(crate) fn apply_patch_file(&mut self, tab: u64, file: String) {
        self.act(tab, Action::ApplyPatchFile { file });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn patch_names_come_from_the_summary() {
        assert_eq!(super::patch_name("Fix: the login bug!"), "fix-the-login-bug.patch");
        assert_eq!(super::patch_name("!!!"), "changes.patch");
    }
}
