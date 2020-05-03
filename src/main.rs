use std::convert::Infallible;
use hyper::{Body, Request, Response, Server};
use hyper::service::{make_service_fn, service_fn};

const BOT_NAME: &'static str = "bisect-bot ";
const TOKEN: &'static str = env!("TOKEN", "gh personal access token not defined");

#[tokio::main]
async fn main() {
    let addr = (
        [0, 0, 0, 0],
        std::env::var("PORT")
            .unwrap_or("3000".to_string())
            .parse::<u16>()
            .unwrap(),
    )
        .into();

    let make_svc = make_service_fn(|_conn| async {
        Ok::<_, Infallible>(service_fn(request_handler))
    });

    let server = Server::bind(&addr).serve(make_svc);

    // Run this server for... forever!
    if let Err(e) = server.await {
        eprintln!("server error: {}", e);
    }
}

async fn request_handler(req: Request<Body>) -> Result<Response<Body>, Box<dyn std::error::Error + Send + Sync>> {
    web_hook(req).await.map_err(|err| {
        println!("error: {}", err);
        err
    })
}

async fn web_hook(req: Request<Body>) -> Result<Response<Body>, Box<dyn std::error::Error + Send + Sync>> {
    let body: hyper::body::Bytes = hyper::body::to_bytes(req.into_body()).await?;
    let body = std::str::from_utf8(&*body)?;
    let json: serde_json::Value = serde_json::from_str(body)?;
    let json = json.as_object().ok_or_else(|| "not json")?;

    let repo = json.get("repository").and_then(|repo| repo.as_object()?.get("full_name")?.as_str());
    if repo != Some("bjorn3/cargo-bisect-rustc-bot") && repo != Some("bjorn3/cargo-bisect-rustc-bot-jobs") {
        println!("wrong repo {:?}", json);
        return Ok(Response::new("wrong repo".into()));
    }

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
                parse_comment(repo.unwrap(), issue_number, comment_id, comment_body).await?;
            } else {
                println!("no comment body: {:#?}", json);
            }
        }
        (None, None, Some(check_run), Some("completed")) => {
            let issue_number = if let Some(head_sha) = check_run.get("head_sha").and_then(|sha| sha.as_str()) {
                let res = gh_api(&format!("https://api.github.com/repos/bjorn3/cargo-bisect-rustc-bot-jobs/git/commits/{}", head_sha)).await?;
                println!("{}", res);
                let json: serde_json::Value = serde_json::from_str(&res)?;
                let json = json.as_object().ok_or_else(|| "not json")?;
                json["message"].as_str().unwrap().split("#").nth(1).unwrap().split(")").nth(0).unwrap().parse::<u64>().unwrap()
            } else {
                return Ok(Response::new("missing head_sha".into()));
            };
            println!("issue number: {}", issue_number);
            if let Some(html_url) = check_run.get("html_url").and_then(|url| url.as_str()) {
                gh_post_comment(issue_number, &format!("bisect result: {}", html_url)).await?;
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
        .basic_auth("bjorn3", Some(TOKEN))
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
        .basic_auth("bjorn3", Some(TOKEN))
        .body(body)
        .send()
        .await?;
    println!("{}", res.status());
    Ok(res.text().await?)
}

async fn gh_post_comment(issue_number: u64, body: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("on issue {} post comment {:?}", issue_number, body);
    let res = gh_api_post(
        &format!("https://api.github.com/repos/bjorn3/cargo-bisect-rustc-bot/issues/{}/comments", issue_number),
        format!(r#"{{"body": {:?}}}"#, body),
    ).await?;
    println!("on issue {} post comment result {:?}", issue_number, res);
    Ok(())
}

async fn parse_comment(repo: &str, issue_number: u64, comment_id: u64, comment: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut lines = comment.lines();
    while let Some(line) = lines.next() {
        let line = line.trim();
        if !line.starts_with(BOT_NAME) {
            continue;
        }
        let line = line[BOT_NAME.len()..].trim();
        let mut parts = line.split(" ").map(|part| part.trim());

        match parts.next() {
            Some("bisect") => {
                let mut cmds = vec![];
                for part in parts {
                    if part.starts_with("start=") {
                        cmds.push(format!("--start={}", &part["start=".len()..]));
                    } else if part.starts_with("end=") {
                        cmds.push(format!("--end={}", &part["end=".len()..]));
                    } else {
                        println!("unknown command part {:?}", part);
                        return Ok(());
                    }
                }
                loop {
                    match lines.next() {
                        Some(line) if line.trim() == "```rust" => break,
                        Some(_) => {}
                        None => {
                            println!("didn't find repro code");
                            return Ok(());
                        }
                    }
                }
                let repro = lines.take_while(|line| line.trim() != "```").collect::<Vec<_>>().join("\n");
                // --start={} --end={}
                println!("{:?}", &cmds);
                push_job(repo, issue_number, comment_id, &cmds, &repro);
                gh_post_comment(issue_number, "started bisection").await?;
            }
            cmd => {
                println!("unknown command {:?}", cmd);
                return Ok(());
            }
        }

        return Ok(());
    }

    return Ok(());
}

macro_rules! cmd {
    ($cmd:ident $($arg:tt)*) => {
        #[allow(unused_parens)]
        let res = std::process::Command::new(stringify!($cmd))
            .current_dir("push-job")
            $(.arg($arg))*
            .spawn()
            .unwrap()
            .wait()
            .unwrap();
        assert!(res.success());
    };
}

fn push_job(repo: &str, issue_number: u64, job_id: u64, bisect_cmds: &[String], repro: &str) {
    // Escape commands and join with whitespace
    let bisect_cmds = bisect_cmds.iter().map(|cmd| format!("{:?}", cmd)).collect::<Vec<_>>().join(" ");

    let _ = std::process::Command::new("git")
            .current_dir("push-job")
            .arg("branch")
            .arg("-d")
            .arg("--force")
            .arg(format!("job{}", job_id))
            .spawn()
            .unwrap()
            .wait()
            .unwrap();
    cmd!(git "checkout" "--orphan" (format!("job{}", job_id)));
    std::fs::remove_dir_all("push-job/.github").unwrap();
    std::fs::create_dir_all("push-job/.github/workflows").unwrap();
    std::fs::remove_dir_all("push-job/src").unwrap();
    std::fs::create_dir("push-job/src").unwrap();
    std::fs::write("push-job/src/lib.rs", repro).unwrap();
    std::fs::write("push-job/.github/workflows/bisect.yaml", format!(
        r#"
name: Bisect

on:
  - push

jobs:
  build:
    runs-on: ubuntu-latest

    steps:
    - uses: actions/checkout@v2

    # https://github.com/actions/cache/issues/133
    - name: Fixup owner of ~/.cargo/
      # Don't remove the trailing /. It is necessary to follow the symlink.
      run: sudo chown -R $(whoami):$(id -ng) ~/.cargo/

    - name: Cache cargo installed crates
      uses: actions/cache@v1.1.2
      with:
        path: ~/.cargo/bin
        key: cargo-installed-crates

    - run: cargo install cargo-bisect-rustc || true

    - name: Bisect
      run: cargo bisect-rustc {} | grep -v "for x86_64-unknown-linux-gnu" || true
        "#,
        bisect_cmds,
    )).unwrap();
    cmd!(git "add" ".");
    cmd!(git "commit" "-m" (format!("Bisect job for comment id {} ({}#{})", job_id, repo, issue_number)));
    cmd!(git "push" "origin" (format!("job{}", job_id)) "--force");
}
