//! Stage 4: identity, credentials, SSH keys and GPG signing — all stored in Git's own config and
//! the user's ~/.ssh folder, so other Git tools see the same settings.

use crate::git;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[derive(Serialize, Default)]
pub struct SshKey {
    pub path: String,
    pub kind: String,
    pub public: String,
}

#[derive(Serialize, Default)]
pub struct GpgKey {
    pub id: String,
    pub fingerprint: String,
    pub uid: String,
}

#[derive(Serialize, Default)]
pub struct AccountInfo {
    /// Global identity (and this repository's override, if any).
    pub name: String,
    pub email: String,
    pub repo_name: String,
    pub repo_email: String,
    pub credential_helper: String,
    /// Git Credential Manager is installed.
    pub gcm: bool,
    pub ssh_dir: String,
    pub ssh_keys: Vec<SshKey>,
    pub ssh_keygen: bool,
    pub gpg: bool,
    pub gpg_keys: Vec<GpgKey>,
    pub signing_key: String,
    pub sign_commits: bool,
    pub signing_format: String,
}

fn cmd(program: &str) -> Command {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut c = Command::new(program);
    #[cfg(windows)]
    c.creation_flags(0x0800_0000);
    c
}

fn home() -> PathBuf {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default()
}



pub fn info(dir: &Path) -> AccountInfo {
    let ssh_dir = home().join(".ssh");
    let mut ssh_keys = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&ssh_dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "pub").unwrap_or(false) {
                let public = std::fs::read_to_string(&p).unwrap_or_default().trim().to_string();
                let kind = public.split_whitespace().next().unwrap_or("").to_string();
                ssh_keys.push(SshKey { path: p.with_extension("").to_string_lossy().into_owned(), kind, public });
            }
        }
    }
    let keys: Vec<String> = ["user.name", "user.email", "credential.helper", "user.signingkey", "commit.gpgsign", "gpg.format", "gpg.program"].map(str::to_owned).to_vec();
    let global = crate::prefs::config_get(Some(dir), "global", &keys);
    let local = crate::prefs::config_get(Some(dir), "local", &keys);
    let effective = crate::prefs::config_get(Some(dir), "", &keys);
    let config = |scope: &str, key: &str| {
        let values = match scope { "--global" => &global, "--local" => &local, _ => &effective };
        values.get(key).cloned().unwrap_or_default()
    };
    let gpg_program = config("", "gpg.program");
    let gpg_out = cmd(if gpg_program.is_empty() { "gpg" } else { &gpg_program }).args(["--list-secret-keys", "--keyid-format", "LONG", "--with-colons"]).output();
    let gpg = gpg_out.as_ref().is_ok_and(|out| out.status.success());
    let gpg_keys = gpg_out.map(|out| parse_gpg_keys(&String::from_utf8_lossy(&out.stdout))).unwrap_or_default();
    AccountInfo {
        name: config("--global", "user.name"),
        email: config("--global", "user.email"),
        repo_name: config("--local", "user.name"),
        repo_email: config("--local", "user.email"),
        credential_helper: config("", "credential.helper"),
        gcm: git::run(dir, &["credential-manager", "--version"]).ok,
        ssh_dir: ssh_dir.to_string_lossy().into_owned(),
        ssh_keys,
        ssh_keygen: cmd("ssh-keygen").arg("-?").output().is_ok(),
        gpg,
        gpg_keys,
        signing_key: config("", "user.signingkey"),
        sign_commits: config("", "commit.gpgsign") == "true",
        signing_format: {
            let f = config("", "gpg.format");
            if f.is_empty() { "openpgp".into() } else { f }
        },
    }
}

/// Set (or with an empty value, remove) a Git setting. scope: "global" or "local".
pub fn set(dir: &Path, scope: &str, key: &str, value: &str) -> Result<(), String> {
    let s = format!("--{scope}");
    if value.is_empty() {
        // Unsetting a key that isn't there is fine (git exits with 5 and says nothing).
        let o = git::run(dir, &["config", &s, "--unset-all", key]);
        return if o.ok || o.message().is_empty() { Ok(()) } else { Err(o.message()) };
    }
    let o = git::run(dir, &["config", &s, key, value]);
    if o.ok {
        Ok(())
    } else {
        Err(o.message())
    }
}

#[derive(Serialize)]
pub struct GeneratedSshKey {
    pub public_key: String,
    pub private_path: String,
    pub agent_added: bool,
    pub agent_error: Option<String>,
}

const ASKPASS_SECRET: &str = "GITCRAFT_SSH_ASKPASS_PASSPHRASE";

pub fn handle_ssh_askpass() -> bool {
    let Some(passphrase) = std::env::var_os(ASKPASS_SECRET) else { return false; };
    println!("{}", passphrase.to_string_lossy());
    true
}

fn ssh_askpass(command: &mut Command, passphrase: &str) -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    command.env("SSH_ASKPASS", executable).env("SSH_ASKPASS_REQUIRE", "force")
        .env(ASKPASS_SECRET, passphrase);
    Ok(())
}

