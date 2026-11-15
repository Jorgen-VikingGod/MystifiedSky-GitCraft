//! Create an empty hosted repository, connect it, and publish one reviewed branch.
use crate::{git, hosting, hosting_api::q};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

#[derive(Clone, Serialize, Deserialize)]
pub struct Target {
    pub id: String,
    pub label: String,
}

pub fn targets(service: &str) -> Result<Vec<Target>, String> {
    let get = |path: &str| {
        hosting::http(
            "GET",
            &format!("{}{path}", hosting::api_base(service)),
            &hosting::auth(service)?,
            None,
        )
    };
    let pairs: Vec<(String, String)> = match service {
        "github" => {
            let v = get("/user")?;
            let name = v["login"]
                .as_str()
                .ok_or("Missing GitHub account")?
                .to_string();
            vec![(name.clone(), name)]
        }
        "gitlab" => {
            let v = get("/user")?;
            let name = v["username"]
                .as_str()
                .ok_or("Missing GitLab account")?
                .to_string();
            vec![(name.clone(), name)]
        }
        "bitbucket" => {
            let url = format!("{}/workspaces?pagelen=100", hosting::api_base(service));
            let v = crate::pagination::collect(&url, |url| {
                hosting::http_page("GET", url, &hosting::auth(service)?, None)
            })?;
            v["values"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| Some((v["slug"].as_str()?.into(), v["name"].as_str()?.into())))
                .collect()
        }
        "azure" => {
            let url = format!(
                "{}/_apis/projects?api-version=7.1",
                hosting::api_base(service)
            );
            let v = crate::pagination::collect(&url, |url| {
                hosting::http_page("GET", url, &hosting::auth(service)?, None)
            })?;
            v["value"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| Some((v["id"].as_str()?.into(), v["name"].as_str()?.into())))
                .collect()
        }
        _ => return Err("Choose a supported hosting service.".into()),
    };
    Ok(pairs
        .into_iter()
        .map(|(id, label)| Target { id, label })
        .collect())
}

#[derive(Clone, Deserialize)]
pub struct PublishRequest {
    pub service: String,
    pub target: String,
    pub name: String,
    pub description: String,
    pub visibility: String,
    pub remote: String,
    pub branch: String,
    pub head: String,
}

#[derive(Serialize)]
pub struct PublishResult {
    pub url: String,
    pub connected: bool,
    pub pushed: bool,
    pub error: Option<String>,
}

fn create_request(r: &PublishRequest) -> Result<(String, Value), String> {
    if r.name.trim().is_empty()
        || r.name.starts_with('.')
        || !r
            .name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
    {
        return Err(
            "Repository names may contain letters, numbers, hyphens, underscores and periods."
                .into(),
        );
    }
    if !["private", "public"].contains(&r.visibility.as_str()) {
        return Err("Choose Public or Private access.".into());
    }
    let private = r.visibility == "private";
    Ok(match r.service.as_str() {
        "github" => ("/user/repos".into(),json!({"name":r.name,"description":r.description,"private":private,"auto_init":false})),
        "gitlab" => ("/projects".into(),json!({"name":r.name,"path":r.name,"description":r.description,"visibility":r.visibility,"initialize_with_readme":false})),
        "bitbucket" => (format!("/repositories/{}/{}",q(&r.target),q(&r.name)),json!({"scm":"git","name":r.name,"description":r.description,"is_private":private})),
        "azure" if r.visibility=="private" && r.description.is_empty() => (format!("/{}/_apis/git/repositories?api-version=7.1",q(&r.target)),json!({"name":r.name,"project":{"id":r.target}})),
        "azure" => return Err("Azure repository access is inherited from its project; a repository description is not supported.".into()),
        _ => return Err("Choose a supported hosting service.".into()),
    })
}

