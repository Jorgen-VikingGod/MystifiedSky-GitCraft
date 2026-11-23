//! Pull requests, issues, comments, Launchpad, repositories to clone and SSH keys for every integration
//! (see hosting.rs for connections, sign-in and remotes).

use crate::hosting::{api_base, auth, config, host_of, http, label, secret_get, secret_name, Auth, RemoteRepo};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Serialize, Clone, Debug, Default)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub source: String,
    pub source_url: String,
    pub source_sha: String,
    pub target: String,
    pub draft: bool,
    pub state: String,
    pub url: String,
    pub updated: String,
    pub body: String,
    /// "owner/repo" - set on Launchpad results that span repositories.
    pub repo: String,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub state: String,
    pub url: String,
    pub labels: Vec<String>,
    pub updated: String,
    pub body: String,
    pub repo: String,
    /// Tracker key when it isn't just a number (Jira "ABC-123", Trello "#12").
    pub key: String,
    /// Which tracker it came from (github, gitlab, jira, trello, azure, bitbucket).
    pub tracker: String,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Comment {
    pub id: String,
    pub author: String,
    pub body: String,
    pub created: String,
}

fn get(id: &str, path: &str) -> Result<Value, String> {
    crate::pagination::collect(&format!("{}{path}", api_base(id)), |url| crate::hosting::http_page("GET", url, &auth(id)?, None))
}

fn send(id: &str, method: &str, path: &str, body: Value) -> Result<Value, String> {
    http(method, &format!("{}{path}", api_base(id)), &auth(id)?, Some(body))
}

fn s(v: &Value) -> String {
    v.as_str().map(|x| x.to_string()).unwrap_or_else(|| if v.is_number() { v.to_string() } else { String::new() })
}

fn list(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}

fn ms_to_iso(v: &Value) -> String {
    v.as_i64().and_then(chrono::DateTime::from_timestamp_millis).map(|d| d.to_rfc3339()).unwrap_or_default()
}

fn enc(path: &str) -> String {
    path.replace('/', "%2F")
}

pub(crate) fn q(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub(crate) fn github_pr(v: &Value) -> PullRequest {
    PullRequest {
        number: v["number"].as_u64().unwrap_or(0),
        title: s(&v["title"]),
        author: s(&v["user"]["login"]),
        source: s(&v["head"]["ref"]),
        source_url: s(&v["head"]["repo"]["clone_url"]), source_sha: s(&v["head"]["sha"]),
        target: s(&v["base"]["ref"]),
        draft: v["draft"].as_bool().unwrap_or(false),
        state: if v["merged_at"].is_string() { "merged".into() } else { s(&v["state"]) },
        url: s(&v["html_url"]),
        updated: s(&v["updated_at"]),
        body: s(&v["body"]),
        repo: s(&v["base"]["repo"]["full_name"]),
    }
}

pub(crate) fn gitlab_mr(v: &Value) -> PullRequest {
    PullRequest {
        number: v["iid"].as_u64().unwrap_or(0),
        title: s(&v["title"]),
        author: s(&v["author"]["username"]),
        source: s(&v["source_branch"]),
        source_url: String::new(), source_sha: v["sha"].as_str().or(v["diff_refs"]["head_sha"].as_str()).unwrap_or_default().into(),
        target: s(&v["target_branch"]),
        draft: v["draft"].as_bool().unwrap_or(false) || v["work_in_progress"].as_bool().unwrap_or(false),
        state: s(&v["state"]),
        url: s(&v["web_url"]),
        updated: s(&v["updated_at"]),
        body: s(&v["description"]),
        repo: s(&v["references"]["full"]).split('!').next().unwrap_or_default().to_string(),
    }
}

pub(crate) fn bitbucket_pr(v: &Value) -> PullRequest {
    PullRequest {
        number: v["id"].as_u64().unwrap_or(0),
        title: s(&v["title"]),
        author: s(&v["author"]["display_name"]),
        source: s(&v["source"]["branch"]["name"]),
        source_url: clone_link(&v["source"]["repository"]["links"]["clone"], "https"), source_sha: s(&v["source"]["commit"]["hash"]),
        target: s(&v["destination"]["branch"]["name"]),
        draft: v["draft"].as_bool().unwrap_or(false),
        state: s(&v["state"]).to_lowercase(),
        url: s(&v["links"]["html"]["href"]),
        updated: s(&v["updated_on"]),
        body: s(&v["description"]),
        repo: s(&v["destination"]["repository"]["full_name"]),
    }
}

fn bitbucket_server_pr(v: &Value) -> PullRequest {
    PullRequest {
        number: v["id"].as_u64().unwrap_or(0),
        title: s(&v["title"]),
        author: s(&v["author"]["user"]["displayName"]),
        source: s(&v["fromRef"]["displayId"]),
        source_url: clone_link(&v["fromRef"]["repository"]["links"]["clone"], "http"), source_sha: s(&v["fromRef"]["latestCommit"]),
        target: s(&v["toRef"]["displayId"]),
        draft: v["draft"].as_bool().unwrap_or(false),
        state: s(&v["state"]).to_lowercase(),
        url: s(&v["links"]["self"][0]["href"]),
        updated: ms_to_iso(&v["updatedDate"]),
        body: s(&v["description"]),
        repo: format!("{}/{}", s(&v["toRef"]["repository"]["project"]["key"]), s(&v["toRef"]["repository"]["slug"])),
    }
}

pub(crate) fn azure_pr(v: &Value, web: &str) -> PullRequest {
    let id = v["pullRequestId"].as_u64().unwrap_or(0);
    PullRequest {
        number: id,
        title: s(&v["title"]),
        author: s(&v["createdBy"]["displayName"]),
        source: s(&v["sourceRefName"]).trim_start_matches("refs/heads/").to_string(),
        source_url: v["forkSource"]["repository"]["remoteUrl"].as_str().or(v["repository"]["remoteUrl"].as_str()).unwrap_or_default().into(), source_sha: s(&v["lastMergeSourceCommit"]["commitId"]),
        target: s(&v["targetRefName"]).trim_start_matches("refs/heads/").to_string(),
        draft: v["isDraft"].as_bool().unwrap_or(false),
        state: s(&v["status"]),
        url: if web.is_empty() { String::new() } else { format!("{web}/pullrequest/{id}") },
        updated: s(&v["creationDate"]),
        body: s(&v["description"]),
        repo: s(&v["repository"]["name"]),
    }
}

pub(crate) fn github_issue(v: &Value) -> Issue {
    Issue {
        number: v["number"].as_u64().unwrap_or(0),
        title: s(&v["title"]),
        author: s(&v["user"]["login"]),
        state: s(&v["state"]),
        url: s(&v["html_url"]),
        labels: list(&v["labels"]).iter().map(|l| s(&l["name"])).collect(),
        updated: s(&v["updated_at"]),
        body: s(&v["body"]),
        repo: s(&v["repository_url"]).rsplitn(3, '/').take(2).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("/"),
        tracker: "github".into(),
        ..Default::default()
    }
}

fn gitlab_issue(v: &Value) -> Issue {
    Issue {
        number: v["iid"].as_u64().unwrap_or(0),
        title: s(&v["title"]),
        author: s(&v["author"]["username"]),
        state: s(&v["state"]),
        url: s(&v["web_url"]),
        labels: list(&v["labels"]).iter().map(s).collect(),
        updated: s(&v["updated_at"]),
        body: s(&v["description"]),
        repo: s(&v["references"]["full"]).split('#').next().unwrap_or_default().to_string(),
        tracker: "gitlab".into(),
        ..Default::default()
    }
}

/// Jira Cloud (API 3) descriptions are Atlassian documents; keep their plain text.
fn adf_text(v: &Value, out: &mut String) {
    if let Some(t) = v["text"].as_str() {
        out.push_str(t);
    }
    for c in list(&v["content"]) {
        adf_text(&c, out);
    }
    if v["type"] == "paragraph" {
        out.push('\n');
    }
}

pub(crate) fn jira_issue(v: &Value, id: &str) -> Issue {
    let key = s(&v["key"]);
    let f = &v["fields"];
    let mut body = String::new();
    if f["description"].is_string() {
        body = s(&f["description"]);
    } else {
        adf_text(&f["description"], &mut body);
    }
    Issue {
        number: key.rsplit('-').next().and_then(|n| n.parse().ok()).unwrap_or(0),
        title: s(&f["summary"]),
        author: s(&f["reporter"]["displayName"]),
        state: s(&f["status"]["name"]),
        url: format!("https://{}/browse/{key}", host_of(id)),
        labels: list(&f["labels"]).iter().map(s).collect(),
        updated: s(&f["updated"]),
        body: body.trim().to_string(),
        repo: s(&f["project"]["key"]),
        key,
        tracker: id.into(),
    }
}

pub(crate) fn trello_issue(v: &Value) -> Issue {
    Issue {
        number: v["idShort"].as_u64().unwrap_or(0),
        title: s(&v["name"]),
        state: if v["closed"].as_bool().unwrap_or(false) { "closed".into() } else { "open".into() },
        url: s(&v["url"]),
        labels: list(&v["labels"]).iter().map(|l| s(&l["name"])).filter(|n| !n.is_empty()).collect(),
        updated: s(&v["dateLastActivity"]),
        body: s(&v["desc"]),
        key: format!("#{}", v["idShort"].as_u64().unwrap_or(0)),
        tracker: "trello".into(),
        ..Default::default()
    }
}

fn azure_parts(r: &RemoteRepo) -> (String, String, String) {
    let p: Vec<&str> = r.path.split('/').collect();
    (p.first().unwrap_or(&"").to_string(), p.get(1).unwrap_or(&"").to_string(), p.get(2).unwrap_or(&"").to_string())
}

/// Azure DevOps calls go to the remote's own organisation.
fn azure_project(r: &RemoteRepo) -> (String, String) {
    if r.service == "azure-server" {
        if let Some((collection_project, repo)) = r.web.rsplit_once("/_git/") {
            return (collection_project.to_owned(), repo.to_owned());
        }
    }
    let (org, project, repo) = azure_parts(r);
    (format!("https://dev.azure.com/{org}/{project}"), repo)
}

fn azure_url(r: &RemoteRepo, rest: &str) -> String {
    let (project, repo) = azure_project(r);
    let rest = if r.service == "azure-server" { rest.replace("api-version=7.1", "api-version=7.0") } else { rest.to_owned() };
    format!("{project}/_apis/git/repositories/{repo}{rest}")
}

fn forge_pr(value: &Value) -> PullRequest {
    let mut pr = github_pr(value);
    pr.source_sha = value["head"]["sha"].as_str().or(value["head"]["commit_id"].as_str()).unwrap_or_default().to_owned();
    pr.draft |= pr.title.starts_with("WIP:") || pr.title.starts_with("Draft:");
    if value["merged"] == true { pr.state = "merged".into(); }
    pr
}

fn forge_issue(value: &Value, service: &str) -> Issue {
    let mut issue = github_issue(value);
    issue.tracker = service.to_owned();
    issue
}

fn bbs_path(r: &RemoteRepo) -> String {
    let (p, repo) = r.path.split_once('/').unwrap_or((&r.path, ""));
    format!("/projects/{p}/repos/{repo}")
}

fn rget(r: &RemoteRepo, url: &str) -> Result<Value, String> {
    crate::pagination::collect(url, |next| crate::hosting::http_page("GET", next, &auth(&r.service)?, None))
}

fn rsend(r: &RemoteRepo, method: &str, url: &str, body: Value) -> Result<Value, String> {
    http(method, url, &auth(&r.service)?, Some(body))
}

#[derive(Clone, Copy, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum PullFilter {
    #[default]
    Open,
    Closed,
    Merged,
    Mine,
    ReviewRequested,
}

