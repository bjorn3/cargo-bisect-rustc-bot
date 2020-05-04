use std::convert::Infallible;
use hyper::{Body, Request, Response, Server};
use hyper::service::{make_service_fn, service_fn};

mod zulip;

const BOT_NAME: &'static str = "bisect-bot ";
const TOKEN: &'static str = env!("TOKEN", "gh personal access token not defined");
const ZULIP_USER: &'static str = env!("ZULIP_USERNAME", "zulip username not defined");
const ZULIP_TOKEN: &'static str = env!("ZULIP_TOKEN", "zulip api token not defined");

#[tokio::main]
async fn main() {
    let _zulip = tokio::spawn(crate::zulip::zulip_task());

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
                parse_comment(repo, issue_number, comment_id, comment_body).await?;
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
                ReplyTo::from_commit_message(msg).map_err(|()| format!("Failed to parse commit message {:?}", msg))?
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

#[derive(Debug)]
enum ReplyTo {
    Github {
        repo: String,
        issue_number: u64,
    },
    ZulipPrivate {
        user_id: u64,
    },
}

impl ReplyTo {
    async fn comment(&self, body: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match *self {
            ReplyTo::Github { ref repo, issue_number } => {
                gh_post_comment(issue_number, body).await
            }
            ReplyTo::ZulipPrivate { user_id } => {
                crate::zulip::zulip_post_message(user_id, body).await
            }
        }
    }

    const COMMIT_HEADER: &'static str = "X-Bisectbot-Reply-To";

    fn to_commit_header(&self) -> String {
        match *self {
            ReplyTo::Github { ref repo, issue_number } => {
                format!("{}: github {}#{}", Self::COMMIT_HEADER, repo, issue_number)
            }
            ReplyTo::ZulipPrivate { user_id } => {
                format!("{}: zulip-private {}", Self::COMMIT_HEADER, user_id)
            }
        }
    }

    fn from_commit_message(message: &str) -> Result<Self, ()> {
        for line in message.lines() {
            let line = line.trim();
            if !line.starts_with(Self::COMMIT_HEADER) {
                continue;
            }
            let header = line[Self::COMMIT_HEADER.len()+1..].trim();
            let mut split = header.split(" ");
            let kind = split.next().ok_or(())?.trim();
            let to = split.next().ok_or(())?.trim();
            if split.next().is_some() {
                return Err(());
            }
            match kind {
                "github" => {
                    let mut split = to.split("#");
                    let repo = split.next().ok_or(())?.trim();
                    let issue_number = split.next().ok_or(())?.trim().parse().map_err(|_| ())?;
                    if split.next().is_some() {
                        return Err(());
                    }
                    return Ok(ReplyTo::Github {
                        repo: repo.to_string(),
                        issue_number,
                    });
                }
                "zulip-private" => {
                    let user_id = to.parse().map_err(|_| ())?;
                    return Ok(ReplyTo::ZulipPrivate {
                        user_id,
                    });
                }
                _ => return Err(()),
            }
        }

        Err(())
    }
}

enum Command {
    Bisect {
        start: Option<String>,
        end: String,
        code: String,
    },
}

impl Command {
    fn parse_comment(comment: &str) -> Result<Option<Command>, String> {
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
                    let mut start = None;
                    let mut end = None;
                    for part in parts {
                        if part.starts_with("start=") {
                            if start.is_some() {
                                return Err(format!("start range specified twice"));
                            }
                            start = Some(part["start=".len()..].to_string());
                        } else if part.starts_with("end=") {
                            if end.is_some() {
                                return Err(format!("end range specified twice"));
                            }
                            end = Some(part["end=".len()..].to_string());
                        } else {
                            return Err(format!("unknown command part {:?}", part));
                        }
                    }
                    let end = end.ok_or("missing end range")?;
                    loop {
                        match lines.next() {
                            Some(line) if line.trim() == "```rust" => break,
                            Some(_) => {}
                            None => {
                                return Err("didn't find repro code".to_string());
                            }
                        }
                    }
                    let code = lines.take_while(|line| line.trim() != "```").collect::<Vec<_>>().join("\n");
                    return Ok(Some(Command::Bisect {
                        start,
                        end,
                        code,
                    }));

                }
                cmd => {
                    return Err(format!("unknown command {:?}", cmd));
                }
            }
        }

        return Ok(None);
    }
}

async fn parse_comment(repo: &str, issue_number: u64, comment_id: u64, comment: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match Command::parse_comment(comment)? {
        Some(Command::Bisect {
            start,
            end,
            code,
        }) => {
            let mut cmds = Vec::new();
            if let Some(start) = start {
                cmds.push(format!("--start={}", start));
            }
            cmds.push(format!("--end={}", end));
            println!("{:?}", &cmds);
            let reply_to = if repo == "zulip" {
                ReplyTo::ZulipPrivate { user_id: issue_number }
            } else {
                ReplyTo::Github { repo: repo.to_string(), issue_number }
            };
            push_job(&reply_to, comment_id, &cmds, &code);
            reply_to.comment("started bisection").await?;
        }
        None => {}
    }

    Ok(())
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

fn push_job(reply_to: &ReplyTo, job_id: u64, bisect_cmds: &[String], repro: &str) {
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
      run: cargo bisect-rustc {} --access=github | grep -v "for x86_64-unknown-linux-gnu" || true
        "#,
        bisect_cmds,
    )).unwrap();
    cmd!(git "add" ".");
    cmd!(git "commit" "-m" (format!("Bisect job for comment id {}\n\n{}", job_id, reply_to.to_commit_header())));
    cmd!(git "push" "origin" (format!("job{}", job_id)) "--force");
}
