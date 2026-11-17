//! Runs the built binary against a throwaway data folder: import the legacy fixture, read it back,
//! record from the fake capture, and process the recording with the fake engines.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

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

    let found = env.ok(&["search", "roadmap"]);
    assert_eq!(found.lines().count(), 1, "{found}");
    assert!(found.starts_with(DONE));
    assert!(found.trim_end().ends_with("text"), "{found}");
    let titled = env.ok(&["search", "ke", "hoach"]);
    assert!(titled.trim_end().ends_with("title"), "{titled}");

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

    let out = env
        .cmd(&["process", &id, "--fake-engines"])
        .env("DORY_MACHINE", "battery")
        .output()
        .unwrap();
    let done = text(&out.stdout);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(done.contains("done"), "{done}");
    assert!(
        done.contains("pace light, 2 threads, on battery"),
        "Automatic runs light on battery: {done}"
    );

    let shown = env.ok(&["show", &id]);
    for want in [
        "Fake tone",
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
    let fresh = env.run(&["models"]);
    assert!(!fresh.status.success(), "a reader never creates a library");
    assert!(
        String::from_utf8_lossy(&fresh.stderr).contains("open Dory once to create it"),
        "{}",
        String::from_utf8_lossy(&fresh.stderr)
    );
    assert!(!env.data().join("dory.sqlite").exists());
    env.ok(&["languages"]);
    let plain = env.ok(&["models"]);
    for id in [
        "sherpa/parakeet-tdt-0.6b-v3",
        "sherpa/whisper-turbo",
        "ollama/gemma4:e4b",
        "ollama/gemma4:12b",
    ] {
        assert!(plain.contains(id), "missing {id} in\n{plain}");
    }
    assert_eq!(
        plain.matches("(none registered)").count(),
        1,
        "only speakers has no engine yet:\n{plain}"
    );
    let about = env.ok(&["models", "--about"]);
    assert!(about.contains("No Tagalog"), "{about}");
    let fake = env.ok(&["models", "--fake-engines"]);
    assert!(
        fake.contains("fake/stt") && fake.contains("fake/llm"),
        "{fake}"
    );
}

#[test]
fn meeting_languages_steer_auto_away_from_models_without_them() {
    let env = Env::new();
    assert_eq!(env.ok(&["languages"]).trim(), "meeting languages: any");
    assert_eq!(
        env.ok(&["languages", "tl", "en"]).trim(),
        "meeting languages: tl, en"
    );
    let rows = env.ok(&["models"]);
    let parakeet = rows
        .lines()
        .find(|l| l.contains("sherpa/parakeet-tdt-0.6b-v3"))
        .unwrap();
    assert!(parakeet.contains("language"), "{parakeet}");
    assert_eq!(
        env.ok(&["languages", "--clear"]).trim(),
        "meeting languages: any"
    );
}