fn pull_identity(r: &RemoteRepo, base: &str) -> Result<String, String> {
    let get = |url: &str| http("GET", url, &auth(&r.service)?, None);
    let id = match r.service.as_str() {
        "github" | "github-enterprise" | "gitea" | "forgejo" | "codeberg" => s(&get(&format!("{base}/user"))?["login"]),
        "gitlab" | "gitlab-self" => s(&get(&format!("{base}/user"))?["username"]),
        "bitbucket" => s(&get(&format!("{base}/user"))?["uuid"]),
        "bitbucket-server" => crate::hosting::config().bitbucket_server_user.trim().into(),
        "azure-server" => s(&get(&format!("{base}/_apis/connectionData"))?["authenticatedUser"]["id"]),
        "azure" => {
            let (org, _, _) = azure_parts(r);
            s(&get(&format!("https://dev.azure.com/{org}/_apis/connectionData"))?["authenticatedUser"]["id"])
        }
        _ => String::new(),
    };
    if id.is_empty() { Err("Connect a named account to filter by author or reviewer.".into()) } else { Ok(id) }
}

fn pull_person_matches(service: &str, row: &Value, filter: PullFilter, identity: &str) -> bool {
    let same = |value: &Value| value.as_str().is_some_and(|value| value.eq_ignore_ascii_case(identity));
    match (service, filter) {
        ("github" | "github-enterprise" | "gitea" | "forgejo" | "codeberg", PullFilter::Mine) => same(&row["user"]["login"]),
        ("github" | "github-enterprise" | "gitea" | "forgejo" | "codeberg", PullFilter::ReviewRequested) => list(&row["requested_reviewers"]).iter().any(|user| same(&user["login"])),
        ("gitlab" | "gitlab-self", PullFilter::Mine) => same(&row["author"]["username"]),
        ("gitlab" | "gitlab-self", PullFilter::ReviewRequested) => list(&row["reviewers"]).iter().any(|user| same(&user["username"])),
        ("bitbucket", PullFilter::Mine) => same(&row["author"]["uuid"]),
        ("bitbucket", PullFilter::ReviewRequested) => list(&row["reviewers"]).iter().any(|user| same(&user["uuid"])),
        ("bitbucket-server", PullFilter::Mine) => same(&row["author"]["user"]["name"]) || same(&row["author"]["user"]["slug"]),
        ("bitbucket-server", PullFilter::ReviewRequested) => list(&row["reviewers"]).iter().any(|user| same(&user["user"]["name"]) || same(&user["user"]["slug"])),
        ("azure" | "azure-server", PullFilter::Mine) => same(&row["createdBy"]["id"]),
        ("azure" | "azure-server", PullFilter::ReviewRequested) => list(&row["reviewers"]).iter().any(|user| same(&user["id"]) && user["vote"].as_i64().unwrap_or(0) == 0),
        _ => true,
    }
}

fn pull_requests_filtered(r: &RemoteRepo, filter: PullFilter) -> Result<Vec<PullRequest>, String> {
    let base = api_base(&r.service);
    let identity = if matches!(filter, PullFilter::Mine | PullFilter::ReviewRequested) {
        Some(pull_identity(r, &base)?)
    } else { None };
    let gh_closed = matches!(filter, PullFilter::Closed | PullFilter::Merged);
    let (url, key, decode): (_, _, fn(&Value, &RemoteRepo) -> PullRequest) = match r.service.as_str() {
        "github" | "github-enterprise" => (format!("{base}/repos/{}/pulls?state={}&per_page=50", r.path, if gh_closed { "closed" } else { "open" }), None, |v, _| github_pr(v)),
        "gitea" | "forgejo" | "codeberg" => (format!("{base}/repos/{}/pulls?state={}&limit=50", r.path, if gh_closed { "closed" } else { "open" }), None, |v, _| forge_pr(v)),
        "gitlab" | "gitlab-self" => {
            let state = match filter { PullFilter::Closed => "closed", PullFilter::Merged => "merged", _ => "opened" };
            (format!("{base}/projects/{}/merge_requests?state={state}&per_page=50", enc(&r.path)), None, |v, _| gitlab_mr(v))
        }
        "bitbucket" => {
            let state = match filter { PullFilter::Closed => "DECLINED&state=SUPERSEDED", PullFilter::Merged => "MERGED", _ => "OPEN" };
            let fields = if filter == PullFilter::ReviewRequested { "&fields=%2Bvalues.reviewers" } else { "" };
            (format!("{base}/repositories/{}/pullrequests?state={state}&pagelen=50{fields}", r.path), Some("values"), |v, _| bitbucket_pr(v))
        }
        "bitbucket-server" => {
            let state = match filter { PullFilter::Closed => "DECLINED", PullFilter::Merged => "MERGED", _ => "OPEN" };
            (format!("{base}{}/pull-requests?state={state}&limit=50", bbs_path(r)), Some("values"), |v, _| bitbucket_server_pr(v))
        }
        "azure" | "azure-server" => {
            let state = match filter { PullFilter::Closed => "abandoned", PullFilter::Merged => "completed", _ => "active" };
            (azure_url(r, &format!("/pullrequests?searchCriteria.status={state}&api-version=7.1&$top=50")), Some("value"), |v, r| azure_pr(v, &r.web))
        }
        _ => return Ok(vec![]),
    };
    let data = rget(r, &url)?;
    let mut items = list(if let Some(key) = key { &data[key] } else { &data }).iter()
        .filter(|row| identity.as_ref().is_none_or(|id| pull_person_matches(&r.service, row, filter, id)))
        .map(|row| decode(row, r)).collect::<Vec<_>>();
    if gh_closed && matches!(r.service.as_str(), "github" | "github-enterprise" | "gitea" | "forgejo" | "codeberg") {
        items.retain(|row| (row.state == "merged") == (filter == PullFilter::Merged));
    }
    Ok(items)
}

pub fn pull_requests(r: &RemoteRepo) -> Result<Vec<PullRequest>, String> {
    pull_requests_filtered(r, PullFilter::Open)
}

pub fn pull_requests_page(r: &RemoteRepo, cursor: Option<&str>, filter: PullFilter) -> Result<crate::pagination::Page<Vec<PullRequest>>, String> {
    crate::pagination::one_page(cursor.map(str::to_owned), || pull_requests_filtered(r, filter))
}

#[derive(Serialize)]
pub struct CommitCheck {
    name: String,
    state: &'static str,
    url: String,
}

fn check_state(value: &str) -> &'static str {
    match value.to_ascii_lowercase().as_str() {
        "success" | "succeeded" | "successful" | "passed" => "passed",
        "failed" | "failure" | "error" | "timed_out" | "stale" | "action_required" | "startup_failure" => "failed",
        "pending" | "created" | "waiting_for_resource" | "queued" | "in_progress" | "running" | "inprogress" | "requested" | "waiting" | "scheduled" => "pending",
        "cancelled" | "canceled" | "stopped" => "cancelled",
        _ => "neutral",
    }
}

pub fn commit_checks(r: &RemoteRepo, sha: &str, number: Option<u64>) -> Result<Vec<CommitCheck>, String> {
    if !matches!(sha.len(), 40 | 64) || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Choose a commit with a full Git hash.".into());
    }
    let base = api_base(&r.service);
    let mut checks = Vec::new();
    let mut add = |name: String, state: String, url: String| {
        if name.is_empty() { return; }
        checks.push(CommitCheck { name, state: check_state(&state), url });
    };
    match r.service.as_str() {
        "github" | "github-enterprise" => {
            let commit = format!("{base}/repos/{}/commits/{sha}", r.path);
            let runs = rget(r, &format!("{commit}/check-runs?filter=latest&per_page=100"));
            let statuses = rget(r, &format!("{commit}/statuses?per_page=100"));
            if let (Err(error), Err(_)) = (&runs, &statuses) { return Err(error.clone()); }
            if let Ok(runs) = runs {
                for row in list(&runs["check_runs"]) {
                    let state = if row["status"] == "completed" { s(&row["conclusion"]) } else { s(&row["status"]) };
                    add(s(&row["name"]), state, s(&row["details_url"]));
                }
            }
            if let Ok(statuses) = statuses {
                for row in list(&statuses) { add(s(&row["context"]), s(&row["state"]), s(&row["target_url"])); }
            }
        }
        "gitea" | "forgejo" | "codeberg" => {
            for row in list(&rget(r, &format!("{base}/repos/{}/statuses/{sha}?limit=50", r.path))?) {
                add(s(&row["context"]), s(&row["status"]), s(&row["target_url"]));
            }
        }
        "gitlab" | "gitlab-self" => {
            let project = format!("{base}/projects/{}", enc(&r.path));
            let url = match number {
                Some(number) => format!("{project}/merge_requests/{number}/pipelines?per_page=100"),
                None => format!("{project}/repository/commits/{sha}/statuses?per_page=100"),
            };
            for row in list(&rget(r, &url)?) {
                if number.is_some() && row["sha"] != sha { continue; }
                if number.is_some() { add(format!("Pipeline #{}", s(&row["id"])), s(&row["status"]), s(&row["web_url"])); }
                else { add(s(&row["name"]), s(&row["status"]), s(&row["target_url"])); }
            }
        }
        "bitbucket" => {
            let resource = match number {
                Some(number) => format!("pullrequests/{number}/statuses"),
                None => format!("commit/{sha}/statuses"),
            };
            for row in list(&rget(r, &format!("{base}/repositories/{}/{resource}?pagelen=50", r.path))?["values"]) {
                add(s(&row["name"]), s(&row["state"]), s(&row["url"]));
            }
        }
        "bitbucket-server" => {
            let host = host_of(&r.service);
            for row in list(&rget(r, &format!("https://{host}/rest/build-status/1.0/commits/{sha}?limit=50"))?["values"]) {
                add(s(&row["name"]), s(&row["state"]), s(&row["url"]));
            }
        }
        "azure" | "azure-server" => {
            let resource = match number {
                Some(number) => format!("/pullRequests/{number}/statuses?api-version=7.1"),
                None => format!("/commits/{sha}/statuses?latestOnly=true&api-version=7.1"),
            };
            for row in list(&rget(r, &azure_url(r, &resource))?["value"]) {
                add(s(&row["context"]["name"]), s(&row["state"]), s(&row["targetUrl"]));
            }
        }
        _ => return Err("This hosting service does not publish commit checks through GitCraft.".into()),
    }
    let mut seen = std::collections::HashSet::new();
    checks.retain(|check| seen.insert((check.name.clone(), check.url.clone())));
    Ok(checks)
}