fn created_url(service: &str, v: &Value) -> Result<String, String> {
    let url=match service {
        "github" => v["clone_url"].as_str(),
        "gitlab" => v["http_url_to_repo"].as_str(),
        "bitbucket" => v["links"]["clone"].as_array().and_then(|links|links.iter().find(|v|v["name"]=="https")).and_then(|v|v["href"].as_str()),
        "azure" => v["remoteUrl"].as_str(),
        _ => None,
    }.ok_or("The service created the repository but returned no clone URL. Check your account before trying again.")?;
    let parsed = tauri::Url::parse(url).map_err(|_| {
        "The created repository returned an invalid URL. Check your account before trying again."
    })?;
    if parsed.scheme() != "https"
        || parsed.host_str() != Some(hosting::host_of(service).as_str())
        || parsed.password().is_some()
    {
        return Err("The created repository returned an unexpected URL. Check your account before trying again.".into());
    }
    Ok(url.into())
}

fn preflight(dir: &Path, r: &PublishRequest) -> Result<(), String> {
    let remote_ref = format!("refs/remotes/{}/{}", r.remote, r.branch);
    if r.remote.is_empty()
        || r.remote.starts_with('-')
        || r.remote.contains('/')
        || !git::run(dir, &["check-ref-format", &remote_ref]).ok
    {
        return Err("Enter a valid remote name.".into());
    }
    let branch = git::run(dir, &["symbolic-ref", "-q", "HEAD"]);
    let head = git::run(dir, &["rev-parse", "--verify", "HEAD"]);
    if !branch.ok
        || branch.stdout.trim() != format!("refs/heads/{}", r.branch)
        || !head.ok
        || head.stdout.trim() != r.head
    {
        return Err("The checked-out branch or commit changed. Close this panel and review the branch before publishing.".into());
    }
    Ok(())
}

pub fn publish(
    dir: &Path,
    r: &PublishRequest,
    jobs: &crate::jobs::Jobs,
) -> Result<PublishResult, String> {
    publish_with(dir, r, jobs, || {
        let (path, body) = create_request(r)?;
        if !targets(&r.service)?.iter().any(|t| t.id == r.target) {
            return Err(
                "The selected account, workspace or project is no longer available.".into(),
            );
        }
        let response=hosting::http("POST",&format!("{}{path}",hosting::api_base(&r.service)),&hosting::auth(&r.service)?,Some(body))
            .map_err(|error|format!("{error}\nCheck your account for the repository before retrying; a lost response can occur after creation."))?;
        created_url(&r.service, &response)
    })
}

fn publish_with(
    dir: &Path,
    r: &PublishRequest,
    jobs: &crate::jobs::Jobs,
    create: impl FnOnce() -> Result<String, String>,
) -> Result<PublishResult, String> {
    let lock = crate::operation::lock_for(dir);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    create_request(r)?;
    preflight(dir, r)?;
    let remotes = git::run(dir, &["remote"]);
    if !remotes.ok {
        return Err(remotes.message());
    }
    if remotes.stdout.lines().any(|name| name == r.remote) {
        return Err("That remote already exists. Choose another name or use the URL tab.".into());
    }
    let url = create()?;
    Ok(finish(dir, r, &url, jobs))
}

/// Resume only local connection and push; never repeat the repository creation request.
pub fn resume(
    dir: &Path,
    r: &PublishRequest,
    url: &str,
    jobs: &crate::jobs::Jobs,
) -> PublishResult {
    let lock = crate::operation::lock_for(dir);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    finish(dir, r, url, jobs)
}

