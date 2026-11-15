//! Runs the built binary against a throwaway data folder: import the legacy fixture, read it back,
//! record from the fake capture, and process the recording with the fake engines.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const DONE: &str = "rec_1731234567890_k3j2h1x";

struct Env {
    root: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        Self {
            root: tempfile::tempdir().unwrap(),
        }
    }

    fn data(&self) -> PathBuf {
        self.root.path().join("data")
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_dory-cli"));
        cmd.args(args).env("DORY_DATA_DIR", self.data());
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    /// A writable copy of the legacy fixture.
    fn legacy(&self) -> PathBuf {
        let from = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/dory-store/tests/fixtures/legacy-data");
        let to = self.root.path().join("legacy");
        copy_dir(&from, &to);
        to
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let dest = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn imports_lists_and_shows_the_legacy_folder() {
    let env = Env::new();
    let legacy = env.legacy();
    let legacy_arg = legacy.to_str().unwrap();

    let first = env.ok(&["import", legacy_arg]);
    assert!(
        first.contains("6 new, 0 updated, 0 unchanged, 2 skipped"),
        "{first}"
    );
    let second = env.ok(&["import", legacy_arg]);
    assert!(
        second.contains("0 new, 0 updated, 6 unchanged, 2 skipped"),
        "{second}"
    );

    let list = env.ok(&["list"]);
    assert_eq!(list.lines().count(), 6, "{list}");
    let done = list.lines().find(|l| l.starts_with(DONE)).unwrap();
    assert!(done.contains("Weekly sync"), "{done}");
    assert!(done.contains("30:32"), "{done}");
    assert!(done.ends_with("done"), "{done}");

    let found = env.ok(&["list", "--search", "roadmap"]);
    assert_eq!(found.lines().count(), 1, "{found}");
    assert!(found.starts_with(DONE));

    let shown = env.ok(&["show", DONE]);
    for want in [
        "Weekly sync – kế hoạch quý 4",
        "Chào mọi người, chúng ta bắt đầu nhé.",
        "Okay, I'll share my screen.",
        "## Weekly sync",
        "- Action: share the deck by Friday",
    ] {
        assert!(shown.contains(want), "missing {want:?} in\n{shown}");
    }
    assert!(
        !shown.contains("[0:"),
        "imported transcripts carry no times:\n{shown}"
    );

    let missing = env.run(&["show", "rec_nope"]);
    assert!(!missing.status.success());
    assert!(text(&missing.stderr).contains("no recording rec_nope"));
}

#[test]
fn records_from_the_fake_capture_then_processes_with_fake_engines() {
    let env = Env::new();

    // Stdin stays open, so the recording ends by itself after three seconds of audio.
    let mut child = env
        .cmd(&["record", "--fake-capture", "3"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take();
    let out = child.wait_with_output().unwrap();
    drop(stdin);
    let stdout = text(&out.stdout);
    assert!(out.status.success(), "{stdout}\n{}", text(&out.stderr));
    let id = stdout
        .lines()
        .find_map(|l| l.strip_prefix("recorded "))
        .and_then(|l| l.split(' ').next())
        .unwrap_or_else(|| panic!("no recorded line in\n{stdout}"))
        .to_owned();
    assert!(stdout.contains("sealed Flac 48000 Hz 3.0 s"), "{stdout}");
    assert!(
        env.data()
            .join("takes")
            .join(format!("{id}.flac"))
            .is_file()
    );

    let list = env.ok(&["list"]);
    let line = list.lines().find(|l| l.starts_with(&id)).unwrap();
    assert!(line.contains("0:03"), "{line}");
    // Each CLI run reopens the library, which re-checks blocked jobs, so the job may be
    // queued again for a moment before it blocks on the missing engine.
    assert!(
        line.ends_with("blocked at transcribe") || line.ends_with("queued"),
        "{line}"
    );

    let blocked = env.run(&["process", &id]);
    assert!(!blocked.status.success());
    let why = text(&blocked.stderr);
    assert!(why.contains("blocked at transcribe"), "{why}");
    assert!(why.contains("fix: "), "{why}");

    let done = env.ok(&["process", &id, "--fake-engines"]);
    assert!(done.contains("done"), "{done}");

    let shown = env.ok(&["show", &id]);
    for want in [
        "status    done",
        "[0:00] Me: Hello from the fake microphone.",
        "[0:01] Speaker 1: Hello from the fake system audio.",
        "Summary (fake/llm)",
        "## Fake summary",
    ] {
        assert!(shown.contains(want), "missing {want:?} in\n{shown}");
    }
    assert!(
        env.ok(&["list"])
            .lines()
            .any(|l| l.starts_with(&id) && l.ends_with("done"))
    );
}

#[test]
fn models_lists_each_kind_and_the_fakes_when_asked() {
    let env = Env::new();
    let plain = env.ok(&["models"]);
    assert!(plain.contains("pending/speech-engine"), "{plain}");
    assert!(plain.contains("(none registered)"), "{plain}");
    let fake = env.ok(&["models", "--fake-engines"]);
    assert!(
        fake.contains("fake/stt") && fake.contains("fake/llm"),
        "{fake}"
    );
}

#[test]
fn bad_input_prints_usage_and_fails() {
    let env = Env::new();
    for args in [
        &[][..],
        &["nonsense"],
        &["show"],
        &["record", "--layout", "both"],
    ] {
        let out = env.run(args);
        assert!(!out.status.success(), "{args:?}");
        assert!(!out.stderr.is_empty(), "{args:?}");
    }
}