pub fn pull_request(r: &RemoteRepo, number: u64) -> Result<PullRequest, String> {
    let base = api_base(&r.service);
    Ok(match r.service.as_str() {
        "github" | "github-enterprise" => github_pr(&rget(r, &format!("{base}/repos/{}/pulls/{number}", r.path))?),
        "gitea" | "forgejo" | "codeberg" => forge_pr(&rget(r, &format!("{base}/repos/{}/pulls/{number}", r.path))?),
        "gitlab" | "gitlab-self" => gitlab_mr(&rget(r, &format!("{base}/projects/{}/merge_requests/{number}", enc(&r.path)))?),
        "bitbucket" => bitbucket_pr(&rget(r, &format!("{base}/repositories/{}/pullrequests/{number}", r.path))?),
        "bitbucket-server" => bitbucket_server_pr(&rget(r, &format!("{base}{}/pull-requests/{number}", bbs_path(r)))?),
        "azure" | "azure-server" => azure_pr(&rget(r, &azure_url(r, &format!("/pullrequests/{number}?api-version=7.1")))?, &r.web),
        _ => return Err("This hosting service does not support pull-request checkout.".into()),
    })
}

#[derive(Serialize)]
pub struct PullRequestDetails {
    #[serde(flatten)]
    request: PullRequest,
    merge_methods: Vec<&'static str>,
    merge_status: Option<&'static str>,
}

pub fn pull_request_details(r: &RemoteRepo, number: u64) -> Result<PullRequestDetails, String> {
    let base = api_base(&r.service);
    let (request, methods, status) = match r.service.as_str() {
        "github" | "github-enterprise" => {
            let repo = format!("{base}/repos/{}", r.path);
            let detail = rget(r, &format!("{repo}/pulls/{number}"))?;
            let policy = rget(r, &repo).unwrap_or_default();
            let methods = [("merge", "allow_merge_commit"), ("squash", "allow_squash_merge"), ("rebase", "allow_rebase_merge")]
                .into_iter().filter_map(|(method, field)| (policy[field].as_bool() == Some(true)).then_some(method)).collect();
            let status = match (detail["mergeable"].as_bool(), detail["mergeable_state"].as_str()) {
                (Some(false), _) | (_, Some("dirty")) => Some("Resolve merge conflicts on the hosting service."),
                (None, _) => Some("The hosting service is checking mergeability. Refresh to check again."),
                (_, Some("blocked")) => Some("The hosting service is waiting for required reviews or checks."),
                _ => None,
            };
            (github_pr(&detail), methods, status)
        }
        "gitea" | "forgejo" | "codeberg" => {
            let repo = format!("{base}/repos/{}", r.path);
            let detail = rget(r, &format!("{repo}/pulls/{number}"))?;
            let policy = rget(r, &repo).unwrap_or_default();
            let methods = [("merge", "allow_merge_commits"), ("squash", "allow_squash_merge"), ("rebase", "allow_rebase"),
                ("rebase_merge", "allow_rebase_explicit"), ("fast_forward", "allow_fast_forward_only_merge")]
                .into_iter().filter_map(|(method, flag)| (policy[flag] == true).then_some(method)).collect();
            let status = (detail["mergeable"] == false).then_some("The hosting service reports that this pull request cannot be merged yet.");
            (forge_pr(&detail), methods, status)
        }
        "gitlab" | "gitlab-self" => {
            let project = format!("{base}/projects/{}", enc(&r.path));
            let detail = rget(r, &format!("{project}/merge_requests/{number}"))?;
            let policy = rget(r, &project).unwrap_or_default();
            let methods = match policy["squash_option"].as_str() {
                Some("always") => vec!["squash"],
                Some("never") => vec!["default"],
                Some("default_on") => vec!["squash", "unsquashed"],
                Some("default_off") => vec!["unsquashed", "squash"],
                _ => vec!["default"],
            };
            let status = match detail["detailed_merge_status"].as_str() {
                _ if detail["has_conflicts"] == true => Some("Resolve merge conflicts on the hosting service."),
                Some("not_approved" | "requested_changes") => Some("The merge request needs approval."),
                Some("need_rebase") => Some("Rebase the source branch before merging."),
                Some("checking" | "unchecked" | "preparing" | "approvals_syncing") => Some("The hosting service is checking mergeability. Refresh to check again."),
                Some("mergeable" | "draft_status") | None => None,
                Some(_) => Some("The hosting service reports an unmet merge requirement. Review it in the browser."),
            };
            (gitlab_mr(&detail), methods, status)
        }
        "bitbucket" => {
            let url = format!("{base}/repositories/{}/pullrequests/{number}", r.path);
            let detail = rget(r, &url)?;
            let branch = &detail["destination"]["branch"];
            let strategies = branch["merge_strategies"].as_array();
            let mut methods: Vec<_> = [("merge", "merge_commit"), ("squash", "squash"), ("fast_forward", "fast_forward")]
                .into_iter().filter_map(|(method, value)| strategies.is_some_and(|items| items.iter().any(|item| item == value)).then_some(method)).collect();
            if let Some(position) = methods.iter().position(|method| {
                let strategy = if *method == "merge" { "merge_commit" } else { *method };
                branch["default_merge_strategy"] == strategy
            }) { methods.swap(0, position); }
            if methods.is_empty() { methods.push("default"); }
            let checks = rget(r, &format!("{url}/mergeability/checks")).unwrap_or_default();
            let status = list(&checks["values"]).iter().any(|check| check["blocking"] == true && check["status"] != "PASSED")
                .then_some("The hosting service reports a blocking merge check. Review it in the browser.");
            (bitbucket_pr(&detail), methods, status)
        }
        "bitbucket-server" => {
            let url = format!("{base}{}/pull-requests/{number}", bbs_path(r));
            let detail = rget(r, &url)?;
            let check = rget(r, &format!("{url}/merge")).unwrap_or_default();
            let status = if check["conflicted"] == true { Some("Resolve merge conflicts on the hosting service.") }
                else if check["canMerge"] == false { Some("The hosting service reports an unmet merge requirement. Review it in the browser.") }
                else { None };
            (bitbucket_server_pr(&detail), vec!["default"], status)
        }
        "azure" | "azure-server" => {
            let detail = rget(r, &azure_url(r, &format!("/pullrequests/{number}?api-version=7.1")))?;
            let (project, _) = azure_project(r);
            let endpoint = format!("{project}/_apis");
            let version = if r.service == "azure-server" { "7.0" } else { "7.1" };
            let mut methods = Vec::new();
            if let (Some(repo), Some(reference)) = (
                detail["repository"]["id"].as_str(), detail["targetRefName"].as_str()
            ) {
                let url = format!("{endpoint}/git/policy/configurations?repositoryId={}&refName={}&api-version={version}", q(repo), q(reference));
                if let Ok(policies) = crate::pagination::collect(&url, |url| crate::hosting::http_page("GET", url, &auth(&r.service)?, None)) {
                    let policies = list(&policies["value"]);
                    methods = [("merge", "allowNoFastForward"), ("squash", "allowSquash"), ("rebase", "allowRebase"), ("rebase_merge", "allowRebaseMerge")].into_iter()
                        .filter_map(|(method, field)| policies.iter().filter(|p| p["isEnabled"] == true && p["isBlocking"] == true && p["type"]["id"] == "fa4e907d-c16b-4a4c-9dfa-4916e5d171ab")
                            .all(|p| p["settings"][field].as_bool() == Some(true)).then_some(method)).collect();
                }
            }
            let status = match detail["mergeStatus"].as_str() {
                Some("conflicts") => Some("Resolve merge conflicts on the hosting service."),
                Some("queued") => Some("The hosting service is checking mergeability. Refresh to check again."),
                Some("rejectedByPolicy") => Some("The hosting service is waiting for required reviews or checks."),
                _ => None,
            };
            let status = status.or_else(|| {
                let project_id = detail["repository"]["project"]["id"].as_str()?;
                let artifact = q(&format!("vstfs:///CodeReview/CodeReviewId/{project_id}/{number}"));
                let checks = rget(r, &format!("{endpoint}/policy/evaluations?artifactId={artifact}&api-version={version}-preview.1")).ok()?;
                list(&checks["value"]).iter().any(|check| {
                    check["configuration"]["isEnabled"] == true && check["configuration"]["isBlocking"] == true
                        && matches!(check["status"].as_str(), Some("queued" | "running" | "rejected" | "broken"))
                }).then_some("The hosting service is waiting for required reviews or checks.")
            });
            (azure_pr(&detail, &r.web), methods, status)
        }
        _ => return Err("This hosting service does not support pull requests.".into()),
    };
    crate::read_jobs::check()?;
    let merge_status = if request.draft { Some("Draft pull requests cannot be merged.") }
        else if !matches!(request.state.to_ascii_lowercase().as_str(), "open" | "opened" | "active") { Some("This pull request is closed.") }
        else { status };
    Ok(PullRequestDetails { request, merge_methods: methods, merge_status })
}

#[derive(Serialize)]
pub struct PrCheckout { pub url: String, pub reference: String, pub expected: String }

pub fn pr_checkout(r: &RemoteRepo, pr: &PullRequest) -> Result<PrCheckout, String> {
    let reference = match r.service.as_str() {
        "github" | "github-enterprise" => format!("refs/pull/{}/head", pr.number),
        "gitlab" | "gitlab-self" => format!("refs/merge-requests/{}/head", pr.number),
        "bitbucket-server" => format!("refs/pull-requests/{}/from", pr.number),
        _ => pr.source_sha.clone(),
    };
    let url = if matches!(r.service.as_str(), "github" | "github-enterprise" | "gitlab" | "gitlab-self" | "bitbucket-server") { r.web.clone() } else { pr.source_url.clone() };
    if !url.starts_with("https://") || reference.is_empty() || pr.source_sha.is_empty() {
        return Err("The pull request has no available source repository or commit. Refresh it or open it in the browser.".into());
    }
    Ok(PrCheckout { url, reference, expected: pr.source_sha.clone() })
}

/// A repository's issue tracker (Preferences > Issue Tracker); kind "" uses the hosting service.
#[derive(Deserialize, Serialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Tracker {
    pub kind: String,
    /// Jira project key, Trello board id, or Azure DevOps project.
    pub project: String,
}

pub fn issues(r: &RemoteRepo) -> Result<Vec<Issue>, String> {
    let b = api_base(&r.service);
    Ok(match r.service.as_str() {
        "gitea" | "forgejo" | "codeberg" => list(&rget(r, &format!("{b}/repos/{}/issues?state=open&type=issues&limit=50", r.path))?).iter()
            .filter(|issue| issue["pull_request"].is_null()).map(|issue| forge_issue(issue, &r.service)).collect(),
        "github" | "github-enterprise" => list(&rget(r, &format!("{b}/repos/{}/issues?state=open&per_page=50", r.path))?).iter().filter(|i| i.get("pull_request").is_none()).map(github_issue).collect(),
        "gitlab" | "gitlab-self" => list(&rget(r, &format!("{b}/projects/{}/issues?state=opened&per_page=50", enc(&r.path)))?).iter().map(gitlab_issue).collect(),
        "bitbucket" => match rget(r, &format!("{b}/repositories/{}/issues?pagelen=50&q={}", r.path, q("state=\"new\" OR state=\"open\""))) {
            Ok(v) => list(&v["values"])
                .iter()
                .map(|i| Issue {
                    number: i["id"].as_u64().unwrap_or(0),
                    title: s(&i["title"]),
                    author: s(&i["reporter"]["display_name"]),
                    state: s(&i["state"]),
                    url: s(&i["links"]["html"]["href"]),
                    updated: s(&i["updated_on"]),
                    body: s(&i["content"]["raw"]),
                    tracker: "bitbucket".into(),
                    ..Default::default()
                })
                .collect(),
            Err(e) if e.starts_with("404") => vec![], // the repository has no issue tracker
            Err(e) => return Err(e),
        },
        "azure" | "azure-server" => {
            let (project_url, _) = azure_project(r);
            let project = url::Url::parse(&project_url).ok()
                .and_then(|url| url.path_segments()?.next_back().map(str::to_owned))
                .ok_or("The Azure repository has no project.")?;
            let project = percent_decode_path(&project);
            tracker_issues(&Tracker { kind: r.service.clone(), project }, false)?
        }
        _ => vec![],
    })
}

