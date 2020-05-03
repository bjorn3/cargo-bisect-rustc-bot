use std::convert::Infallible;
use hyper::{Body, Request, Response, Server};
use hyper::service::{make_service_fn, service_fn};

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
        Ok::<_, Infallible>(service_fn(web_hook))
    });

    let server = Server::bind(&addr).serve(make_svc);

    // Run this server for... forever!
    if let Err(e) = server.await {
        eprintln!("server error: {}", e);
    }
}

async fn web_hook(req: Request<Body>) -> Result<Response<Body>, Box<dyn std::error::Error + Send + Sync>> {
    let body: hyper::body::Bytes = hyper::body::to_bytes(req.into_body()).await?;
    let body = std::str::from_utf8(&*body)?;
    let json: serde_json::Value = serde_json::from_str(body)?;
    let json = json.as_object().ok_or_else(|| "not json")?;

    let repo = json.get("repository").and_then(|repo| repo.as_object()?.get("full_name")?.as_str());
    if repo != Some("bjorn3/cargo-bisect-rustc-bot") {
        println!("wrong repo {:?}", json);
        return Ok(Response::new("wrong repo".into()));
    }

    let sender = if let Some(sender) = json.get("sender").and_then(|sender| sender.as_object()?.get("login")?.as_str()) {
        sender
    } else {
        println!("no sender {:?}", json);
        return Ok(Response::new("no sender".into()));
    };

    match (json.get("comment"), json.get("action").and_then(|action| action.as_str())) {
        (Some(comment), Some("created")) => {
        if let Some(comment_body) = comment.as_object().and_then(|comment| comment.get("body")?.as_str()) {
                println!("{:?} commented \"{}\"", sender, comment_body);
            } else {
                println!("no comment body: {:#?}", json);
            }
        }
        _ => {
            println!("{:#?}", json);
        }
    }
    Ok(Response::new("processed".into()))
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

fn push_job(job_id: usize, start: &str, end: &str) {
    cmd!(git "branch" "-d" (format!("job{}", job_id)));
    cmd!(git "checkout" "--orphan" (format!("job{}", job_id)));
    std::fs::remove_dir_all("push-job/.github").unwrap();
    std::fs::create_dir_all("push-job/.github/workflows").unwrap();
    std::fs::remove_dir_all("push-job/src").unwrap();
    std::fs::create_dir("push-job/src").unwrap();
    std::fs::copy("./regression.rs", "push-job/src/lib.rs").unwrap();
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
      run: cargo bisect-rustc --start={} --end={}
        "#,
        start, end
    )).unwrap();
    cmd!(git "add" ".");
    cmd!(git "commit" "-m" (format!("Bisect job {}", job_id)));
    cmd!(git "push" "origin" (format!("job{}", job_id)) "--force");
}
