use hyper::{Body, Request, Response};

pub(crate) async fn web_hook(req: Request<Body>) -> Result<Response<Body>, Box<dyn std::error::Error + Send + Sync>> {
    let event = req.headers().get("X-GitHub-Event").ok_or("no X-Github-Event header")?.to_str()?.to_string();
    let body: hyper::body::Bytes = hyper::body::to_bytes(req.into_body()).await?;
    let body = std::str::from_utf8(&*body)?;
    let json: serde_json::Value = serde_json::from_str(body)?;

    let repo = json
        .as_object()
        .ok_or("not json")?
        .get("repository")
        .and_then(|repo| repo.as_object()?.get("full_name")?.as_str())
        .ok_or("missing repo")?;
    if !crate::REPO_WHITELIST.iter().any(|&r| r == repo) {
        println!("wrong repo {:?}", json);
        return Ok(Response::new("wrong repo".into()));
    }

    match &*event {
        "issue_comment" => {
            let event: IssueCommentEvent = serde_json::from_value(json)?;

            if event.action != "created" {
                return Ok(Response::new("processed".into()));
            }
            println!("{:?} commented \"{}\"", event.sender.login, event.comment.body);
            crate::parse_comment(
                &crate::ReplyTo::Github { repo: event.repository.full_name.clone(), issue_number: event.issue.number },
                event.comment.id,
                &event.comment.body,
            ).await?;
        }
        "check_run" => {
            let event: CheckRunEvent = serde_json::from_value(json)?;
            println!("check_run action: {}", event.action);
            let reply_to = {
                let res = gh_api(&format!(
                    "https://api.github.com/repos/{}/git/commits/{}",
                    crate::JOB_REPO, event.check_run.head_sha,
                )).await?;
                let commit: Commit = serde_json::from_str(&res)?;
                crate::ReplyTo::from_commit_message(&commit.message).map_err(|()| format!("Failed to parse commit message {:?}", commit.message))?
            };
            println!("reply to: {:?}", reply_to);
            match &*event.action {
                "created" => {
                    reply_to.comment(&format!(
                        "bisect started: {}\n\nUse `{}cancel {}` to cancel the bisection.",
                        event.check_run.html_url, crate::BOT_NAME, event.check_run.id,
                    )).await?;
                }
                "completed" => {
                    reply_to.comment(&format!("bisect result: {}", event.check_run.html_url)).await?;
                }
                _ => {
                    println!("unknown check_run action");
                }
            }
        }
        _ => {
            println!("unknown event {}: {}", event, body);
            return Ok(Response::new("unknown event".into()));
        }
    }

    Ok(Response::new("processed".into()))
}

#[derive(serde::Deserialize)]
struct Repository {
    full_name: String,
}

#[derive(serde::Deserialize)]
struct Commit {
    message: String,
}

#[derive(serde::Deserialize)]
struct Issue {
    number: u64,
}

#[derive(serde::Deserialize)]
struct Comment {
    id: u64,
    body: String,
}

#[derive(serde::Deserialize)]
struct User {
    login: String,
}

#[derive(serde::Deserialize)]
struct IssueCommentEvent {
    action: String,
    repository: Repository,
    issue: Issue,
    comment: Comment,
    sender: User,
}

#[derive(serde::Deserialize)]
struct CheckRun {
    id: u64,
    head_sha: String,
    html_url: String,
}

#[derive(serde::Deserialize)]
struct CheckRunEvent {
    action: String,
    check_run: CheckRun,
    repository: Repository,
}

async fn gh_api(url: &str) -> reqwest::Result<String> {
    println!("GET {}", url);
    let res: reqwest::Response = reqwest::Client::new()
        .get(url)
        .header(hyper::http::header::USER_AGENT, hyper::http::HeaderValue::from_str(crate::USER_AGENT).unwrap())
        .header(hyper::http::header::ACCEPT, hyper::http::HeaderValue::from_str("application/vnd.github.antiope-preview+json").unwrap())
        .basic_auth(crate::GITHUB_USERNAME, Some(crate::GITHUB_TOKEN))
        .send()
        .await?;
    println!("GET {}: {}", url, res.status());
    match res.error_for_status_ref() {
        Ok(_) => res.text().await,
        Err(err) => {
            println!("{}", res.text().await?);
            return Err(err)
        }
    }
}

pub(crate) async fn gh_api_post(url: &str, body: String) -> reqwest::Result<String> {
    println!("POST {} <- {}", url, body);
    let res = reqwest::Client::new()
        .post(url)
        .header(hyper::http::header::USER_AGENT, hyper::http::HeaderValue::from_str(crate::USER_AGENT).unwrap())
        .header(hyper::http::header::ACCEPT, hyper::http::HeaderValue::from_str("application/vnd.github.v3.html+json").unwrap())
        .header(hyper::http::header::CONTENT_TYPE, hyper::http::HeaderValue::from_str("text/json").unwrap())
        .basic_auth(crate::GITHUB_USERNAME, Some(crate::GITHUB_TOKEN))
        .body(body)
        .send()
        .await?;
    println!("POST {}: {}", url, res.status());
    match res.error_for_status_ref() {
        Ok(_) => res.text().await,
        Err(err) => {
            println!("{}", res.text().await?);
            return Err(err)
        }
    }
}

pub(crate) async fn gh_post_comment(repo: &str, issue_number: u64, body: &str) -> reqwest::Result<()> {
    println!("on issue {} post comment {:?}", issue_number, body);
    let res = gh_api_post(
        &format!("https://api.github.com/repos/{}/issues/{}/comments", repo, issue_number),
        format!(r#"{{"body": {:?}}}"#, body),
    ).await?;
    println!("on issue {} post comment success", issue_number);
    Ok(())
}