/// Issues from a separate tracker (Jira, Trello, Azure Boards): for a project / board, or the ones assigned to you.
fn percent_decode_path(value: &str) -> String {
    let mut encoded = url::Url::parse("https://fixture.invalid").expect("static URL");
    encoded.set_query(Some(&format!("path={}", value.replace('+', "%2B").replace('&', "%26"))));
    encoded.query_pairs().next().map(|(_, value)| value.into_owned()).unwrap_or_default()
}

fn azure_workitem_page(url: &str, endpoint: &str, base: &str, version: &str, scope: &str, mut fetch: impl FnMut(&str, &str, Option<Value>) -> Result<Value, String>) -> Result<crate::hosting::HttpPage, String> {
    let fields = "System.Title,System.State,System.ChangedDate,System.Tags,System.CreatedBy,System.TeamProject";
                let after = crate::pagination::parameter(url, "gitcraftAfter").and_then(|v| v.parse::<u64>().ok()).ok_or("Invalid work-item continuation")?;
                let wiql = format!("Select [System.Id] From WorkItems Where {scope} And [System.State] Not In ('Closed','Done','Removed') And [System.Id] > {after} Order By [System.Id] Asc");
                let ids = fetch("POST", endpoint, Some(json!({"query":wiql})))?;
                let ids: Vec<u64> = list(&ids["workItems"]).iter().filter_map(|v| v["id"].as_u64()).collect();
                if ids.iter().any(|id| *id <= after) { return Err("The service returned a repeated work-item page.".into()); }
                let next = if ids.len() == 50 { ids.iter().max().map(|id| crate::pagination::with_parameter(url, "gitcraftAfter", &id.to_string())) } else { None };
                let mut value = if ids.is_empty() { json!({"value":[]}) } else {
                    let ids = ids.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
                    fetch("GET", &format!("{base}/_apis/wit/workitems?ids={ids}&fields={fields}&api-version={version}"), None)?
                };
                if let Some(next) = next { value["next"] = Value::String(next); }
                Ok(crate::hosting::HttpPage { value, link: None, next_page: None, continuation: None })
}

pub fn tracker_issues(t: &Tracker, mine: bool) -> Result<Vec<Issue>, String> {
    let id = t.kind.as_str();
    match id {
        "jira" | "jira-server" => {
            let jql = if !mine && !t.project.trim().is_empty() {
                format!("project = \"{}\" AND resolution = Unresolved ORDER BY updated DESC", t.project.trim().replace('\\', "\\\\").replace('"', "\\\""))
            } else {
                "assignee = currentUser() AND resolution = Unresolved ORDER BY updated DESC".to_string()
            };
            let fields = "summary,status,updated,reporter,labels,description,project";
            let path = if id == "jira" { format!("/search/jql?jql={}&maxResults=50&fields={fields}", q(&jql)) } else { format!("/search?jql={}&maxResults=50&fields={fields}", q(&jql)) };
            Ok(list(&get(id, &path)?["issues"]).iter().map(|i| jira_issue(i, id)).collect())
        }
        "trello" => {
            let path = if !mine && !t.project.trim().is_empty() { format!("/boards/{}/cards/open", t.project.trim()) } else { "/members/me/cards?filter=open".into() };
            Ok(list(&get(id, &path)?).iter().map(trello_issue).collect())
        }
        "azure" | "azure-server" => {
            if id == "azure" && config().azure_org.trim().is_empty() || id == "azure-server" && crate::hosting::azure_server_base().is_none() {
                return Err("Set your Azure DevOps organization or collection in Preferences > Integrations.".into());
            }
            let base = api_base(id);
            let version = if id == "azure-server" { "7.0" } else { "7.1" };
            let scope = if mine || t.project.trim().is_empty() { "[System.AssignedTo] = @Me".to_string() } else { format!("[System.TeamProject] = '{}'", t.project.replace('\'', "''")) };
            let a = auth(id)?;
            let endpoint = format!("{base}/_apis/wit/wiql?api-version={version}&$top=50");
            let start = format!("{endpoint}&gitcraftScope={}&gitcraftAfter=0", q(&scope));
            let items = crate::pagination::collect(&start, |url| azure_workitem_page(url, &endpoint, &base, version, &scope, |method, url, body| http(method, url, &a, body)))?;
            Ok(list(&items["value"])
                .iter()
                .map(|w| {
                    let f = &w["fields"];
                    let n = w["id"].as_u64().unwrap_or(0);
                    Issue {
                        number: n,
                        title: s(&f["System.Title"]),
                        author: s(&f["System.CreatedBy"]["displayName"]),
                        state: s(&f["System.State"]),
                        url: format!("{base}/_workitems/edit/{n}"),
                        labels: s(&f["System.Tags"]).split(';').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(),
                        updated: s(&f["System.ChangedDate"]),
                        repo: s(&f["System.TeamProject"]),
                        tracker: id.into(),
                        ..Default::default()
                    }
                })
                .collect())
        }
        _ => Err("Choose an issue tracker".into()),
    }
}

/// "Create Issue" (IssueTrackerCreateForm): GitHub, GitLab, Bitbucket and Jira.
pub fn create_issue(r: Option<&RemoteRepo>, t: &Tracker, title: &str, body: &str) -> Result<Issue, String> {
    match t.kind.as_str() {
        "jira" | "jira-server" => {
            let id = t.kind.as_str();
            let description = if id == "jira" {
                json!({ "type": "doc", "version": 1, "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": if body.is_empty() { " " } else { body } }] }] })
            } else {
                json!(body)
            };
            let v = send(id, "POST", "/issue", json!({ "fields": { "project": { "key": t.project.trim() }, "summary": title, "description": description, "issuetype": { "name": "Task" } } }))?;
            let key = s(&v["key"]);
            Ok(Issue { title: title.into(), url: format!("https://{}/browse/{key}", host_of(id)), number: key.rsplit('-').next().and_then(|n| n.parse().ok()).unwrap_or(0), key, tracker: id.into(), ..Default::default() })
        }
        _ => {
            let r = r.ok_or("This repository's remote isn't on a connected service.")?;
            let b = api_base(&r.service);
            match r.service.as_str() {
                "gitea" | "forgejo" | "codeberg" => Ok(forge_issue(&rsend(r, "POST", &format!("{b}/repos/{}/issues", r.path), json!({ "title": title, "body": body }))?, &r.service)),
                "github" | "github-enterprise" => Ok(github_issue(&rsend(r, "POST", &format!("{b}/repos/{}/issues", r.path), json!({ "title": title, "body": body }))?)),
                "gitlab" | "gitlab-self" => Ok(gitlab_issue(&rsend(r, "POST", &format!("{b}/projects/{}/issues", enc(&r.path)), json!({ "title": title, "description": body }))?)),
                "bitbucket" => {
                    let v = rsend(r, "POST", &format!("{b}/repositories/{}/issues", r.path), json!({ "title": title, "content": { "raw": body } }))?;
                    Ok(Issue { number: v["id"].as_u64().unwrap_or(0), title: title.into(), url: s(&v["links"]["html"]["href"]), tracker: "bitbucket".into(), ..Default::default() })
                }
                _ => Err("Creating issues isn't available for this service.".into()),
            }
        }
    }
}

pub fn comments(r: &RemoteRepo, number: u64) -> Result<Vec<Comment>, String> {
    let b = api_base(&r.service);
    let c = |id: String, author: String, body: String, created: String| Comment { id, author, body, created };
    Ok(match r.service.as_str() {
        "github" | "github-enterprise" | "gitea" | "forgejo" | "codeberg" => list(&rget(r, &format!("{b}/repos/{}/issues/{number}/comments?per_page=100&limit=100", r.path))?)
            .iter()
            .map(|x| c(s(&x["id"]), s(&x["user"]["login"]), s(&x["body"]), s(&x["created_at"])))
            .collect(),
        "gitlab" | "gitlab-self" => list(&rget(r, &format!("{b}/projects/{}/merge_requests/{number}/notes?sort=asc&per_page=100", enc(&r.path)))?)
            .iter()
            .filter(|x| !x["system"].as_bool().unwrap_or(false))
            .map(|x| c(s(&x["id"]), s(&x["author"]["username"]), s(&x["body"]), s(&x["created_at"])))
            .collect(),
        "bitbucket" => list(&rget(r, &format!("{b}/repositories/{}/pullrequests/{number}/comments?pagelen=100", r.path))?["values"])
            .iter()
            .filter(|x| !x["deleted"].as_bool().unwrap_or(false))
            .map(|x| c(s(&x["id"]), s(&x["user"]["display_name"]), s(&x["content"]["raw"]), s(&x["created_on"])))
            .collect(),
        "bitbucket-server" => list(&rget(r, &format!("{b}{}/pull-requests/{number}/activities?limit=100", bbs_path(r)))?["values"])
            .iter()
            .filter(|x| x["action"] == "COMMENTED")
            .map(|x| c(s(&x["comment"]["id"]), s(&x["comment"]["author"]["displayName"]), s(&x["comment"]["text"]), ms_to_iso(&x["comment"]["createdDate"])))
            .rev()
            .collect(),
        "azure" | "azure-server" => list(&rget(r, &azure_url(r, &format!("/pullRequests/{number}/threads?api-version=7.1")))?["value"])
            .iter()
            .flat_map(|t| list(&t["comments"]).into_iter().map(move |x| (t, x)))
            .filter(|(_, x)| x["commentType"] == "text")
            .map(|(t, x)| c(format!("{}:{}", s(&t["id"]), s(&x["id"])), s(&x["author"]["displayName"]), s(&x["content"]), s(&x["publishedDate"])))
            .collect(),
        _ => vec![],
    })
}

pub fn issue_comments(r: &RemoteRepo, number: u64) -> Result<Vec<Comment>, String> {
    let base = api_base(&r.service);
    let url = match r.service.as_str() {
        "github" | "github-enterprise" => format!("{base}/repos/{}/issues/{number}/comments?per_page=50", r.path),
        "gitea" | "forgejo" | "codeberg" => format!("{base}/repos/{}/issues/{number}/comments?limit=50", r.path),
        "gitlab" | "gitlab-self" => format!("{base}/projects/{}/issues/{number}/notes?sort=asc&per_page=50", enc(&r.path)),
        "bitbucket" => format!("{base}/repositories/{}/issues/{number}/comments?pagelen=50", r.path),
        _ => return Ok(Vec::new()),
    };
    let result = rget(r, &url)?;
    let mut comments = Vec::new();
    for row in list(result.as_array().map(|_| &result).unwrap_or(&result["values"])) {
        if row["system"] == true || row["deleted"] == true { continue; }
        let (author, body, created) = match r.service.as_str() {
            "gitlab" | "gitlab-self" => (s(&row["author"]["username"]), s(&row["body"]), s(&row["created_at"])),
            "bitbucket" => (s(&row["user"]["display_name"]), s(&row["content"]["raw"]), s(&row["created_on"])),
            _ => (s(&row["user"]["login"]), s(&row["body"]), s(&row["created_at"])),
        };
        comments.push(Comment { id: s(&row["id"]), author, body, created });
    }
    Ok(comments)
}