pub fn generate_ssh_key(email: &str, name: &str, passphrase: &str, add_to_agent: bool) -> Result<GeneratedSshKey, String> {
    let name = if name.trim().is_empty() { "id_ed25519" } else { name.trim() };
    if !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')) {
        return Err("Use letters, numbers, hyphens or underscores in the SSH key name.".into());
    }
    if passphrase.contains(['\r', '\n', '\0']) {
        return Err("An SSH passphrase cannot contain line breaks or NUL characters.".into());
    }
    let dir = home().join(".ssh");
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let file = dir.join(name);
    let public_path = file.with_extension("pub");
    if file.exists() || public_path.exists() { return Err(format!("{} already exists", file.display())); }
    let temporary = tempfile::tempdir_in(&dir).map_err(|error| error.to_string())?;
    let staged = temporary.path().join("key");
    let staged_public = staged.with_extension("pub");
    let mut command = cmd("ssh-keygen");
    command.args(["-q", "-t", "ed25519", "-C", email, "-f"]).arg(&staged);
    if passphrase.is_empty() { command.args(["-N", ""]); }
    else { ssh_askpass(&mut command, passphrase)?; }
    let output = crate::read_jobs::run(command, 64 * 1024, None);
    if !output.ok { return Err(format!("Could not generate the SSH key. {}", output.message())); }
    let public_key = std::fs::read_to_string(&staged_public).map_err(|error| error.to_string())?.trim().to_string();
    tempfile::TempPath::try_from_path(staged).and_then(|path| path.persist_noclobber(&file).map_err(std::io::Error::from)).map_err(|error| error.to_string())?;
    if let Err(error) = tempfile::TempPath::try_from_path(staged_public).and_then(|path| path.persist_noclobber(&public_path).map_err(std::io::Error::from)) {
        let _ = std::fs::remove_file(&file);
        return Err(error.to_string());
    }

    let mut agent_error = None;
    if add_to_agent {
        let mut command = cmd("ssh-add");
        #[cfg(target_os = "macos")]
        if !passphrase.is_empty() { command.arg("--apple-use-keychain"); }
        command.arg(&file);
        let result = ssh_askpass(&mut command, passphrase).and_then(|()| {
            let output = crate::read_jobs::run(command, 64 * 1024, None);
            if output.ok { Ok(()) } else { Err(output.message()) }
        });
        agent_error = result.err().map(|error| format!("The SSH key was created. Start your SSH agent to add it. {error}"));
    }
    Ok(GeneratedSshKey {
        public_key, private_path: file.to_string_lossy().into_owned(),
        agent_added: add_to_agent && agent_error.is_none(), agent_error,
    })
}

pub fn read_public_key(path: &str) -> Result<String, String> {
    let path = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")).map(|rest| home().join(rest)).unwrap_or_else(|| path.into());
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let fields: Vec<_> = text.split_whitespace().collect();
    if fields.len() < 2 || !(fields[0].starts_with("ssh-") || fields[0].starts_with("ecdsa-") || fields[0].starts_with("sk-")) {
        return Err("Choose an SSH public key file (.pub).".into());
    }
    Ok(text.trim().into())
}

pub fn parse_gpg_keys(text: &str) -> Vec<GpgKey> {
    let mut keys = Vec::new(); let mut id = String::new(); let mut fingerprint = String::new();
    for line in text.lines() {
        let fields: Vec<_> = line.split(':').collect();
        match fields.first().copied() {
            Some("sec") => {
                id.clear();
                fingerprint.clear();
                if fields.len() > 11 && !matches!(fields[1], "e" | "r" | "d" | "i")
                    && fields[11].contains('S') && !fields[11].contains('D') {
                    id = fields[4].into();
                }
            },
            Some("fpr") if fields.len() > 9 && !id.is_empty() && fingerprint.is_empty() => fingerprint = fields[9].into(),
            Some("uid") if fields.len() > 9 && !id.is_empty() && !fingerprint.is_empty() && !matches!(fields[1], "e" | "r" | "d" | "i") => { keys.push(GpgKey { id: std::mem::take(&mut id), fingerprint: std::mem::take(&mut fingerprint), uid: fields[9].into() }); },
            _ => {},
        }
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::parse_gpg_keys;

    #[test]
    fn gpg_picker_only_lists_usable_signing_keys() {
        let mut input = String::new();
        for (validity, id, capabilities) in [
            ("u", "usable", "cSC"), ("e", "expired", "scESC"), ("r", "revoked", "scSC"),
            ("d", "disabled", "scSCD"), ("u", "encrypt-only", "eE"),
        ] {
            input.push_str(&format!("sec:{validity}:256:22:{id}:0:0:::::{capabilities}:
fpr:::::::::{id}-fingerprint:
uid:u::::::::{id} UID:
"));
        }
        let keys = parse_gpg_keys(&input);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].id, "usable");
        assert_eq!(keys[0].fingerprint, "usable-fingerprint");
    }
}