fn finish(dir: &Path, r: &PublishRequest, url: &str, jobs: &crate::jobs::Jobs) -> PublishResult {
    let mut result = PublishResult {
        url: url.into(),
        connected: false,
        pushed: false,
        error: None,
    };
    let run = || -> Result<(), String> {
        preflight(dir, r)?;
        let remotes = git::run(dir, &["remote"]);
        if !remotes.ok {
            return Err(remotes.message());
        }
        if remotes.stdout.lines().any(|name| name == r.remote) {
            let current = git::run(dir, &["remote", "get-url", "--push", "--all", &r.remote]);
            let fetch = git::run(dir, &["remote", "get-url", "--all", &r.remote]);
            if !current.ok
                || !fetch.ok
                || current.stdout.trim() != url
                || fetch.stdout.trim() != url
            {
                return Err("The remote URL changed. Review it before pushing.".into());
            }
        } else {
            let out = git::run(dir, &["remote", "add", "--", &r.remote, url]);
            if !out.ok {
                return Err(out.message());
            }
        }
        Ok(())
    };
    if let Err(error) = run() {
        result.error = Some(error);
        return result;
    }
    result.connected = true;
    // Pin the object and destination even if another Git process changes branches during the request.
    let destination = format!("refs/heads/{}", r.branch);
    let spec = format!("{}:{destination}", r.head);
    let out = jobs.run(
        &format!("{}|push", dir.to_string_lossy()),
        dir,
        &["push", "--progress", "--no-follow-tags", "--", url, &spec],
    );
    if !out.ok {
        result.error = Some(out.message());
        return result;
    }
    result.pushed = true;
    for args in [
        vec![
            "update-ref".into(),
            format!("refs/remotes/{}/{}", r.remote, r.branch),
            r.head.clone(),
        ],
        vec![
            "config".into(),
            format!("branch.{}.remote", r.branch),
            r.remote.clone(),
        ],
        vec![
            "config".into(),
            format!("branch.{}.merge", r.branch),
            destination,
        ],
    ] {
        let out = git::run(dir, &args.iter().map(String::as_str).collect::<Vec<_>>());
        if !out.ok {
            result.error = Some(format!(
                "The branch was pushed, but its upstream could not be saved: {}",
                out.message()
            ));
            break;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, PublishRequest) {
        let d = tempfile::tempdir().unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec![
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "Initial",
            ],
        ] {
            assert!(git::run(d.path(), &args).ok);
        }
        let head = git::run(d.path(), &["rev-parse", "HEAD"])
            .stdout
            .trim()
            .to_string();
        (
            d,
            PublishRequest {
                service: "github".into(),
                target: "fixture".into(),
                name: "new-repo".into(),
                description: "A repository".into(),
                visibility: "private".into(),
                remote: "origin".into(),
                branch: "main".into(),
                head,
            },
        )
    }
    #[test]
    fn creates_empty_repositories_with_explicit_access_for_each_provider() {
        let (_, mut r) = fixture();
        let (path, body) = create_request(&r).unwrap();
        assert_eq!(path, "/user/repos");
        assert_eq!(body["private"], true);
        assert_eq!(body["auto_init"], false);
        r.service = "gitlab".into();
        r.visibility = "public".into();
        let (path, body) = create_request(&r).unwrap();
        assert_eq!(path, "/projects");
        assert_eq!(body["visibility"], "public");
        assert_eq!(body["initialize_with_readme"], false);
        r.service = "bitbucket".into();
        r.target = "workspace a".into();
        let (path, body) = create_request(&r).unwrap();
        assert_eq!(path, "/repositories/workspace%20a/new-repo");
        assert_eq!(body["is_private"], false);
        r.service = "azure".into();
        assert!(create_request(&r).is_err());
        r.visibility = "private".into();
        r.description.clear();
        let (path, body) = create_request(&r).unwrap();
        assert!(path.contains("workspace%20a/_apis/git/repositories"));
        assert_eq!(body["project"]["id"], "workspace a");
        r.name = "../bad".into();
        assert!(create_request(&r).is_err());
        assert!(created_url(
            "github",
            &json!({"clone_url":"https://unrelated.example/a.git"})
        )
        .is_err());
        assert!(created_url(
            "github",
            &json!({"clone_url":"https://user:secret@github.com/a/b.git"})
        )
        .is_err());
        assert_eq!(
            created_url(
                "gitlab",
                &json!({"http_url_to_repo":"https://gitlab.com/a/b.git"})
            )
            .unwrap(),
            "https://gitlab.com/a/b.git"
        );
        assert_eq!(created_url("bitbucket",&json!({"links":{"clone":[{"name":"ssh","href":"ssh://example"},{"name":"https","href":"https://bitbucket.org/a/b.git"}]}})).unwrap(),"https://bitbucket.org/a/b.git");
    }
    #[test]
    fn local_preflight_refuses_stale_branch_and_existing_remote_before_creation() {
        let (d, mut r) = fixture();
        let jobs = crate::jobs::Jobs::default();
        r.head = "0".repeat(40);
        assert!(publish_with(d.path(), &r, &jobs, || panic!("must not create")).is_err());
        r.head = git::run(d.path(), &["rev-parse", "HEAD"])
            .stdout
            .trim()
            .into();
        assert!(
            git::run(
                d.path(),
                &["remote", "add", "origin", "https://fixture.example/a.git"]
            )
            .ok
        );
        assert!(publish_with(d.path(), &r, &jobs, || panic!("must not create")).is_err());
        assert_eq!(
            git::run(d.path(), &["remote", "get-url", "origin"])
                .stdout
                .trim(),
            "https://fixture.example/a.git"
        );
    }
    #[test]
    fn publishes_exact_branch_and_saves_upstream_without_pushing_tags() {
        let (d, r) = fixture();
        let bare = tempfile::tempdir().unwrap();
        assert!(git::run(bare.path(), &["init", "--bare", "-q"]).ok);
        assert!(git::run(d.path(), &["tag", "main"]).ok);
        assert!(
            git::run(
                d.path(),
                &[
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "tag",
                    "-a",
                    "release",
                    "-m",
                    "Release"
                ]
            )
            .ok
        );
        assert!(git::run(d.path(), &["config", "push.followTags", "true"]).ok);
        let result = publish_with(d.path(), &r, &crate::jobs::Jobs::default(), || {
            Ok(bare.path().to_string_lossy().into())
        })
        .unwrap();
        assert!(result.connected && result.pushed, "{:?}", result.error);
        assert!(result.error.is_none());
        assert_eq!(
            git::run(bare.path(), &["rev-parse", "refs/heads/main"])
                .stdout
                .trim(),
            r.head
        );
        assert!(!git::run(bare.path(), &["show-ref", "--verify", "refs/tags/main"]).ok);
        assert!(!git::run(bare.path(), &["show-ref", "--verify", "refs/tags/release"]).ok);
        assert_eq!(
            git::run(d.path(), &["rev-parse", "--abbrev-ref", "@{u}"])
                .stdout
                .trim(),
            "origin/main"
        );
    }
    #[test]
    fn failed_push_keeps_remote_and_resumes_without_creating_again() {
        let (d, r) = fixture();
        let parent = tempfile::tempdir().unwrap();
        let bare = parent.path().join("new.git");
        let url = bare.to_string_lossy().to_string();
        let jobs = crate::jobs::Jobs::default();
        let first = publish_with(d.path(), &r, &jobs, || Ok(url.clone())).unwrap();
        assert!(first.connected && !first.pushed && first.error.is_some());
        assert!(git::run(parent.path(), &["init", "--bare", "-q", "new.git"]).ok);
        let result = resume(d.path(), &r, &url, &jobs);
        assert!(result.pushed && result.error.is_none());
        assert!(
            git::run(
                d.path(),
                &[
                    "remote",
                    "set-url",
                    "origin",
                    "https://fixture.example/other.git"
                ]
            )
            .ok
        );
        let refused = resume(d.path(), &r, &url, &jobs);
        assert!(!refused.pushed);
        assert!(refused.error.unwrap().contains("URL changed"));
    }
    #[test]
    fn creation_failure_leaves_local_configuration_untouched() {
        let (d, r) = fixture();
        let jobs = crate::jobs::Jobs::default();
        assert!(
            publish_with(d.path(), &r, &jobs, || Err("403 missing permission".into())).is_err()
        );
        assert!(git::run(d.path(), &["remote"]).stdout.trim().is_empty());
        let partial = publish_with(d.path(), &r, &jobs, || {
            assert!(git::run(d.path(), &["checkout", "-qb", "other"]).ok);
            Ok("https://github.com/fixture/new-repo.git".into())
        })
        .unwrap();
        assert!(!partial.connected && !partial.pushed);
        assert!(partial.error.unwrap().contains("branch or commit changed"));
        assert!(git::run(d.path(), &["remote"]).stdout.trim().is_empty());
    }
}