pub fn add_comment(r: &RemoteRepo, number: u64, body: &str) -> Result<(), String> {
    let b = api_base(&r.service);
    match r.service.as_str() {
        "github" | "github-enterprise" | "gitea" | "forgejo" | "codeberg" => rsend(r, "POST", &format!("{b}/repos/{}/issues/{number}/comments", r.path), json!({ "body": body })),
        "gitlab" | "gitlab-self" => rsend(r, "POST", &format!("{b}/projects/{}/merge_requests/{number}/notes", enc(&r.path)), json!({ "body": body })),
        "bitbucket" => rsend(r, "POST", &format!("{b}/repositories/{}/pullrequests/{number}/comments", r.path), json!({ "content": { "raw": body } })),
        "bitbucket-server" => rsend(r, "POST", &format!("{b}{}/pull-requests/{number}/comments", bbs_path(r)), json!({ "text": body })),
        "azure" | "azure-server" => rsend(r, "POST", &azure_url(r, &format!("/pullRequests/{number}/threads?api-version=7.1")), json!({ "comments": [{ "parentCommentId": 0, "content": body, "commentType": 1 }], "status": 1 })),
        _ => Err("Not supported for this service".into()),
    }
    .map(|_| ())
}

pub fn create_pr(r: &RemoteRepo, source: &str, target: &str, title: &str, body: &str, draft: bool) -> Result<PullRequest, String> {
    let b = api_base(&r.service);
    match r.service.as_str() {
        "gitea" | "forgejo" | "codeberg" => {
            let title = if draft && !title.starts_with("WIP:") { format!("WIP: {title}") } else { title.into() };
            Ok(forge_pr(&rsend(r, "POST", &format!("{b}/repos/{}/pulls", r.path), json!({ "title": title, "head": source, "base": target, "body": body }))?))
        }
        "github" | "github-enterprise" => Ok(github_pr(&rsend(r, "POST", &format!("{b}/repos/{}/pulls", r.path), json!({ "title": title, "head": source, "base": target, "body": body, "draft": draft }))?)),
        "gitlab" | "gitlab-self" => {
            let title = if draft { format!("Draft: {title}") } else { title.to_string() };
            Ok(gitlab_mr(&rsend(r, "POST", &format!("{b}/projects/{}/merge_requests", enc(&r.path)), json!({ "source_branch": source, "target_branch": target, "title": title, "description": body }))?))
        }
        "bitbucket" => Ok(bitbucket_pr(&rsend(
            r,
            "POST",
            &format!("{b}/repositories/{}/pullrequests", r.path),
            json!({ "title": title, "description": body, "draft": draft, "source": { "branch": { "name": source } }, "destination": { "branch": { "name": target } } }),
        )?)),
        "bitbucket-server" => Ok(bitbucket_server_pr(&rsend(
            r,
            "POST",
            &format!("{b}{}/pull-requests", bbs_path(r)),
            json!({ "title": title, "description": body, "draft": draft, "fromRef": { "id": format!("refs/heads/{source}") }, "toRef": { "id": format!("refs/heads/{target}") } }),
        )?)),
        "azure" | "azure-server" => Ok(azure_pr(
            &rsend(
                r,
                "POST",
                &azure_url(r, "/pullrequests?api-version=7.1"),
                json!({ "sourceRefName": format!("refs/heads/{source}"), "targetRefName": format!("refs/heads/{target}"), "title": title, "description": body, "isDraft": draft }),
            )?,
            &r.web,
        )),
        _ => Err("Not supported for this service".into()),
    }
}

/// method: "merge", "squash" or "rebase".
static MERGE_WAITERS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, std::sync::Weak<std::sync::atomic::AtomicBool>>>> = std::sync::OnceLock::new();
fn merge_key(r: &RemoteRepo, number: u64) -> String { format!("{}|{}|{}|{number}", r.service, r.host, r.path) }
struct MergeWait { key: String, cancelled: std::sync::Arc<std::sync::atomic::AtomicBool> }
impl MergeWait {
    fn new(r: &RemoteRepo, number: u64) -> Result<Self, String> {
        let key=merge_key(r,number);let mut waiters=MERGE_WAITERS.get_or_init(Default::default).lock().unwrap();
        if waiters.get(&key).and_then(std::sync::Weak::upgrade).is_some() { return Err("This pull request already has a merge request running.".into()); }
        let cancelled=std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));waiters.insert(key.clone(),std::sync::Arc::downgrade(&cancelled));Ok(Self {key,cancelled})
    }
    fn is_cancelled(&self) -> bool { self.cancelled.load(std::sync::atomic::Ordering::Acquire) }
}
impl Drop for MergeWait { fn drop(&mut self) { MERGE_WAITERS.get_or_init(Default::default).lock().unwrap().remove(&self.key); } }
pub fn cancel_merge(r: &RemoteRepo, number: u64) -> bool {
    let flag=MERGE_WAITERS.get_or_init(Default::default).lock().unwrap().get(&merge_key(r,number)).and_then(std::sync::Weak::upgrade);
    if let Some(flag)=flag { flag.store(true,std::sync::atomic::Ordering::Release); true } else { false }
}

fn await_gitlab_rebase(mut read: impl FnMut() -> Result<Value,String>, mut pause: impl FnMut(), cancelled: impl Fn() -> bool, expected: &Value, timeout: std::time::Duration) -> Result<Value,String> {
    let start=std::time::Instant::now();
    loop {
        if cancelled() { return Err("Stopped waiting for the rebase. The server may still finish the queued rebase; no merge was sent by this operation.".into()); }
        if start.elapsed() >= timeout { return Err("The server rebase is still running. Refresh the pull request before merging.".into()); }
        pause();
        if cancelled() { continue; }
        let current=read()?;
        for field in ["source_project_id","source_branch","target_branch"] {
            if current[field] != expected[field] { return Err("The merge request source or target changed during rebase. Refresh it before merging.".into()); }
        }
        if let Some(error)=current["merge_error"].as_str().filter(|error|!error.is_empty()) { return Err(format!("The server could not rebase this merge request: {error}")); }
        match current["rebase_in_progress"].as_bool() {
            Some(true) => {},
            Some(false) if current["sha"].as_str().is_some_and(|sha| !sha.is_empty()) => return Ok(current),
            _ => return Err("The server did not report a completed rebase and source commit. Refresh before merging.".into()),
        }
    }
}

pub fn merge_pr(r: &RemoteRepo, number: u64, method: &str, expected: Option<&str>, squash_message: Option<&str>) -> Result<(), String> {
    let squash_message = squash_message.map(str::trim).filter(|message| !message.is_empty());
    if squash_message.is_some_and(|message| method != "squash" || message.len() > 65536 || message.contains('\0')) {
        return Err("A custom squash message requires the Squash strategy and must fit within 64 KiB.".into());
    }
    if expected.is_some_and(|sha| !matches!(sha.len(), 40 | 64) || !sha.bytes().all(|byte| byte.is_ascii_hexdigit())) {
        return Err("The pull request source is invalid. Refresh it before merging.".into());
    }
    if r.service == "bitbucket-server" && method != "default" {
        return Err("This service uses its configured server default merge strategy. Refresh the pull request and select Server default.".into());
    }
    let waiting = MergeWait::new(r, number)?;
    let b = api_base(&r.service);
    match r.service.as_str() {
        "github" | "github-enterprise" => {
            let mut body = json!({ "merge_method": method });
            if let Some(sha) = expected { body["sha"] = json!(sha); }
            if let Some(message) = squash_message {
                let (title, description) = message.split_once('\n').unwrap_or((message, ""));
                body["commit_title"] = json!(title.trim());
                body["commit_message"] = json!(description.trim());
            }
            let result = rsend(r, "PUT", &format!("{b}/repos/{}/pulls/{number}/merge", r.path), body)?;
            if result["merged"] == true { Ok(()) }
            else { Err(result["message"].as_str().unwrap_or("The server did not confirm the merge.").into()) }
        }
        "gitea" | "forgejo" | "codeberg" => {
            let url = format!("{b}/repos/{}/pulls/{number}", r.path);
            let mut body = json!({ "do": match method { "rebase_merge" => "rebase-merge", "fast_forward" => "fast-forward-only", "merge" | "rebase" | "squash" => method, _ => return Err("Choose a supported merge method.".into()) } });
            if let Some(sha) = expected { body["head_commit_id"] = json!(sha); }
            if let Some(message) = squash_message {
                let (title, content) = message.split_once('\n').unwrap_or((message, ""));
                body["merge_title_field"] = json!(title.trim());
                body["merge_message_field"] = json!(content.trim());
            }
            rsend(r, "POST", &format!("{url}/merge"), body).map(|_| ())
        }
        "gitlab" | "gitlab-self" => {
            let url = format!("{b}/projects/{}/merge_requests/{number}", enc(&r.path));
            let mut body = if method == "default" { json!({}) } else { json!({ "squash": method == "squash" }) };
            if let Some(sha) = expected { body["sha"] = json!(sha); }
            if let Some(message) = squash_message { body["squash_commit_message"] = json!(message); }
            if method == "rebase" {
                let current = rget(r, &url)?;
                ensure_merge_source(expected, &current["sha"])?;
                http("PUT", &format!("{url}/rebase"), &auth(&r.service)?, None)?;
                let completed = await_gitlab_rebase(|| rget(r, &format!("{url}?include_rebase_in_progress=true")), || {
                    for _ in 0..10 { if waiting.is_cancelled() { break; } std::thread::sleep(std::time::Duration::from_millis(100)); }
                }, || waiting.is_cancelled(), &current, std::time::Duration::from_secs(60))?;
                body["sha"] = completed["sha"].clone();
            }
            if waiting.is_cancelled() { return Err("Merge cancelled before it was sent.".into()); }
            rsend(r, "PUT", &format!("{url}/merge"), body).map(|_| ())
        }
        "bitbucket" => {
            let url = format!("{b}/repositories/{}/pullrequests/{number}", r.path);
            if expected.is_some() { ensure_merge_source(expected, &rget(r, &url)?["source"]["commit"]["hash"])?; }
            let mut body = match method {
                "default" => json!({}),
                "squash" => json!({ "merge_strategy": "squash" }),
                "fast_forward" | "rebase" => json!({ "merge_strategy": "fast_forward" }),
                _ => json!({ "merge_strategy": "merge_commit" }),
            };
            if let Some(message) = squash_message { body["message"] = json!(message); }
            let response = rsend(r, "POST", &format!("{url}/merge"), body)?;
            if response["state"] == "MERGED" { Ok(()) } else { Err("The server has not confirmed the merge. Refresh the pull request to see its status.".into()) }
        }
        "bitbucket-server" => {
            let pr = rget(r, &format!("{b}{}/pull-requests/{number}", bbs_path(r)))?;
            ensure_merge_source(expected, &pr["fromRef"]["latestCommit"])?;
            let version = pr["version"].as_i64().unwrap_or(0);
            rsend(r, "POST", &format!("{b}{}/pull-requests/{number}/merge?version={version}", bbs_path(r)), json!({})).map(|_| ())
        }
        "azure" | "azure-server" => {
            let pr = rget(r, &azure_url(r, &format!("/pullrequests/{number}?api-version=7.1")))?;
            ensure_merge_source(expected, &pr["lastMergeSourceCommit"]["commitId"])?;
            let strategy = match method {
                "squash" => "squash",
                "rebase" => "rebase",
                "rebase_merge" => "rebaseMerge",
                _ => "noFastForward",
            };
            let mut body = json!({ "status": "completed", "lastMergeSourceCommit": pr["lastMergeSourceCommit"], "completionOptions": { "mergeStrategy": strategy } });
            if let Some(message) = squash_message { body["completionOptions"]["mergeCommitMessage"] = json!(message); }
            rsend(r, "PATCH", &azure_url(r, &format!("/pullrequests/{number}?api-version=7.1")), body).map(|_| ())
        }
        _ => Err("Not supported for this service".into()),
    }
}

