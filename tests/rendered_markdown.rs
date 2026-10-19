//! End-to-end check for rendered Markdown review: runs the real binary in a
//! tmux pane, comments on a rendered paragraph, quits, reads the comment back
//! through `tuicr review list` / `tuicr review comments`, and reopens the file
//! in rendered and source view. Covers the `main.rs` config wiring and the
//! on-disk session store, which unit tests cannot reach. Skipped when tmux is
//! not installed.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant};

const SPEC: &str = "# Payment retries\n\
\n\
Retries use **exponential backoff**\n\
and a `max_attempts` cap;\n\
each retry is logged.\n\
\n\
| Setting | Default |\n\
|---|---|\n\
| max_attempts | 5 |\n";

fn tmux_available() -> bool {
    Command::new("tmux")
        .arg("-V")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn tmux(socket: &Path, args: &[&str]) -> String {
    let out = Command::new("tmux")
        .arg("-S")
        .arg(socket)
        .args(args)
        .output()
        .expect("run tmux");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// An isolated HOME, config, and data dir, so sessions never touch the user's.
struct Sandbox {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    socket: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let home = root.join("home");
        std::fs::create_dir_all(home.join(".config/tuicr")).expect("config dir");
        std::fs::write(root.join("spec.md"), SPEC).expect("write spec");
        let socket = root.join("tmux.sock");
        Self {
            _dir: dir,
            root,
            home,
            socket,
        }
    }

    fn write_config(&self, extra: &str) {
        std::fs::write(
            self.home.join(".config/tuicr/config.toml"),
            format!("username = \"e2e\"\n{extra}"),
        )
        .expect("write config");
    }

    fn env(&self) -> [(&'static str, PathBuf); 3] {
        [
            ("HOME", self.home.clone()),
            ("XDG_CONFIG_HOME", self.home.join(".config")),
            ("XDG_DATA_HOME", self.home.join(".local/share")),
        ]
    }

    /// Start the TUI on the spec in a fresh tmux server.
    fn open(&self, args: &str) {
        let env: String = self
            .env()
            .iter()
            .map(|(k, v)| format!("{k}={} ", v.display()))
            .collect();
        let command = format!(
            "cd {root} && env {env}{bin} --no-update-check --file spec.md {args}",
            root = self.root.display(),
            bin = env!("CARGO_BIN_EXE_tuicr"),
        );
        tmux(
            &self.socket,
            &["new-session", "-d", "-x", "100", "-y", "30", &command],
        );
    }

    fn keys(&self, keys: &[&str]) {
        let mut args = vec!["send-keys"];
        args.extend_from_slice(keys);
        tmux(&self.socket, &args);
    }

    fn literal(&self, text: &str) {
        tmux(&self.socket, &["send-keys", "-l", text]);
    }

    /// Wait until the pane shows `needle`, returning the screen either way.
    fn wait_for(&self, needle: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut screen = String::new();
        while Instant::now() < deadline {
            screen = tmux(&self.socket, &["capture-pane", "-p"]);
            if screen.contains(needle) {
                return screen;
            }
            sleep(Duration::from_millis(100));
        }
        screen
    }

    /// Quit the TUI and wait for the pane's process to exit.
    fn quit(&self) {
        self.keys(&["Escape"]);
        self.keys(&[":q", "Enter"]);
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && !tmux(&self.socket, &["list-sessions"]).trim().is_empty()
        {
            sleep(Duration::from_millis(100));
        }
        tmux(&self.socket, &["kill-server"]);
    }

    /// Run a `tuicr review` subcommand and parse its JSON.
    fn review(&self, args: &[&str]) -> serde_json::Value {
        let out = Command::new(env!("CARGO_BIN_EXE_tuicr"))
            .arg("review")
            .args(args)
            .current_dir(&self.root)
            .envs(self.env())
            .output()
            .expect("run tuicr review");
        assert!(
            out.status.success(),
            "tuicr review {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).expect("review output is JSON")
    }
}

#[test]
fn should_persist_a_rendered_comment_and_export_its_source_range() {
    if !tmux_available() {
        eprintln!("skipping: tmux not installed");
        return;
    }
    let sandbox = Sandbox::new();

    // 1. Open in rendered view on line 4, the middle of the 3-5 paragraph,
    //    and comment on the block under the cursor.
    sandbox.write_config("markdown_view = \"rendered\"\n");
    sandbox.open("--line 4");
    let screen = sandbox.wait_for("MARKDOWN · L3–5");
    assert!(
        screen.contains("rendered Markdown · lines 1–9") && !screen.contains("**"),
        "opens rendered with the cursor on the paragraph:\n{screen}"
    );
    sandbox.keys(&["c"]);
    sandbox.wait_for("COMMENT");
    sandbox.literal("Say which errors are retryable.");
    sandbox.keys(&["C-s"]);
    let screen = sandbox.wait_for("Comment added to lines 3-5");
    assert!(
        screen.contains("Comment added to lines 3-5"),
        "saved against the source range:\n{screen}"
    );
    sandbox.quit();

    // 2. The session store and the review CLI see the source range, in the
    //    existing JSON shape.
    let sessions = sandbox.review(&["list", "--all"]);
    let sessions = sessions.as_array().expect("session list");
    assert_eq!(sessions.len(), 1, "{sessions:#?}");
    assert_eq!(sessions[0]["comment_count"], 1);
    let path = sessions[0]["path"].as_str().expect("session path");
    let comments = sandbox.review(&["comments", "--session", path]);
    let comments = comments.as_array().expect("comment list");
    assert_eq!(comments.len(), 1, "{comments:#?}");
    let comment = comments[0].as_object().expect("comment object");
    let mut keys: Vec<&str> = comment.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "author",
            "comment_type",
            "content",
            "created_at",
            "end_line",
            "id",
            "lifecycle_state",
            "location",
            "path",
            "side",
            "start_line",
        ]
    );
    assert_eq!(comment["location"], "spec.md:3-5");
    assert_eq!(comment["start_line"], 3);
    assert_eq!(comment["end_line"], 5);
    assert_eq!(comment["side"], "new");
    assert_eq!(comment["author"], "e2e");
    assert_eq!(comment["content"], "Say which errors are retryable.");

    // 3. Reopen in source view: the same comment, on lines 3-5.
    sandbox.write_config("");
    sandbox.open("");
    let screen = sandbox.wait_for("Say which errors are retryable.");
    assert!(
        screen.contains("L3-L5") && screen.contains("**exponential backoff**"),
        "source view shows the stored range:\n{screen}"
    );
    // 4. And again in rendered view, switched at runtime.
    sandbox.keys(&[":markdown", "Enter"]);
    let screen = sandbox.wait_for("rendered Markdown · lines 1–9");
    assert!(
        screen.contains("L3-L5")
            && screen.contains("Say which errors are retryable.")
            && !screen.contains("**"),
        "rendered view shows the same comment:\n{screen}"
    );
    sandbox.quit();
}
