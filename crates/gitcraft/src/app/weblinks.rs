//! Links to a commit, branch or tag on the remote's website.

use super::App;
use super::ctxmenu::{Entry, item, sub};
use gitcraft_core::session::RepoView;

#[derive(Clone, Debug)]
pub enum Target {
    Repo,
    Commit(String),
    Branch(String),
    Tag(String),
}

/// The website of a remote URL (`git@host:owner/repo.git`, `https://host/owner/repo.git`, Azure's
/// `git@ssh.dev.azure.com:v3/org/project/repo`), or None.
pub fn web_base(remote_url: &str) -> Option<String> {
    let url = remote_url.trim();
    if url.is_empty() {
        return None;
    }
    let (mut host, mut path): (String, String);
    let scp = url.split_once('@').filter(|(user, _)| !user.contains('/') && !user.contains(':')).and_then(|(_, rest)| rest.split_once(':')).filter(|(h, p)| !h.contains('/') && !p.starts_with('/'));
    if let (Some((h, p)), false) = (scp, url.contains("://")) {
        host = h.to_owned();
        path = p.to_owned();
    } else {
        let (scheme, rest) = url.split_once("://")?;
        if !matches!(scheme.to_lowercase().as_str(), "ssh" | "git" | "http" | "https" | "git+ssh") {
            return None;
        }
        let rest = rest.split_once('@').filter(|(u, _)| !u.contains('/')).map_or(rest, |(_, r)| r);
        let (authority, p) = rest.split_once('/')?;
        let (h, port) = authority.split_once(':').map_or((authority, None), |(h, p)| (h, Some(p)));
        host = h.to_owned();
        if scheme.starts_with("http")
            && let Some(port) = port
        {
            host = format!("{host}:{port}");
        }
        path = p.to_owned();
    }
    if let Some(p) = path.strip_suffix(".git/") {
        path = p.to_owned();
    } else if let Some(p) = path.strip_suffix(".git") {
        path = p.to_owned();
    }
    let path = path.trim_end_matches('/').to_owned();
    let lower = host.to_lowercase();
    if lower == "dev.azure.com" || lower.ends_with(".dev.azure.com") || lower.ends_with("visualstudio.com") {
        if let Some(rest) = path.strip_prefix("v3/") {
            let parts: Vec<&str> = rest.split('/').collect();
            if parts.len() >= 3 {
                return Some(format!("https://dev.azure.com/{}/{}/_git/{}", parts[0], parts[1], parts[2..].join("/")));
            }
        }
        return Some(format!("https://{}/{path}", host.trim_start_matches("ssh.")));
    }
    let host = host.strip_prefix("ssh.").unwrap_or(&host);
    if !path.contains('/') {
        return None;
    }
    Some(format!("https://{host}/{path}"))
}

fn enc(s: &str) -> String {
    s.split('/').map(|part| url::form_urlencoded::byte_serialize(part.as_bytes()).collect::<String>().replace('+', "%20")).collect::<Vec<_>>().join("/")
}

/// The page for a commit, branch or tag on the remote's site.
pub fn web_link(remote_url: &str, target: &Target) -> Option<String> {
    let base = web_base(remote_url)?;
    let b = base.to_lowercase();
    Some(match target {
        Target::Repo => base,
        _ if b.contains("dev.azure.com") || b.contains("visualstudio.com") => match target {
            Target::Commit(sha) => format!("{base}/commit/{sha}"),
            Target::Branch(n) => format!("{base}?version=GB{}", enc(n).replace('/', "%2F")),
            Target::Tag(n) => format!("{base}?version=GT{}", enc(n).replace('/', "%2F")),
            Target::Repo => unreachable!(),
        },
        _ if b.contains("gitlab") => match target {
            Target::Commit(sha) => format!("{base}/-/commit/{sha}"),
            Target::Branch(n) => format!("{base}/-/tree/{}", enc(n)),
            Target::Tag(n) => format!("{base}/-/tags/{}", enc(n)),
            Target::Repo => unreachable!(),
        },
        _ if b.contains("bitbucket") => match target {
            Target::Commit(sha) => format!("{base}/commits/{sha}"),
            Target::Branch(n) | Target::Tag(n) => format!("{base}/src/{}", enc(n)),
            Target::Repo => unreachable!(),
        },
        Target::Commit(sha) => format!("{base}/commit/{sha}"),
        Target::Branch(n) => format!("{base}/tree/{}", enc(n)),
        Target::Tag(n) => format!("{base}/releases/tag/{}", enc(n)),
    })
}

impl App {
    fn link_for(&mut self, tab: u64, remote: &str, target: &Target) -> Option<String> {
        let path = self.repo(tab)?.path.clone();
        let r = gitcraft_core::manage::remotes(std::path::Path::new(&path)).into_iter().find(|r| r.name == remote);
        let link = r.and_then(|r| web_link(if r.fetch_url.is_empty() { &r.push_url } else { &r.fetch_url }, target));
        if link.is_none() {
            self.toast(false, format!("{remote} isn't on a website GitCraft can link to."));
        }
        link
    }

    pub(crate) fn copy_web_link(&mut self, tab: u64, remote: &str, target: &Target) {
        if let Some(link) = self.link_for(tab, remote, target) {
            self.copy(&link);
        }
    }

    pub(crate) fn open_web_link(&mut self, tab: u64, remote: &str, target: &Target) {
        if let Some(link) = self.link_for(tab, remote, target) {
            self.open_url(&link);
        }
    }

    /// "Copy link …" entries: one remote gives "<label> on remote: <name>", several a submenu.
    pub(crate) fn link_items(&self, tab: u64, repo: &RepoView, label: &str, target: Target, only: Option<String>) -> Vec<Entry> {
        let remotes: Vec<String> = match only {
            Some(r) => vec![r],
            None => repo.data.remotes.clone(),
        };
        match remotes.len() {
            0 => Vec::new(),
            1 => {
                let r = remotes[0].clone();
                vec![item(format!("{label} on remote: {r}"), move |app| app.copy_web_link(tab, &r, &target))]
            }
            _ => vec![sub(
                label,
                remotes
                    .into_iter()
                    .map(|r| {
                        let t = target.clone();
                        item(format!("On remote: {r}"), move |app| app.copy_web_link(tab, &r, &t))
                    })
                    .collect(),
            )],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_links_for_common_remotes() {
        assert_eq!(web_base("git@github.com:owner/repo.git").as_deref(), Some("https://github.com/owner/repo"));
        assert_eq!(web_base("https://user@gitlab.example:8443/group/sub/repo.git").as_deref(), Some("https://gitlab.example:8443/group/sub/repo"));
        assert_eq!(web_base("git@ssh.dev.azure.com:v3/org/proj/repo").as_deref(), Some("https://dev.azure.com/org/proj/_git/repo"));
        assert_eq!(web_link("https://gitlab.com/a/b.git", &Target::Branch("feat/x".into())).as_deref(), Some("https://gitlab.com/a/b/-/tree/feat/x"));
        assert_eq!(web_link("git@github.com:a/b.git", &Target::Tag("v1".into())).as_deref(), Some("https://github.com/a/b/releases/tag/v1"));
        assert!(web_base("/local/path").is_none());
    }
}
