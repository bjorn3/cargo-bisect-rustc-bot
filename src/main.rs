use std::convert::Infallible;
use hyper::{Body, Request, Response, Server};
use hyper::service::{make_service_fn, service_fn};

mod github;
mod zulip;

const BOT_NAME: &'static str = "bisect-bot ";
const GITHUB_USERNAME: &'static str = env!("GITHUB_USERNAME", "github username not defined");
const GITHUB_TOKEN: &'static str = env!("GITHUB_TOKEN", "github personal access token not defined");
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
    crate::github::web_hook(req).await.map_err(|err| {
        println!("error: {}", err);
        err
    })
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
                crate::github::gh_post_comment(repo, issue_number, body).await?;
                Ok(())
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

async fn parse_comment(reply_to: &ReplyTo, comment_id: u64, comment: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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
            push_job(&reply_to, comment_id, &cmds, &code).await?;
        }
        None => {}
    }

    Ok(())
}

async fn push_job(reply_to: &ReplyTo, job_id: u64, bisect_cmds: &[String], repro: &str) -> reqwest::Result<()> {
    // Escape commands and join with whitespace
    let bisect_cmds = bisect_cmds.iter().map(|cmd| format!("{:?}", cmd)).collect::<Vec<_>>().join(" ");

    let src_lib = create_blob(repro).await?;
    let src = create_tree(&[TreeEntry {
        path: "lib.rs".to_string(),
        mode: TreeEntryMode::File,
        type_: TreeEntryType::Blob,
        sha: src_lib,
    }]).await?;

    let github_workflow_bisect = create_blob(&format!(
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
    )).await?;
    let github_workflow = create_tree(&[TreeEntry {
        path: "bisect.yaml".to_string(),
        mode: TreeEntryMode::File,
        type_: TreeEntryType::Blob,
        sha: github_workflow_bisect,
    }]).await?;
    let github = create_tree(&[TreeEntry {
        path: "workflows".to_string(),
        mode: TreeEntryMode::Subdirectory,
        type_: TreeEntryType::Tree,
        sha: github_workflow,
    }]).await?;

    let cargo = create_blob(r#"[package]
name = "cargo-bisect-bot-job"
version = "0.0.0"
edition = "2018"
publish = false

[dependencies]
    "#).await?;

    let root = create_tree(&[
        TreeEntry {
            path: "src".to_string(),
            mode: TreeEntryMode::Subdirectory,
            type_: TreeEntryType::Tree,
            sha: src,
        },
        TreeEntry {
            path: ".github".to_string(),
            mode: TreeEntryMode::Subdirectory,
            type_: TreeEntryType::Tree,
            sha: github,
        },
        TreeEntry {
            path: "Cargo.toml".to_string(),
            mode: TreeEntryMode::File,
            type_: TreeEntryType::Blob,
            sha: cargo,
        }
    ]).await?;

    let commit = create_commit(
        &format!("Bisect job for comment id {}\n\n{}", job_id, reply_to.to_commit_header()),
        &root,
        &[],
    ).await?;

    push_branch(&format!("job{}", job_id), &commit).await?;

    Ok(())
}

async fn create_blob(content: &str) -> reqwest::Result<String> {
    let res = crate::github::gh_api_post("https://api.github.com/repos/bjorn3/cargo-bisect-rustc-bot-jobs/git/blobs", serde_json::to_string(&serde_json::json!({
        "content": content,
        "encoding": "utf-8",
    })).unwrap()).await?;
    println!("create blob: {}", res);
    let res: serde_json::Value = serde_json::from_str(&res).unwrap();
    Ok(res["sha"].as_str().unwrap().to_string())
}

async fn create_tree(content: &[TreeEntry]) -> reqwest::Result<String> {
    let res = crate::github::gh_api_post("https://api.github.com/repos/bjorn3/cargo-bisect-rustc-bot-jobs/git/trees", serde_json::to_string(&serde_json::json!({
        "tree": content,
    })).unwrap()).await?;
    println!("create tree: {}", res);
    let res: serde_json::Value = serde_json::from_str(&res).unwrap();
    Ok(res["sha"].as_str().unwrap().to_string())
}

#[derive(serde::Serialize)]
struct TreeEntry {
    path: String,
    mode: TreeEntryMode,
    #[serde(rename = "type")]
    type_: TreeEntryType,
    sha: String,
}

#[derive(serde::Serialize)]
enum TreeEntryMode {
    #[serde(rename = "100644")]
    File,
    #[serde(rename = "100755")]
    Executable,
    #[serde(rename = "040000")]
    Subdirectory,
    #[serde(rename = "160000")]
    Submodule,
    #[serde(rename = "120000")]
    Symlink,
}

#[derive(serde::Serialize)]
enum TreeEntryType {
    #[serde(rename = "blob")]
    Blob,
    #[serde(rename = "tree")]
    Tree,
    #[serde(rename = "commit")]
    Commit,
}

async fn create_commit(message: &str, tree: &str, parents: &[&str]) -> reqwest::Result<String> {
    let res = crate::github::gh_api_post("https://api.github.com/repos/bjorn3/cargo-bisect-rustc-bot-jobs/git/commits", serde_json::to_string(&serde_json::json!({
        "message": message,
        "tree": tree,
        "parents": parents,
    })).unwrap()).await?;
    println!("create commit: {}", res);
    let res: serde_json::Value = serde_json::from_str(&res).unwrap();
    Ok(res["sha"].as_str().unwrap().to_string())
}

async fn push_branch(branch: &str, commit: &str) -> reqwest::Result<()> {
    let res = crate::github::gh_api_post("https://api.github.com/repos/bjorn3/cargo-bisect-rustc-bot-jobs/git/refs", serde_json::to_string(&serde_json::json!({
        "ref": format!("refs/heads/{}", branch),
        "sha": commit,
    })).unwrap()).await?;
    println!("push branch: {}", res);
    Ok(())
}
