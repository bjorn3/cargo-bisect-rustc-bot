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

fn main() {
    let job_id = std::env::args().nth(1).unwrap();
    let start = std::env::args().nth(2).unwrap();
    let end = std::env::args().nth(3).unwrap();

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