#[derive(Serialize)]
struct ReviewDiff {
    path: String,
    patch: String,
}

#[derive(Serialize)]
struct ReviewComment {
    id: String,
    path: String,
    line: Option<u64>,
    side: String,
    author: String,
    body: String,
}

#[derive(Serialize)]
pub struct ReviewData {
    reviewers: Vec<String>,
    reviews: Vec<(String, String, String)>,
    files: Vec<ReviewDiff>,
    comments: Vec<ReviewComment>,
}

fn github_review_url(r: &RemoteRepo, number: u64) -> Result<String, String> {
    if !matches!(r.service.as_str(), "github" | "github-enterprise") {
        return Err("This hosting service does not support pull request reviews through GitCraft.".into());
    }
    Ok(format!("{}/repos/{}/pulls/{number}", api_base(&r.service), r.path))
}

pub fn review_data(r: &RemoteRepo, number: u64, expected: &str) -> Result<ReviewData, String> {
    if matches!(r.service.as_str(), "gitlab" | "gitlab-self") {
        return gitlab_review_data(r, number, expected);
    }
    let url = github_review_url(r, number)?;
    let detail = rget(r, &url)?;
    ensure_merge_source(Some(expected), &detail["head"]["sha"])?;
    let requested = rget(r, &format!("{url}/requested_reviewers"))?;
    let reviewers = list(&requested["users"]).iter().map(|row| s(&row["login"]))
        .chain(list(&requested["teams"]).iter().map(|row| format!("Team {}", s(&row["slug"])))).collect();
    let reviews = list(&rget(r, &format!("{url}/reviews?per_page=100"))?).iter()
        .filter(|&row| row["state"] != "PENDING").map(|row| (s(&row["user"]["login"]), s(&row["state"]), s(&row["body"])))
        .collect();
    let files = list(&rget(r, &format!("{url}/files?per_page=100"))?).into_iter().take(3000)
        .filter_map(|row| Some(ReviewDiff { path: row["filename"].as_str()?.into(), patch: row["patch"].as_str()?.into() }))
        .filter(|row| row.patch.len() <= 256 * 1024)
        .collect();
    let comments = list(&rget(r, &format!("{url}/comments?per_page=100"))?).into_iter()
        .map(|row| ReviewComment { id: s(&row["id"]), path: s(&row["path"]), line: row["line"].as_u64(),
            side: s(&row["side"]), author: s(&row["user"]["login"]), body: s(&row["body"]) }).collect();
    ensure_merge_source(Some(expected), &rget(r, &url)?["head"]["sha"])?;
    Ok(ReviewData { reviewers, reviews, files, comments })
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ReviewAction {
    Approve { body: String },
    RequestChanges { body: String },
    RequestReviewers { reviewers: Vec<String> },
    Inline { path: String, line: u64, side: String, body: String, old_line: Option<u64> },
}

pub fn submit_review(r: &RemoteRepo, number: u64, expected: &str, action: ReviewAction) -> Result<(), String> {
    if matches!(r.service.as_str(), "gitlab" | "gitlab-self") {
        return submit_gitlab_review(r, number, expected, action);
    }
    let url = github_review_url(r, number)?;
    let detail = rget(r, &url)?;
    ensure_merge_source(Some(expected), &detail["head"]["sha"])?;
    if detail["state"] != "open" { return Err("The pull request must be open to submit a review.".into()); }
    let body_ok = |body: &str| body.len() <= 64 * 1024 && !body.contains('\0');
    let (endpoint, body) = match action {
        ReviewAction::Approve { body } if body_ok(&body) => ("reviews", json!({ "event": "APPROVE", "commit_id": expected, "body": body.trim() })),
        ReviewAction::RequestChanges { body } if body_ok(&body) && !body.trim().is_empty() => ("reviews", json!({ "event": "REQUEST_CHANGES", "commit_id": expected, "body": body.trim() })),
        ReviewAction::RequestReviewers { reviewers } if !reviewers.is_empty() && reviewers.len() <= 15 && reviewers.iter().all(|name| !name.is_empty() && name.len() <= 100 && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'[' | b']'))) =>
            ("requested_reviewers", json!({ "reviewers": reviewers })),
        ReviewAction::Inline { path, line, side, body, .. } if line > 0 && !path.is_empty() && !path.contains('\0') && path.len() <= 4096 && matches!(side.as_str(), "LEFT" | "RIGHT") && body_ok(&body) && !body.trim().is_empty() =>
            ("comments", json!({ "path": path, "line": line, "side": side, "commit_id": expected, "body": body.trim() })),
        _ => return Err("Review the selected line, message or reviewer names and try again.".into()),
    };
    rsend(r, "POST", &format!("{url}/{endpoint}"), body).map(|_| ())
}

fn gitlab_review_url(r: &RemoteRepo, number: u64) -> String {
    format!("{}/projects/{}/merge_requests/{number}", api_base(&r.service), enc(&r.path))
}

fn gitlab_review_data(r: &RemoteRepo, number: u64, expected: &str) -> Result<ReviewData, String> {
    let url = gitlab_review_url(r, number);
    ensure_merge_source(Some(expected), &rget(r, &url)?["sha"])?;
    let assigned = list(&rget(r, &format!("{url}/reviewers"))?);
    let reviewers = assigned.iter().filter(|row| matches!(row["state"].as_str(), Some("unreviewed" | "review_started" | "unapproved") | None))
        .map(|row| s(&row["user"]["username"])).collect();
    let reviews = assigned.iter().filter(|row| matches!(row["state"].as_str(), Some("approved" | "reviewed" | "requested_changes")))
        .map(|row| (s(&row["user"]["username"]), s(&row["state"]), String::new())).collect();
    let files = list(&rget(r, &format!("{url}/diffs?per_page=100"))?).into_iter().take(3000)
        .filter_map(|row| Some(ReviewDiff { path: row["new_path"].as_str()?.into(), patch: row["diff"].as_str()?.into() }))
        .filter(|row| !row.patch.is_empty() && row.patch.len() <= 256 * 1024)
        .collect();
    let comments = list(&rget(r, &format!("{url}/discussions?per_page=100"))?).into_iter()
        .flat_map(|thread| list(&thread["notes"])).filter(|note| note["position"].is_object())
        .map(|note| {
            let position = &note["position"];
            let right = position["new_line"].as_u64();
            ReviewComment {
                id: s(&note["id"]), path: s(&position["new_path"]), line: right.or(position["old_line"].as_u64()),
                side: if right.is_some() { "RIGHT".into() } else { "LEFT".into() }, author: s(&note["author"]["username"]), body: s(&note["body"]),
            }
        }).collect();
    ensure_merge_source(Some(expected), &rget(r, &url)?["sha"])?;
    Ok(ReviewData { reviewers, reviews, files, comments })
}

fn gitlab_review_mutation(r: &RemoteRepo, operation: &str, query: &str, variables: Value) -> Result<(), String> {
    let api = api_base(&r.service);
    let origin = api.strip_suffix("/api/v4").ok_or("Invalid GitLab endpoint.")?;
    let result = rsend(r, "POST", &format!("{origin}/api/graphql"), json!({ "query": query, "variables": variables }))?;
    let value = &result["data"][operation];
    let messages: Vec<_> = list(&result["errors"]).iter().map(|row| s(&row["message"]))
        .chain(list(&value["errors"]).iter().map(s)).filter(|message| !message.is_empty()).collect();
    if !messages.is_empty() { return Err(messages.join("\n")); }
    if value["mergeRequest"]["id"].is_string() { Ok(()) } else { Err("GitLab did not confirm the review update.".into()) }
}

fn submit_gitlab_review(r: &RemoteRepo, number: u64, expected: &str, action: ReviewAction) -> Result<(), String> {
    let url = gitlab_review_url(r, number);
    let detail = rget(r, &url)?;
    ensure_merge_source(Some(expected), &detail["sha"])?;
    if detail["state"] != "opened" { return Err("The merge request must be open to submit a review.".into()); }
    let body_ok = |body: &str| body.len() <= 64 * 1024 && !body.contains('\0');
    let approve = matches!(&action, ReviewAction::Approve { .. });
    match action {
        ReviewAction::RequestReviewers { reviewers } if !reviewers.is_empty() && reviewers.len() <= 15 && reviewers.iter().all(|name| !name.is_empty() && name.len() <= 100 && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))) => {
            gitlab_review_mutation(r, "mergeRequestSetReviewers",
                "mutation($input: MergeRequestSetReviewersInput!) { mergeRequestSetReviewers(input: $input) { errors mergeRequest { id } } }",
                json!({ "input": { "projectPath": r.path, "iid": number.to_string(), "reviewerUsernames": reviewers, "operationMode": "APPEND" } }))
        }
        ReviewAction::Approve { body } | ReviewAction::RequestChanges { body } if body_ok(&body) => {
            if approve {
                rsend(r, "POST", &format!("{url}/approve"), json!({ "sha": expected }))?;
            } else {
                gitlab_review_mutation(r, "mergeRequestRequestChanges",
                    "mutation($input: MergeRequestRequestChangesInput!) { mergeRequestRequestChanges(input: $input) { errors mergeRequest { id } } }",
                    json!({ "input": { "projectPath": r.path, "iid": number.to_string() } }))?;
            }
            if !body.trim().is_empty() {
                add_comment(r, number, body.trim()).map_err(|error| format!("The review decision was saved. The summary comment could not be posted: {error}"))?;
            }
            Ok(())
        }
        ReviewAction::Inline { path, line, side, body, old_line } if line > 0 && !path.is_empty() && !path.contains('\0') &&
            path.len() <= 4096 && matches!(side.as_str(), "LEFT" | "RIGHT") && body_ok(&body) && !body.trim().is_empty() => {
            let revisions = rget(r, &format!("{url}/versions?per_page=1"))?;
            let revision = revisions.get(0).ok_or("GitLab did not return an available diff version.")?;
            ensure_merge_source(Some(expected), &revision["head_commit_sha"])?;
            let diff = list(&rget(r, &format!("{url}/diffs?per_page=100"))?).into_iter().find(|row| row["new_path"] == path)
                .ok_or("The file is no longer in the diff.")?;
            let mut position = json!({
                "head_sha": expected, "base_sha": revision["base_commit_sha"], "start_sha": revision["start_commit_sha"],
                "position_type": "text", "new_path": diff["new_path"], "old_path": diff["old_path"]
            });
            if side == "LEFT" { position["old_line"] = json!(line); }
            else {
                position["new_line"] = json!(line);
                if let Some(old) = old_line { position["old_line"] = json!(old); }
            }
            rsend(r, "POST", &format!("{url}/discussions"), json!({ "body": body.trim(), "position": position })).map(|_| ())
        }
        _ => Err("Review the selected line, message or reviewer names and try again.".into()),
    }
}