#[test]
fn choosing_a_model_pins_it_and_refuses_one_not_in_the_catalog() {
    let env = Env::new();
    env.ok(&["models", "choose", "stt", "sherpa/whisper-turbo"]);
    let chosen = env.ok(&["models"]);
    assert!(
        chosen
            .lines()
            .any(|l| l.starts_with('*') && l.contains("sherpa/whisper-turbo")),
        "{chosen}"
    );
    assert!(
        !env.run(&["models", "choose", "stt", "sherpa/nope"])
            .status
            .success()
    );
    assert!(
        !env.run(&["models", "choose", "video", "auto"])
            .status
            .success()
    );
    env.ok(&["models", "choose", "stt", "auto"]);
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

/// A `record` running in the background, with its output read line by line as it comes.
struct Recording {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    seen: Vec<String>,
}

impl Recording {
    fn start(env: &Env, args: &[&str]) -> Recording {
        let mut child = env
            .cmd(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let _ = tx.send(line);
            }
        });
        Recording {
            stdin: child.stdin.take(),
            child,
            lines,
            seen: Vec::new(),
        }
    }

    /// The first new line that `wanted` accepts.
    fn wait_for(&mut self, what: &str, wanted: impl Fn(&str) -> bool) -> String {
        let give_up = Instant::now() + Duration::from_secs(20);
        loop {
            let left = give_up.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) => {
                    self.seen.push(line.clone());
                    if wanted(&line) {
                        return line;
                    }
                }
                Err(_) => panic!("no line for {what} in\n{}", self.seen.join("\n")),
            }
        }
    }

    /// The id from the `recording <id>` line.
    fn wait_for_id(&mut self) -> String {
        let line = self.wait_for("the recording id", |l| {
            l.strip_prefix("recording ")
                .is_some_and(|rest| !rest.starts_with("to "))
        });
        line["recording ".len()..].to_owned()
    }

    fn type_line(&mut self, text: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{text}").unwrap();
        stdin.flush().unwrap();
    }

    fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// Waits for the exit and returns every stdout line. Kills the process if it outlives
    /// `limit`.
    fn finish(mut self, limit: Duration) -> (bool, Vec<String>, String) {
        let give_up = Instant::now() + limit;
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= give_up {
                let _ = self.child.kill();
                let _ = self.child.wait();
                panic!(
                    "record still running after {limit:?}:\n{}",
                    self.seen.join("\n")
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        self.seen.extend(self.lines.try_iter());
        let mut stderr = String::new();
        if let Some(mut err) = self.child.stderr.take() {
            std::io::Read::read_to_string(&mut err, &mut stderr).unwrap();
        }
        (status.success(), self.seen, stderr)
    }
}

#[test]
fn list_works_while_another_process_records_and_a_second_record_fails_fast() {
    let env = Env::new();
    let mut first = Recording::start(&env, &["record", "--fake-capture", "60"]);
    let id = first.wait_for_id();
    first.wait_for("a level line", |l| l.contains("peak"));
    let wav = env.data().join("takes").join(format!("{id}.wav"));

    for _ in 0..2 {
        let list = env.ok(&["list"]);
        let line = list.lines().find(|l| l.starts_with(&id)).unwrap();
        assert!(
            line.contains("Fake tone  ") && line.contains("  -  "),
            "{line}"
        );
        assert!(wav.is_file(), "the live WAV is left alone");
    }

    let started = Instant::now();
    let second = env.run(&["record", "--fake-capture", "1"]);
    assert!(!second.status.success());
    assert!(
        text(&second.stderr).contains("already running"),
        "{}",
        text(&second.stderr)
    );
    assert!(started.elapsed() < Duration::from_secs(5), "fails fast");

    first.close_stdin();
    let (ok, stdout, stderr) = first.finish(Duration::from_secs(20));
    assert!(ok, "{}\n{stderr}", stdout.join("\n"));
    assert!(
        stdout.iter().any(|l| l.starts_with("sealed Flac 48000 Hz")),
        "{}",
        stdout.join("\n")
    );
    assert!(!wav.exists());
}

#[test]
fn a_closed_stdin_stops_a_recording_that_an_undo_resumed() {
    let env = Env::new();
    let mut rec = Recording::start(&env, &["record", "--fake-capture", "60"]);
    rec.wait_for_id();
    rec.type_line("");
    rec.wait_for("the stop", |l| l.starts_with("stopped at"));
    // The undo and the end of input arrive together, as from `(echo; sleep 1; echo u) | ...`.
    rec.type_line("u");
    rec.close_stdin();
    let (ok, stdout, stderr) = rec.finish(Duration::from_secs(20));
    assert!(ok, "{}\n{stderr}", stdout.join("\n"));
    let sealed = stdout
        .iter()
        .find_map(|l| l.strip_prefix("sealed Flac 48000 Hz "))
        .unwrap_or_else(|| panic!("no sealed line in\n{}", stdout.join("\n")));
    let seconds: f64 = sealed.trim_end_matches(" s").parse().unwrap();
    assert!(
        seconds < 10.0,
        "stopped soon after the input closed: {seconds} s"
    );
}

#[test]
fn fake_flags_are_refused_unless_the_data_folder_is_named() {
    let home = tempfile::tempdir().unwrap();
    for args in [
        &["record", "--fake-capture", "1"][..],
        &["models", "--fake-engines"],
        &["process", "rec_1", "--fake-engines"],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_dory-cli"))
            .args(args)
            .env_remove("DORY_DATA_DIR")
            .env("HOME", home.path())
            .output()
            .unwrap();
        assert!(!out.status.success(), "{args:?}");
        assert!(
            text(&out.stderr).contains("DORY_DATA_DIR"),
            "{args:?}: {}",
            text(&out.stderr)
        );
    }
    assert!(
        !home.path().join("Library").exists(),
        "nothing was written to the default folder"
    );
}

#[test]
fn pretend_conditions_are_refused_unless_the_data_folder_is_named() {
    let home = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_dory-cli"))
        .args(["process", "rec_1"])
        .env_remove("DORY_DATA_DIR")
        .env("DORY_MACHINE", "battery")
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains("DORY_MACHINE") && stderr.contains("DORY_DATA_DIR"),
        "{stderr}"
    );
    assert!(!home.path().join("Library").exists());
}

