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

/// Write `bytes` to `file`, replacing an existing file only once the new one is completely written.
fn write_replacing(file: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    let file = if file.parent().is_some_and(|d| d.as_os_str().is_empty()) { std::path::Path::new(".").join(file) } else { file.to_path_buf() };
    gitcraft_core::files::write_replacing(&file, bytes).map_err(|e| format!("Could not save the patch: {e}"))
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
            let result = gitcraft_core::extras::make_patch(std::path::Path::new(&path), &shas).and_then(|bytes| write_replacing(std::path::Path::new(&file), &bytes));
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

    #[test]
    fn a_patch_replaces_the_old_file_only_when_written() {
        let dir = std::env::temp_dir().join(format!("gitcraft-patch-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.patch");
        std::fs::write(&file, "old").unwrap();
        super::write_replacing(&file, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "new");
        // A folder that doesn't exist: the write fails and leaves nothing behind.
        assert!(super::write_replacing(&dir.join("missing/b.patch"), b"x").is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