fn ensure_merge_source(expected: Option<&str>, actual: &Value) -> Result<(), String> {
    if expected.is_some() && expected != actual.as_str() {
        Err("The pull request source changed after review. Refresh it before merging.".into())
    } else { Ok(()) }
}

pub fn merged_source(r: &RemoteRepo, number: u64, expected: &str) -> Result<PullRequest, String> {
    let mut pr = if matches!(r.service.as_str(), "gitlab" | "gitlab-self") {
        let base = api_base(&r.service);
        let id = enc(&r.path);
        let detail = rget(r, &format!("{base}/projects/{id}/merge_requests/{number}"))?;
        let mut pr = gitlab_mr(&detail);
        if detail["source_project_id"].is_number() && detail["source_project_id"] == detail["target_project_id"] {
            pr.source_url = r.web.clone();
        } else if let Some(project) = detail["source_project_id"].as_u64() {
            pr.source_url = s(&rget(r, &format!("{base}/projects/{project}"))?["http_url_to_repo"]);
        }
        pr
    } else { pull_request(r, number)? };
    if !matches!(pr.state.to_ascii_lowercase().as_str(), "merged" | "completed") {
        return Err("The hosting service has not confirmed that this pull request merged.".into());
    }
    if !matches!(expected.len(), 40 | 64) || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) || pr.source_sha != expected {
        return Err("The merged source is different from the reviewed commit. Review the branch before deleting it.".into());
    }
    if pr.source.is_empty() || pr.source == pr.target { return Err("The source branch cannot be safely identified.".into()); }
    if !pr.source_url.is_empty() && !url::Url::parse(&pr.source_url).is_ok_and(|url| url.scheme() == "https" && url.host_str().is_some() && url.username().is_empty() && url.password().is_none() && url.query().is_none() && url.fragment().is_none()) {
        pr.source_url.clear();
    }
    Ok(pr)
}

#[derive(Serialize, Default)]
pub struct Launchpad {
    pub pull_requests: Vec<PullRequest>,
    pub issues: Vec<Issue>,
    pub errors: Vec<String>,
}

/// Open pull requests and issues that involve you, across every connected service.
pub fn launchpad_page(service: &str, cursor: Option<&str>) -> Result<crate::pagination::Page<Launchpad>, String> {
    if !crate::hosting::SERVICES.contains(&service) { return Err("Unknown integration".into()); }
    let binding = format!("launchpad:{service}:{}", api_base(service));
    crate::pagination::composite_page(&binding, cursor, || Ok(launchpad_service(service)))
}

pub fn launchpad(_gitlab_host: &str) -> Launchpad {
    parallel_launchpad(&crate::hosting::SERVICES, launchpad_service)
}

fn parallel_launchpad(ids: &[&str], work: impl Fn(&str) -> Launchpad + Sync) -> Launchpad {
    let mut out = Launchpad::default();
    for group in ids.chunks(4) {
        std::thread::scope(|scope| {
            let tasks: Vec<_> = group.iter().map(|id| { let work = &work; scope.spawn(crate::read_jobs::inherit(move || work(id))) }).collect();
            for task in tasks {
                match task.join() {
                    Ok(result) => { out.pull_requests.extend(result.pull_requests); out.issues.extend(result.issues); out.errors.extend(result.errors); },
                    Err(_) => out.errors.push("A connected service stopped unexpectedly; other results are available.".into()),
                }
            }
        });
    }
    out.pull_requests.sort_by(|a, b| b.updated.cmp(&a.updated));
    out.pull_requests.dedup_by(|a, b| a.url == b.url);
    out.issues.sort_by(|a, b| b.updated.cmp(&a.updated));
    out
}