#[cfg(unix)]
#[test]
fn a_seal_that_fails_after_the_recording_ends_makes_record_fail() {
    use std::os::unix::fs::PermissionsExt;
    let env = Env::new();
    let mut rec = Recording::start(&env, &["record", "--fake-capture", "1"]);
    rec.wait_for_id();
    // The open WAV keeps growing, but the FLAC cannot be created beside it.
    let takes = env.data().join("takes");
    std::fs::set_permissions(&takes, std::fs::Permissions::from_mode(0o555)).unwrap();
    let (ok, stdout, stderr) = rec.finish(Duration::from_secs(20));
    std::fs::set_permissions(&takes, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!ok, "{}\n{stderr}", stdout.join("\n"));
    assert!(stderr.contains("error sealing the take"), "{stderr}");
    assert!(stderr.contains("error(s) while recording"), "{stderr}");
}

#[test]
fn assistants_are_added_listed_and_hidden_recordings_marked() {
    let env = Env::new();
    let legacy = env.legacy();
    env.ok(&["import", legacy.to_str().unwrap()]);

    let listed = env.ok(&["assistants", "list"]);
    assert!(listed.contains("assistant access is off"), "{listed}");
    let activity = env.ok(&["assistants", "activity"]);
    assert!(
        activity.contains("No assistant activity yet."),
        "{activity}"
    );
    let pruned = env.ok(&["assistants", "prune"]);
    assert!(pruned.contains("deleted 0 day(s)"), "{pruned}");
    let on = env.ok(&["assistants", "on"]);
    assert!(on.contains("sent to that assistant's provider"), "{on}");

    let no_level = env.run(&["assistants", "add", "Claude"]);
    assert!(!no_level.status.success(), "there is no default exposure");

    let added = env.ok(&["assistants", "add", "Claude", "--exposure", "summaries"]);
    let key = added
        .split_whitespace()
        .find(|w| w.starts_with("dk_"))
        .unwrap_or_else(|| panic!("no key in {added}"));
    assert_eq!(key.len(), 67);
    let db = std::fs::read(env.data().join("dory.sqlite")).unwrap();
    assert!(
        !String::from_utf8_lossy(&db).contains(key),
        "only a digest is stored"
    );

    env.ok(&["assistants", "exposure", "Claude", "transcripts"]);
    env.ok(&["assistants", "edits", "Claude", "ask"]);
    let listed = env.ok(&["assistants", "list"]);
    assert!(listed.contains("assistant access is on"), "{listed}");
    assert!(
        listed.contains("Claude  transcripts  edits ask"),
        "{listed}"
    );
    assert!(!listed.contains(key));

    let hidden = env.ok(&["private", DONE, "on"]);
    assert!(hidden.contains("hidden from assistants"), "{hidden}");
    assert!(!env.run(&["private", "rec_nope", "on"]).status.success());

    env.ok(&["assistants", "remove", "Claude"]);
    assert!(
        env.ok(&["assistants", "list"])
            .contains("no assistants added")
    );
    assert!(
        !env.run(&["assistants", "remove", "Claude"])
            .status
            .success()
    );
}
