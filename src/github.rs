use hyper::{Body, Request, Response};

pub(crate) async fn web_hook(req: Request<Body>) -> Result<Response<Body>, Box<dyn std::error::Error + Send + Sync>> {
    let body: hyper::body::Bytes = hyper::body::to_bytes(req.into_body()).await?;
    let body = std::str::from_utf8(&*body)?;
    let json: serde_json::Value = serde_json::from_str(body)?;
    let json = json.as_object().ok_or_else(|| "not json")?;

    let repo = json.get("repository").and_then(|repo| repo.as_object()?.get("full_name")?.as_str());
    if repo != Some("bjorn3/cargo-bisect-rustc-bot") && repo != Some("bjorn3/cargo-bisect-rustc-bot-jobs") {
        println!("wrong repo {:?}", json);
        return Ok(Response::new("wrong repo".into()));
    }
    let repo = repo.unwrap();

    let sender = if let Some(sender) = json.get("sender").and_then(|sender| sender.as_object()?.get("login")?.as_str()) {
        sender
    } else {
        println!("no sender {:?}", json);
        return Ok(Response::new("no sender".into()));
    };

    match (
        json.get("comment").and_then(|action| action.as_object()),
        json.get("issue").and_then(|issue| issue.as_object()),
        json.get("check_run").and_then(|action| action.as_object()),
        json.get("action").and_then(|action| action.as_str()),
    ) {
        (Some(comment), Some(issue), None, Some("created")) => {
            if let (Some(issue_number), Some(comment_id), Some(comment_body)) = (issue.get("number").and_then(|id| id.as_u64()), comment.get("id").and_then(|id| id.as_u64()), comment.get("body").and_then(|body| body.as_str())) {
                println!("{:?} commented \"{}\"", sender, comment_body);
                crate::parse_comment(&crate::ReplyTo::Github { repo: repo.to_string(), issue_number }, comment_id, comment_body).await?;
            } else {
                println!("no comment body: {:#?}", json);
            }
        }
        (None, None, Some(check_run), Some("completed")) => {
            let reply_to = if let Some(head_sha) = check_run.get("head_sha").and_then(|sha| sha.as_str()) {
                let res = gh_api(&format!("https://api.github.com/repos/bjorn3/cargo-bisect-rustc-bot-jobs/git/commits/{}", head_sha)).await?;
                println!("{}", res);
                let json: serde_json::Value = serde_json::from_str(&res)?;
                let json = json.as_object().ok_or_else(|| "not json")?;
                let msg = json["message"].as_str().unwrap();
                crate::ReplyTo::from_commit_message(msg).map_err(|()| format!("Failed to parse commit message {:?}", msg))?
            } else {
                return Ok(Response::new("missing head_sha".into()));
            };
            println!("reply to: {:?}", reply_to);
            if let Some(html_url) = check_run.get("html_url").and_then(|url| url.as_str()) {
                let body = format!("bisect result: {}", html_url);
                reply_to.comment(&body).await?;
            } else {
                println!("no check run id: {:#?}", json);
            }
        }
        _ => {
            println!("{:#?}", json);
        }
    }
    Ok(Response::new("processed".into()))
}

async fn gh_api(url: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    println!("GET {}", url);
    let res = reqwest::Client::new()
        .get(url)
        .header(hyper::http::header::USER_AGENT, hyper::http::HeaderValue::from_str("https://github.com/bjorn3/cargo-bisect-rustc-bot").unwrap())
        .header(hyper::http::header::ACCEPT, hyper::http::HeaderValue::from_str("application/vnd.github.antiope-preview+json").unwrap())
        .basic_auth(crate::GITHUB_USERNAME, Some(crate::GITHUB_TOKEN))
        .send()
        .await?;
    println!("{}", res.status());
    Ok(res.text().await?)
}

async fn gh_api_post(url: &str, body: String) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    println!("POST {} <- {}", url, body);
    let res = reqwest::Client::new()
        .post(url)
        .header(hyper::http::header::USER_AGENT, hyper::http::HeaderValue::from_str("https://github.com/bjorn3/cargo-bisect-rustc-bot").unwrap())
        .header(hyper::http::header::ACCEPT, hyper::http::HeaderValue::from_str("application/vnd.github.v3.html+json").unwrap())
        .header(hyper::http::header::CONTENT_TYPE, hyper::http::HeaderValue::from_str("text/json").unwrap())
        .basic_auth(crate::GITHUB_USERNAME, Some(crate::GITHUB_TOKEN))
        .body(body)
        .send()
        .await?;
    println!("{}", res.status());
    Ok(res.text().await?)
}

pub(crate) async fn gh_post_comment(repo: &str, issue_number: u64, body: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("on issue {} post comment {:?}", issue_number, body);
    let res = gh_api_post(
        &format!("https://api.github.com/repos/{}/issues/{}/comments", repo, issue_number),
        format!(r#"{{"body": {:?}}}"#, body),
    ).await?;
    println!("on issue {} post comment result {:?}", issue_number, res);
    Ok(())
}