fn launchpad_service(only: &str) -> Launchpad {
    let mut out = Launchpad::default();
    let connected = |id: &str| id == only && secret_get(&secret_name(id)).is_some() && !host_of(id).is_empty();
    for id in ["github", "github-enterprise"] {
        if !connected(id) {
            continue;
        }
        match get(id, &format!("/search/issues?per_page=50&sort=updated&q={}", q("is:open is:pr involves:@me archived:false"))) {
            Ok(v) => out.pull_requests.extend(list(&v["items"]).iter().map(|i| {
                let mut p = PullRequest { number: i["number"].as_u64().unwrap_or(0), title: s(&i["title"]), author: s(&i["user"]["login"]), state: "open".into(), url: s(&i["html_url"]), updated: s(&i["updated_at"]), ..Default::default() };
                p.repo = github_issue(i).repo;
                p.draft = i["draft"].as_bool().unwrap_or(false);
                p
            })),
            Err(e) => out.errors.push(format!("{}: {e}", label(id))),
        }
        match get(id, &format!("/search/issues?per_page=50&sort=updated&q={}", q("is:open is:issue assignee:@me archived:false"))) {
            Ok(v) => out.issues.extend(list(&v["items"]).iter().map(github_issue)),
            Err(e) => out.errors.push(format!("{}: {e}", label(id))),
        }
    }
    for id in ["gitlab", "gitlab-self"] {
        if !connected(id) {
            continue;
        }
        for scope in ["created_by_me", "assigned_to_me"] {
            match get(id, &format!("/merge_requests?state=opened&scope={scope}&per_page=50")) {
                Ok(v) => out.pull_requests.extend(list(&v).iter().map(gitlab_mr)),
                Err(e) => out.errors.push(format!("{}: {e}", label(id))),
            }
        }
        match get(id, "/issues?state=opened&scope=assigned_to_me&per_page=50") {
            Ok(v) => out.issues.extend(list(&v).iter().map(gitlab_issue)),
            Err(e) => out.errors.push(format!("{}: {e}", label(id))),
        }
    }
    if connected("bitbucket") {
        match get("bitbucket", "/user").map(|u| s(&u["uuid"])) {
            Ok(uuid) => match get("bitbucket", &format!("/pullrequests/{}?state=OPEN&pagelen=50", q(&uuid))) {
                Ok(v) => out.pull_requests.extend(list(&v["values"]).iter().map(bitbucket_pr)),
                Err(e) => out.errors.push(format!("Bitbucket: {e}")),
            },
            Err(e) => out.errors.push(format!("Bitbucket: {e}")),
        }
    }
    if connected("bitbucket-server") {
        match get("bitbucket-server", "/dashboard/pull-requests?state=OPEN&limit=50") {
            Ok(v) => out.pull_requests.extend(list(&v["values"]).iter().map(bitbucket_server_pr)),
            Err(e) => out.errors.push(format!("Bitbucket Data Center: {e}")),
        }
    }
    let org = config().azure_org.trim().to_string();
    if connected("azure") && !org.is_empty() {
        if let Ok(a) = auth("azure") {
            match crate::pagination::collect(&format!("https://dev.azure.com/{org}/_apis/git/pullrequests?searchCriteria.status=active&$top=50&api-version=7.1"), |url| crate::hosting::http_page("GET", url, &a, None)) {
                Ok(v) => out.pull_requests.extend(list(&v["value"]).iter().map(|p| {
                    let mut pr = azure_pr(p, "");
                    pr.url = format!("https://dev.azure.com/{org}/{}/_git/{}/pullrequest/{}", s(&p["repository"]["project"]["name"]), s(&p["repository"]["name"]), pr.number);
                    pr
                })),
                Err(e) => out.errors.push(format!("Azure DevOps: {e}")),
            }
        }
        match tracker_issues(&Tracker { kind: "azure".into(), project: String::new() }, true) {
            Ok(i) => out.issues.extend(i),
            Err(e) => out.errors.push(format!("Azure DevOps: {e}")),
        }
    }
    for id in ["jira", "jira-server", "trello"] {
        if !connected(id) {
            continue;
        }
        match tracker_issues(&Tracker { kind: id.into(), project: String::new() }, true) {
            Ok(i) => out.issues.extend(i),
            Err(e) => out.errors.push(format!("{}: {e}", label(id))),
        }
    }
    out.pull_requests.sort_by(|a, b| b.updated.cmp(&a.updated));
    out.pull_requests.dedup_by(|a, b| a.url == b.url);
    out.issues.sort_by(|a, b| b.updated.cmp(&a.updated));
    out
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct HostedRepo {
    pub name: String,
    pub full_name: String,
    pub description: String,
    pub private: bool,
    pub https_url: String,
    pub ssh_url: String,
    pub updated: String,
}

fn clone_link(v: &Value, name: &str) -> String {
    list(v).iter().find(|l| l["name"] == name).map(|l| s(&l["href"])).unwrap_or_default()
}

/// Your repositories on a connected service, for finding a repository to clone.
pub fn repositories(id: &str) -> Result<Vec<HostedRepo>, String> {
    let repo = |name: String, full_name: String, description: String, private: bool, https_url: String, ssh_url: String, updated: String| HostedRepo { name, full_name, description, private, https_url, ssh_url, updated };
    Ok(match id {
        "github" | "github-enterprise" => list(&get(id, "/user/repos?per_page=100&sort=updated&affiliation=owner,collaborator,organization_member")?)
            .iter()
            .map(|r| repo(s(&r["name"]), s(&r["full_name"]), s(&r["description"]), r["private"].as_bool().unwrap_or(false), s(&r["clone_url"]), s(&r["ssh_url"]), s(&r["pushed_at"])))
            .collect(),
        "gitea" | "forgejo" | "codeberg" => list(&get(id, "/user/repos?limit=50")?)
            .iter().map(|r| repo(s(&r["name"]), s(&r["full_name"]), s(&r["description"]), r["private"] == true, s(&r["clone_url"]), s(&r["ssh_url"]), s(&r["updated_at"]))).collect(),
        "gitlab" | "gitlab-self" => list(&get(id, "/projects?membership=true&per_page=100&order_by=last_activity_at")?)
            .iter()
            .map(|r| repo(s(&r["path"]), s(&r["path_with_namespace"]), s(&r["description"]), r["visibility"] != "public", s(&r["http_url_to_repo"]), s(&r["ssh_url_to_repo"]), s(&r["last_activity_at"])))
            .collect(),
        "bitbucket" => list(&get(id, "/repositories?role=member&pagelen=100&sort=-updated_on")?["values"])
            .iter()
            .map(|r| {
                let l = &r["links"]["clone"];
                repo(s(&r["slug"]), s(&r["full_name"]), s(&r["description"]), r["is_private"].as_bool().unwrap_or(false), clone_link(l, "https"), clone_link(l, "ssh"), s(&r["updated_on"]))
            })
            .collect(),
        "bitbucket-server" => list(&get(id, "/repos?limit=100")?["values"])
            .iter()
            .map(|r| {
                let l = &r["links"]["clone"];
                let full = format!("{}/{}", s(&r["project"]["key"]), s(&r["slug"]));
                repo(s(&r["slug"]), full, s(&r["description"]), !r["public"].as_bool().unwrap_or(false), clone_link(l, "http"), clone_link(l, "ssh"), String::new())
            })
            .collect(),
        "azure" | "azure-server" => list(&get(id, if id == "azure" { "/_apis/git/repositories?api-version=7.1" } else { "/_apis/git/repositories?api-version=7.0" })?["value"])
            .iter()
            .map(|r| {
                let full = format!("{}/{}", s(&r["project"]["name"]), s(&r["name"]));
                repo(s(&r["name"]), full, String::new(), r["project"]["visibility"] != "public", s(&r["remoteUrl"]), s(&r["sshUrl"]), String::new())
            })
            .collect(),
        _ => return Err(format!("{} doesn't host repositories.", label(id))),
    })
}

/// The service's page for adding an SSH key by hand.
pub fn ssh_keys_page(id: &str) -> String {
    let h = host_of(id);
    match id {
        "github" | "github-enterprise" => format!("https://{h}/settings/ssh/new"),
        "gitlab" | "gitlab-self" => format!("https://{h}/-/user_settings/ssh_keys"),
        "bitbucket" => "https://bitbucket.org/account/settings/ssh-keys/".into(),
        "bitbucket-server" => format!("https://{h}/plugins/servlet/ssh/account/keys"),
        "azure" => format!("https://dev.azure.com/{}/_usersSettings/keys", config().azure_org.trim()),
        _ => String::new(),
    }
}

/// "Add SSH key to {service}" (Services-AddToService). GitHub needs a token with the write:public_key scope.
pub fn add_ssh_key(id: &str, title: &str, key: &str) -> Result<(), String> {
    let key = key.trim();
    match id {
        "github" | "github-enterprise" | "gitlab" | "gitlab-self" => send(id, "POST", "/user/keys", json!({ "title": title, "key": key })).map(|_| ()),
        "bitbucket" => {
            let uuid = s(&get(id, "/user")?["uuid"]);
            send(id, "POST", &format!("/users/{}/ssh-keys", q(&uuid)), json!({ "key": key, "label": title })).map(|_| ())
        }
        "bitbucket-server" => http("POST", &format!("https://{}/rest/ssh/1.0/keys", host_of(id)), &auth(id)?, Some(json!({ "text": key, "label": title }))).map(|_| ()),
        _ => Err(format!("{} doesn't accept SSH keys from other apps; copy the key and paste it on its page.", label(id))),
    }
}

#[allow(dead_code)]
fn _uses(_: Auth) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn azure_workitem_keyset_pages_do_not_truncate_or_send_virtual_parameters() {
        let endpoint = "https://dev.azure.com/fixture/_apis/wit/wiql?api-version=7.1&$top=50";
        let start = format!("{endpoint}&gitcraftScope=fixture&gitcraftAfter=0");
        let mut queries = Vec::new();
        let items = crate::pagination::collect(&start, |url| azure_workitem_page(url, endpoint, "https://dev.azure.com/fixture", "7.1", "[System.AssignedTo] = @Me", |method, url, body| {
            assert!(!url.contains("gitcraft"));
            if method == "POST" {
                let query = body.unwrap()["query"].as_str().unwrap().to_owned();
                let after = if query.contains("> 50 ") { 50 } else { 0 };
                queries.push(query);
                Ok(json!({"workItems": ((after+1)..=(if after==0 {50} else {73})).map(|id| json!({"id":id})).collect::<Vec<_>>()}))
            } else {
                let ids = crate::pagination::parameter(url, "ids").unwrap();
                Ok(json!({"value": ids.split(',').map(|id| json!({"id":id.parse::<u64>().unwrap()})).collect::<Vec<_>>()}))
            }
        })).unwrap();
        assert_eq!(items["value"].as_array().unwrap().len(), 73);
        assert_eq!(queries.len(), 2);
        assert!(queries[1].contains("[System.Id] > 50 Order By [System.Id] Asc"));
    }

    #[test]
    fn feature_hosting_normalizes_check_results_and_forge_pull_requests() {
        assert_eq!(check_state("in_progress"), "pending");
        assert_eq!(check_state("SUCCESSFUL"), "passed");
        assert_eq!(check_state("canceled"), "cancelled");
        assert_eq!(check_state("timed_out"), "failed");
        assert_eq!(check_state("SKIPPED"), "neutral");
        let pr = forge_pr(&json!({"number":17,"title":"WIP: Fixture","merged":true,"state":"closed","head":{"ref":"topic","sha":"abc"}}));
        assert!(pr.draft);
        assert_eq!(pr.source_sha, "abc");
        assert_eq!(pr.state, "merged");
        assert!(pull_person_matches("github", &json!({"requested_reviewers":[{"login":"fixture"}]}), PullFilter::ReviewRequested, "fixture"));
        assert!(!pull_person_matches("azure", &json!({"reviewers":[{"id":"account","vote":10}]}), PullFilter::ReviewRequested, "account"));

        let server = RemoteRepo {
            provider: crate::hosting::Provider::Azure, service: "azure-server".into(), host: "dev.example.invalid".into(),
            path: "tfs/DefaultCollection/project/repo".into(), web: "https://dev.example.invalid/tfs/DefaultCollection/project/_git/repo".into(),
        };
        assert_eq!(azure_url(&server, "/pullrequests?api-version=7.1"), "https://dev.example.invalid/tfs/DefaultCollection/project/_apis/git/repositories/repo/pullrequests?api-version=7.0");
    }

    #[test]
    fn maps_api_answers() {
        let pr = github_pr(&json!({ "number": 7, "title": "Add x", "user": { "login": "sam" }, "head": { "ref": "feat" }, "base": { "ref": "main", "repo": { "full_name": "o/r" } }, "draft": true, "state": "open", "html_url": "u" }));
        assert_eq!((pr.number, pr.source.as_str(), pr.target.as_str(), pr.draft, pr.repo.as_str()), (7, "feat", "main", true, "o/r"));
        let mr = gitlab_mr(&json!({ "iid": 3, "title": "Fix", "author": { "username": "jo" }, "source_branch": "a", "target_branch": "b", "state": "opened", "references": { "full": "g/p!3" } }));
        assert_eq!((mr.number, mr.repo.as_str()), (3, "g/p"));
        let issue = github_issue(&json!({ "number": 9, "title": "Bug", "user": { "login": "x" }, "repository_url": "https://api.github.com/repos/o/r", "labels": [{ "name": "bug" }] }));
        assert_eq!((issue.repo.as_str(), issue.labels.clone()), ("o/r", vec!["bug".to_string()]));
        let bb = bitbucket_pr(&json!({ "id": 5, "title": "T", "author": { "display_name": "Ann" }, "source": { "branch": { "name": "f" } }, "destination": { "branch": { "name": "main" }, "repository": { "full_name": "t/a" } }, "state": "OPEN", "links": { "html": { "href": "h" } } }));
        assert_eq!((bb.number, bb.source.as_str(), bb.state.as_str(), bb.repo.as_str()), (5, "f", "open", "t/a"));
        let az = azure_pr(&json!({ "pullRequestId": 12, "title": "T", "sourceRefName": "refs/heads/feat", "targetRefName": "refs/heads/main", "isDraft": false, "status": "active" }), "https://dev.azure.com/o/p/_git/r");
        assert_eq!((az.source.as_str(), az.target.as_str(), az.url.as_str()), ("feat", "main", "https://dev.azure.com/o/p/_git/r/pullrequest/12"));
        let j = jira_issue(&json!({ "key": "ABC-42", "fields": { "summary": "Do it", "status": { "name": "To Do" }, "description": { "type": "doc", "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "Hello" }] }] } } }), "jira");
        assert_eq!((j.number, j.key.as_str(), j.state.as_str(), j.body.as_str()), (42, "ABC-42", "To Do", "Hello"));
        let t = trello_issue(&json!({ "idShort": 4, "name": "Card", "url": "u", "closed": false, "labels": [{ "name": "ui" }, { "name": "" }] }));
        assert_eq!((t.key.as_str(), t.labels.clone()), ("#4", vec!["ui".to_string()]));
        assert_eq!(q("a b/c"), "a%20b%2Fc");
    }
}


#[cfg(test)]
mod concurrency_tests {
    use super::*;
    #[test]
    fn launchpad_bounds_workers_and_preserves_partial_errors() {
        use std::sync::atomic::{AtomicUsize,Ordering};
        let active=AtomicUsize::new(0);let peak=AtomicUsize::new(0);
        let started=std::time::Instant::now();
        let result=parallel_launchpad(&["a","b","c","d","e","f"],|id| {
            let count=active.fetch_add(1,Ordering::SeqCst)+1;peak.fetch_max(count,Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(25));active.fetch_sub(1,Ordering::SeqCst);
            Launchpad {errors:vec![id.into()],..Default::default()}
        });
        eprintln!("Six independent 25ms services: {:?}, sequential fixture budget 150ms, peak {} workers", started.elapsed(), peak.load(Ordering::SeqCst));
        assert_eq!(result.errors.len(),6);assert!(peak.load(Ordering::SeqCst)>1);assert!(peak.load(Ordering::SeqCst)<=4);
    }
}


#[cfg(test)]
mod rebase_wait_tests {
    use super::*;
    fn state(running:bool)->Value {json!({"source_project_id":1,"source_branch":"feature","target_branch":"main","rebase_in_progress":running,"sha":"rebased"})}
    #[test]
    fn waits_for_rebase_and_rejects_errors_cancellation_timeout_and_changed_source() {
        let expected=state(true);let mut polls=0;
        let completed=await_gitlab_rebase(||{polls+=1;Ok(state(polls<3))},||{},||false,&expected,std::time::Duration::from_secs(5)).unwrap();assert_eq!(polls,3);assert_eq!(completed["sha"],"rebased");
        let mut failed=state(false);failed["merge_error"]=json!("conflict");assert!(await_gitlab_rebase(||Ok(failed.clone()),||{},||false,&expected,std::time::Duration::from_secs(5)).unwrap_err().contains("conflict"));
        assert!(await_gitlab_rebase(||panic!("cancelled"),||{},||true,&expected,std::time::Duration::from_secs(5)).is_err());
        assert!(await_gitlab_rebase(||panic!("expired"),||{},||false,&expected,std::time::Duration::ZERO).is_err());
        let mut changed=state(false);changed["source_branch"]=json!("other");assert!(await_gitlab_rebase(||Ok(changed.clone()),||{},||false,&expected,std::time::Duration::from_secs(5)).is_err());
    }
}

#[cfg(test)]
mod server_merge_tests {
    #[test]
    fn refuses_specific_strategies_before_contacting_a_server_that_uses_defaults() {
        let remote = crate::hosting::RemoteRepo {provider: crate::hosting::Provider::Bitbucket, service:"bitbucket-server".into(),host:"example.invalid".into(),path:"P/repo".into(),web:"https://example.invalid".into()};
        for method in ["merge", "squash", "rebase"] {
            assert!(super::merge_pr(&remote, 1, method, None, None).unwrap_err().contains("server default"));
        }
    }
}
