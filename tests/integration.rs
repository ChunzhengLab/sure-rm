use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

struct TestEnv {
    root: PathBuf,
}

impl TestEnv {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("sure-rm-it-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn cmd(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_sure-rm"));
        cmd.env("SURE_RM_ROOT", &self.root);
        cmd
    }

    fn file(&self, rel: &str) -> PathBuf {
        let path = self.root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "content").unwrap();
        path
    }

    fn missing_path(&self) -> PathBuf {
        self.root.join("missing.txt")
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

// ── 1. delete -> list -> restore round-trip ──

#[test]
fn delete_list_restore_round_trip() {
    let env = TestEnv::new("round-trip");
    let file = env.file("work/hello.txt");

    // delete
    let out = env
        .cmd()
        .args(["-v", file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "delete failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!file.exists());

    // list
    let out = env.cmd().arg("list").output().unwrap();
    assert!(out.status.success());
    let list_output = String::from_utf8_lossy(&out.stdout);
    assert!(
        list_output.contains("hello.txt"),
        "list should show the trashed file"
    );

    // extract id from list output (first column)
    let id = list_output
        .lines()
        .next()
        .unwrap()
        .split('\t')
        .next()
        .unwrap();

    // restore
    let out = env.cmd().args(["restore", id]).output().unwrap();
    assert!(
        out.status.success(),
        "restore failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(file.exists());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "content");
}

// ── 2. purge by id and purge --all ──

#[test]
fn purge_by_id_removes_entry() {
    let env = TestEnv::new("purge-id");
    let file = env.file("work/to_purge.txt");

    env.cmd().arg(file.to_str().unwrap()).output().unwrap();

    let list = env.cmd().arg("list").output().unwrap();
    let id = String::from_utf8_lossy(&list.stdout)
        .lines()
        .next()
        .unwrap()
        .split('\t')
        .next()
        .unwrap()
        .to_string();

    let out = env.cmd().args(["purge", &id]).output().unwrap();
    assert!(out.status.success());

    let list = env.cmd().arg("list").output().unwrap();
    assert!(String::from_utf8_lossy(&list.stdout).contains("empty"));
}

#[test]
fn purge_all_is_cancelled_by_no() {
    let env = TestEnv::new("purge-all");
    let file = env.file("work/keep.txt");

    env.cmd().arg(file.to_str().unwrap()).output().unwrap();

    let mut child = env
        .cmd()
        .args(["purge", "--all"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    child.stdin.take().unwrap().write_all(b"n\n").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("permanently delete"),
        "should show confirmation prompt: {stderr}"
    );

    let list = env.cmd().arg("list").output().unwrap();
    assert!(!String::from_utf8_lossy(&list.stdout).contains("empty"));
}

// ── 3. non-TTY output has no ANSI escapes ──

#[test]
fn non_tty_output_has_no_ansi() {
    let env = TestEnv::new("no-ansi");
    let file = env.file("work/ansi.txt");

    let out = env
        .cmd()
        .args(["-v", file.to_str().unwrap()])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stdout.contains("\x1b["),
        "stdout should not contain ANSI escapes"
    );
    assert!(
        !stderr.contains("\x1b["),
        "stderr should not contain ANSI escapes"
    );
}

// ── 4. error cases: exit codes and messages ──

#[test]
fn no_args_shows_help() {
    let out = Command::new(env!("CARGO_BIN_EXE_sure-rm"))
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Usage"), "should show usage text: {stdout}");
}

#[test]
fn dangerous_target_dot_is_rejected() {
    let env = TestEnv::new("danger-dot");

    let out = env.cmd().arg(".").output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("refusing"),
        "should mention refusing: {stderr}"
    );
}

#[test]
fn nonexistent_file_fails_without_force() {
    let env = TestEnv::new("noexist");
    let missing = env.missing_path();

    let out = env.cmd().arg(&missing).output().unwrap();
    assert!(!out.status.success());

    // with -f it should succeed silently
    let out = env.cmd().args(["-f"]).arg(&missing).output().unwrap();
    assert!(out.status.success());
}

#[test]
fn version_flag_prints_version() {
    let out = Command::new(env!("CARGO_BIN_EXE_sure-rm"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with("sure-rm "),
        "should print version: {stdout}"
    );
}
