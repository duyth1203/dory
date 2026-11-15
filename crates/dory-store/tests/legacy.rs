use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use dory_core::domain::{
    Channel, ChannelLayout, FailureClass, Goal, JobState, LanguagePref, Ms, Priority,
    RecordingEdit, RecordingId, RecordingQuery, Revision, SpeakerId, Stage, SummaryFlag,
    TakeContainer, Timestamp,
};
use dory_core::ports::{Change, Store, StoreRead, TakeError, Takes};
use dory_store::{FsTakes, ImportError, SqliteStore, import_legacy};

const DONE: &str = "rec_1731234567890_k3j2h1x";
const QUEUED: &str = "rec_1731300000000_q9w8e7r";
const FAILED: &str = "rec_1731400000000_zz11aa2";
const INTERRUPTED: &str = "rec_1731500000000_mm44nn5";
const WAV: &str = "rec_1731600000000_ww22xx3";
const CORRUPT: &str = "rec_1731700000000_cc55dd6";
const LOOSE: &str = "rec_1731800000000_loose01";
const GHOST: &str = "rec_1731900000000_ghost99";
const IMPORTED: [&str; 6] = [DONE, QUEUED, FAILED, INTERRUPTED, WAV, LOOSE];

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/legacy-data")
}

fn rid(s: &str) -> RecordingId {
    RecordingId::parse(s).unwrap()
}

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn store(&self) -> SqliteStore {
        SqliteStore::open(self.dir.path().join("new/dory.db")).unwrap()
    }

    fn takes(&self) -> FsTakes {
        FsTakes::new(self.dir.path().join("new/takes"))
    }

    /// A writable copy of the fixture, for tests that change the old folder.
    fn old_copy(&self) -> PathBuf {
        let dest = self.dir.path().join("old");
        copy_dir(&fixture(), &dest);
        dest
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

type Snapshot = BTreeMap<PathBuf, Option<(Vec<u8>, SystemTime)>>;

/// Every path under `dir` with its bytes and modification time, so any write, touch, rename, or
/// delete shows up. This is the in-process equivalent of `shasum` before and after.
fn snapshot(dir: &Path) -> Snapshot {
    fn walk(dir: &Path, out: &mut Snapshot) {
        if !dir.exists() {
            return;
        }
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                out.insert(path.clone(), None);
                walk(&path, out);
            } else {
                let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
                out.insert(
                    path.clone(),
                    Some((std::fs::read(&path).unwrap(), modified)),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, &mut out);
    out
}

#[test]
fn the_fixture_folder_imports_as_the_old_app_listed_it() {
    let env = Env::new();
    let (store, takes) = (env.store(), env.takes());
    let before = snapshot(&fixture());

    let report = import_legacy(&store, &takes, &fixture()).unwrap();

    assert_eq!(
        report.imported,
        IMPORTED.iter().map(|s| rid(s)).collect::<Vec<_>>()
    );
    assert_eq!((report.updated.len(), report.unchanged), (0, 0));
    let skipped: BTreeMap<String, &str> = report
        .skipped
        .iter()
        .map(|s| {
            (
                s.path.file_stem().unwrap().to_string_lossy().into_owned(),
                s.reason.as_str(),
            )
        })
        .collect();
    assert_eq!(skipped.len(), 2, "{report}");
    assert!(
        skipped[CORRUPT].contains("corrupt JSON"),
        "{}",
        skipped[CORRUPT]
    );
    assert!(skipped[GHOST].contains("no audio"), "{}", skipped[GHOST]);
    assert_eq!(
        snapshot(&fixture()),
        before,
        "the old folder is byte and mtime identical"
    );

    let rows = store.recordings(&RecordingQuery::default()).unwrap();
    assert_eq!(
        rows.iter()
            .map(|r| r.recording.id.to_string())
            .collect::<Vec<_>>(),
        [LOOSE, WAV, INTERRUPTED, FAILED, QUEUED, DONE],
        "newest first, and the audio-only file takes its file time"
    );

    let done = store.recording(&rid(DONE)).unwrap().unwrap();
    assert_eq!(
        done.title.as_deref(),
        Some("Weekly sync \u{2013} k\u{1ebf} ho\u{1ea1}ch qu\u{fd} 4")
    );
    assert_eq!(
        done.created_at,
        Timestamp(1_731_234_567_000),
        "seconds became milliseconds"
    );
    assert_eq!(
        (done.layout, done.language.clone()),
        (ChannelLayout::Mixed, LanguagePref::Auto)
    );
    let take = done.take.unwrap();
    assert_eq!(
        (take.container, take.rate_hz, take.duration),
        (TakeContainer::LegacyWebm, 48_000, Ms(1_832_400))
    );
    let old_audio = fixture()
        .canonicalize()
        .unwrap()
        .join(format!("recordings/{DONE}.webm"));
    assert_eq!(
        takes.playback_path(&rid(DONE)).unwrap(),
        old_audio,
        "the take points at the old file"
    );
    assert_eq!(takes.inspect(&rid(DONE)).unwrap(), Some(take));

    let transcript = store.transcript(&rid(DONE)).unwrap().unwrap();
    assert_eq!(transcript.revision, Revision(1));
    assert!(!transcript.is_timed());
    assert_eq!(
        transcript
            .segments
            .iter()
            .map(|s| (s.speaker, s.range, s.channel))
            .collect::<Vec<_>>(),
        [
            (SpeakerId(0), None, Channel::Mixed),
            (SpeakerId(1), None, Channel::Mixed),
            (SpeakerId(0), None, Channel::Mixed),
            (SpeakerId(2), None, Channel::Mixed),
        ]
    );
    assert_eq!(
        transcript.segments[0].text,
        "Ch\u{e0}o m\u{1ecd}i ng\u{1b0}\u{1edd}i, ch\u{fa}ng ta b\u{1eaf}t \u{111}\u{1ea7}u nh\u{e9}."
    );
    assert_eq!(transcript.produced_by.as_str(), "legacy/unknown");
    let summary = store.summary(&rid(DONE)).unwrap().unwrap();
    assert!(summary.markdown.starts_with("## Weekly sync"));
    assert_eq!(summary.transcript_revision, Revision(1));
    let row = &rows[5];
    assert_eq!(
        (row.has_transcript, row.summary, row.job.clone()),
        (true, SummaryFlag::Current, Some(JobState::Succeeded))
    );
    let job = store.jobs_for(&rid(DONE)).unwrap().remove(0);
    assert_eq!(
        (job.goal, job.done.clone()),
        (Goal::ALL, vec![Stage::Transcribe, Stage::Summarize])
    );

    let queued = store.jobs_for(&rid(QUEUED)).unwrap().remove(0);
    assert_eq!(
        (
            queued.state.clone(),
            queued.priority,
            queued.done.clone(),
            queued.goal
        ),
        (JobState::Queued, Priority::Front, vec![], Goal::ALL)
    );
    assert_eq!(
        store
            .next_runnable_job(Timestamp(i64::MAX))
            .unwrap()
            .map(|j| j.id),
        Some(queued.id)
    );
    assert!(store.transcript(&rid(QUEUED)).unwrap().is_none());

    let failed = store.jobs_for(&rid(FAILED)).unwrap().remove(0);
    let JobState::Failed { stage, error } = &failed.state else {
        panic!("expected Failed, got {:?}", failed.state)
    };
    assert_eq!(
        *stage,
        Stage::Summarize,
        "it had a transcript, so summarizing is what failed"
    );
    assert_eq!(
        (error.class, error.message.as_str()),
        (FailureClass::Permanent, "ollama: connection refused")
    );
    assert_eq!(failed.done, vec![Stage::Transcribe]);
    assert_eq!(
        store
            .transcript(&rid(FAILED))
            .unwrap()
            .unwrap()
            .segments
            .len(),
        2
    );
    assert!(store.summary(&rid(FAILED)).unwrap().is_none());

    let interrupted = store.jobs_for(&rid(INTERRUPTED)).unwrap().remove(0);
    assert_eq!(
        (interrupted.state, interrupted.priority),
        (JobState::Queued, Priority::Normal),
        "a stage the old app was in the middle of is queued again, as the old app did on launch"
    );

    let wav = store.recording(&rid(WAV)).unwrap().unwrap();
    let take = wav.take.unwrap();
    assert_eq!(
        (take.container, take.rate_hz, take.duration),
        (TakeContainer::Wav, 8_000, Ms(250)),
        "no duration in the metadata, so it comes from the WAV"
    );
    assert!(
        store.jobs_for(&rid(WAV)).unwrap().is_empty(),
        "no pipeline status, no job"
    );
    let plain = store.transcript(&rid(WAV)).unwrap().unwrap();
    assert_eq!(
        plain
            .segments
            .iter()
            .map(|s| (s.speaker, s.text.as_str()))
            .collect::<Vec<_>>(),
        [
            (SpeakerId(0), "Just one voice, no speaker labels."),
            (SpeakerId(0), "Second line.")
        ]
    );
    assert_eq!(
        store.summary(&rid(WAV)).unwrap().unwrap().markdown,
        "Short note."
    );

    let loose = store.recording(&rid(LOOSE)).unwrap().unwrap();
    assert_eq!((loose.title, loose.take.unwrap().duration), (None, Ms(0)));
    assert!(loose.created_at.0 > 1_731_000_000_000, "its file time");
    assert!(store.jobs_for(&rid(LOOSE)).unwrap().is_empty());

    let pointers = std::fs::read_dir(env.dir.path().join("new/takes"))
        .unwrap()
        .count();
    assert_eq!(pointers, 6, "one pointer per recording and no audio copied");
    assert!(takes.orphans().unwrap().is_empty());
}

#[test]
fn importing_twice_changes_nothing_even_across_a_restart() {
    let env = Env::new();
    {
        let (store, takes) = (env.store(), env.takes());
        import_legacy(&store, &takes, &fixture()).unwrap();
    }
    let new_before = snapshot(&env.dir.path().join("new/takes"));
    let (store, takes) = (env.store(), env.takes());
    let rows_before = store.recordings(&RecordingQuery::default()).unwrap();
    let jobs_before: Vec<_> = IMPORTED
        .iter()
        .flat_map(|id| store.jobs_for(&rid(id)).unwrap())
        .collect();
    let transcripts_before: Vec<_> = IMPORTED
        .iter()
        .map(|id| store.transcript(&rid(id)).unwrap())
        .collect();
    let old_before = snapshot(&fixture());

    let second = import_legacy(&store, &takes, &fixture()).unwrap();
    assert!(
        second.imported.is_empty() && second.updated.is_empty(),
        "{second}"
    );
    assert_eq!(second.unchanged, 6);
    assert_eq!(
        second.skipped.len(),
        2,
        "the two unimportable files are reported again"
    );

    assert_eq!(
        store.recordings(&RecordingQuery::default()).unwrap(),
        rows_before
    );
    let jobs_after: Vec<_> = IMPORTED
        .iter()
        .flat_map(|id| store.jobs_for(&rid(id)).unwrap())
        .collect();
    assert_eq!(jobs_after, jobs_before, "no job was recreated");
    let transcripts_after: Vec<_> = IMPORTED
        .iter()
        .map(|id| store.transcript(&rid(id)).unwrap())
        .collect();
    assert_eq!(
        transcripts_after, transcripts_before,
        "revisions did not move"
    );
    assert_eq!(
        snapshot(&env.dir.path().join("new/takes")),
        new_before,
        "not even a pointer file was rewritten"
    );
    assert_eq!(snapshot(&fixture()), old_before);
}

#[test]
fn a_corrupt_file_is_reported_and_the_rest_import() {
    let env = Env::new();
    let old = env.old_copy();
    std::fs::write(old.join(format!("metadata/{QUEUED}.json")), b"{ not json").unwrap();
    std::fs::write(old.join(format!("metadata/{FAILED}.json")), b"[1, 2, 3]").unwrap();
    std::fs::write(old.join("metadata/bad id.json"), b"{}").unwrap();
    std::fs::write(old.join("recordings/bad id.webm"), b"x").unwrap();
    let (store, takes) = (env.store(), env.takes());
    let before = snapshot(&old);

    let report = import_legacy(&store, &takes, &old).unwrap();

    assert_eq!(report.imported.len(), 4, "{report}");
    let reasons: BTreeMap<String, String> = report
        .skipped
        .iter()
        .map(|s| {
            (
                s.path.file_stem().unwrap().to_string_lossy().into_owned(),
                s.reason.clone(),
            )
        })
        .collect();
    assert!(reasons[QUEUED].contains("corrupt JSON"), "{reasons:?}");
    assert!(reasons[FAILED].contains("not an object"), "{reasons:?}");
    assert!(reasons["bad id"].contains("recording id"), "{reasons:?}");
    assert!(reasons[CORRUPT].contains("corrupt JSON"));
    assert!(store.recording(&rid(QUEUED)).unwrap().is_none());
    assert!(store.recording(&rid(DONE)).unwrap().is_some());
    assert_eq!(snapshot(&old), before);

    std::fs::write(
        old.join(format!("metadata/{QUEUED}.json")),
        br#"{"id":"q","duration_secs":3,"created_at":5}"#,
    )
    .unwrap();
    let healed = import_legacy(&store, &takes, &old).unwrap();
    assert_eq!(
        healed.imported,
        vec![rid(QUEUED)],
        "a repaired file imports on the next run"
    );
}

#[test]
fn one_recording_failing_to_link_does_not_stop_the_others_and_a_rerun_finishes() {
    let env = Env::new();
    let (store, takes) = (env.store(), env.takes());
    std::fs::create_dir_all(env.dir.path().join(format!("new/takes/{FAILED}.legacy"))).unwrap();

    let first = import_legacy(&store, &takes, &fixture()).unwrap();
    assert_eq!(first.imported.len(), 5, "{first}");
    assert!(
        first
            .skipped
            .iter()
            .any(|s| s.path.to_string_lossy().contains(FAILED)),
        "{first}"
    );
    assert!(
        store.recording(&rid(FAILED)).unwrap().is_none(),
        "no half-imported row"
    );

    std::fs::remove_dir(env.dir.path().join(format!("new/takes/{FAILED}.legacy"))).unwrap();
    let second = import_legacy(&store, &takes, &fixture()).unwrap();
    assert_eq!(second.imported, vec![rid(FAILED)]);
    assert_eq!(second.unchanged, 5);
    assert_eq!(
        store.recordings(&RecordingQuery::default()).unwrap().len(),
        6
    );
}

#[test]
fn changed_metadata_updates_the_recording_and_keeps_the_users_own_edits() {
    let env = Env::new();
    let old = env.old_copy();
    let (store, takes) = (env.store(), env.takes());
    import_legacy(&store, &takes, &old).unwrap();
    store
        .apply(vec![Change::RecordingEdited {
            id: rid(QUEUED),
            edit: RecordingEdit::Title(Some("My own name".into())),
        }])
        .unwrap();
    let old_job = store.jobs_for(&rid(QUEUED)).unwrap().remove(0);

    // The old app finished the queued recording and re-transcribed the done one.
    std::fs::write(
        old.join(format!("metadata/{QUEUED}.json")),
        serde_json_text(&[
            ("transcript", "[Speaker 1] Now there is text."),
            ("summary", "Now there is a summary."),
            ("duration_secs", "412.0"),
            ("created_at", "1731300000"),
            ("pipeline_status", "done"),
        ]),
    )
    .unwrap();
    std::fs::write(
        old.join(format!("metadata/{DONE}.json")),
        serde_json_text(&[
            ("display_name", "Weekly sync"),
            ("transcript", "[Speaker 1] A different transcript."),
            ("summary", "## Weekly sync\n\nNew summary."),
            ("duration_secs", "1832.4"),
            ("created_at", "1731234567"),
            ("pipeline_status", "done"),
        ]),
    )
    .unwrap();

    let report = import_legacy(&store, &takes, &old).unwrap();
    assert_eq!(report.updated.len(), 2, "{report}");
    assert_eq!(report.unchanged, 4);
    assert!(report.imported.is_empty());

    let queued = store.recording(&rid(QUEUED)).unwrap().unwrap();
    assert_eq!(
        queued.title.as_deref(),
        Some("My own name"),
        "a title the user set here survives"
    );
    assert_eq!(
        store.transcript(&rid(QUEUED)).unwrap().unwrap().revision,
        Revision(1)
    );
    let jobs = store.jobs_for(&rid(QUEUED)).unwrap();
    assert_eq!(jobs.len(), 1);
    assert_ne!(jobs[0].id, old_job.id, "the stale queued job was replaced");
    assert_eq!(jobs[0].state, JobState::Succeeded);
    assert!(
        store
            .next_runnable_job(Timestamp(i64::MAX))
            .unwrap()
            .is_none_or(|j| j.recording != rid(QUEUED)),
        "the finished recording is not in the queue"
    );

    let done = store.recording(&rid(DONE)).unwrap().unwrap();
    assert_eq!(done.title.as_deref(), Some("Weekly sync"));
    let transcript = store.transcript(&rid(DONE)).unwrap().unwrap();
    assert_eq!(
        transcript.revision,
        Revision(2),
        "new text is a new revision"
    );
    assert_eq!(
        store
            .summary(&rid(DONE))
            .unwrap()
            .unwrap()
            .transcript_revision,
        Revision(2)
    );
    let rows = store.recordings(&RecordingQuery::default()).unwrap();
    assert!(
        rows.iter()
            .filter(|r| r.recording.id == rid(DONE))
            .all(|r| r.summary == SummaryFlag::Current)
    );
    assert_eq!(store.jobs_for(&rid(DONE)).unwrap().len(), 1);

    let again = import_legacy(&store, &takes, &old).unwrap();
    assert_eq!(
        (again.updated.len(), again.imported.len(), again.unchanged),
        (0, 0, 6)
    );
}

/// A metadata file as the old app writes it: string, number, and bool values.
fn serde_json_text(pairs: &[(&str, &str)]) -> String {
    let object: serde_json::Map<String, serde_json::Value> = pairs
        .iter()
        .map(|(k, v)| {
            let value = v
                .parse::<f64>()
                .map_or_else(|_| serde_json::Value::from(*v), serde_json::Value::from);
            ((*k).to_owned(), value)
        })
        .collect();
    serde_json::Value::Object(object).to_string()
}

#[test]
fn a_recording_deleted_here_stays_deleted_until_its_old_metadata_changes_and_old_audio_survives() {
    let env = Env::new();
    let old = env.old_copy();
    let (store, takes) = (env.store(), env.takes());
    import_legacy(&store, &takes, &old).unwrap();
    let audio = old.join(format!("recordings/{DONE}.webm"));
    let audio_before = std::fs::read(&audio).unwrap();

    store
        .apply(vec![Change::RecordingDeleted(rid(DONE))])
        .unwrap();
    takes.delete(&rid(DONE)).unwrap();
    assert!(
        audio.exists() && std::fs::read(&audio).unwrap() == audio_before,
        "delete never touches the old audio"
    );
    assert!(matches!(
        takes.playback_path(&rid(DONE)),
        Err(TakeError::Missing(_))
    ));

    let again = import_legacy(&store, &takes, &old).unwrap();
    assert_eq!(again.unchanged, 6, "{again}");
    assert!(
        store.recording(&rid(DONE)).unwrap().is_none(),
        "no resurrection"
    );

    std::fs::write(
        old.join(format!("metadata/{DONE}.json")),
        serde_json_text(&[
            ("duration_secs", "5"),
            ("created_at", "1731234567"),
            ("pipeline_status", "idle"),
        ]),
    )
    .unwrap();
    let back = import_legacy(&store, &takes, &old).unwrap();
    assert_eq!(back.imported, vec![rid(DONE)], "{back}");
    assert!(takes.inspect(&rid(DONE)).unwrap().is_some());
}

#[test]
fn a_folder_that_is_not_a_data_folder_is_an_error_not_an_empty_import() {
    let env = Env::new();
    let (store, takes) = (env.store(), env.takes());
    let empty = env.dir.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    assert!(matches!(
        import_legacy(&store, &takes, &empty),
        Err(ImportError::NotADataFolder(_))
    ));
    assert!(matches!(
        import_legacy(&store, &takes, &env.dir.path().join("missing")),
        Err(ImportError::Io { .. })
    ));
}

#[test]
fn a_recording_with_a_native_take_of_the_same_id_is_not_overwritten() {
    let env = Env::new();
    let (store, takes) = (env.store(), env.takes());
    let mut writer = takes
        .begin(&rid(WAV), ChannelLayout::MicOnly, 16_000)
        .unwrap();
    writer.write(&[0.0; 160]).unwrap();
    writer.finish().unwrap();
    let report = import_legacy(&store, &takes, &fixture()).unwrap();
    assert!(!report.imported.contains(&rid(WAV)), "{report}");
    assert!(
        report
            .skipped
            .iter()
            .any(|s| s.reason.contains("already exists")),
        "{report}"
    );
}

#[test]
fn two_hundred_recordings_import_within_the_plans_budget_and_five_hundred_list_fast() {
    let env = Env::new();
    let old = env.dir.path().join("big-old");
    std::fs::create_dir_all(old.join("recordings")).unwrap();
    std::fs::create_dir_all(old.join("metadata")).unwrap();
    let statuses = ["done", "queued", "failed", "idle"];
    for n in 0..200u64 {
        let id = format!("rec_{}_{n:07}", 1_700_000_000_000 + n);
        std::fs::write(old.join(format!("recordings/{id}.webm")), b"webm").unwrap();
        let transcript: String = (0..40)
            .map(|i| format!("[Speaker {}] line {i} of recording {n}\n", i % 3 + 1))
            .collect();
        std::fs::write(
            old.join(format!("metadata/{id}.json")),
            serde_json_text(&[
                ("transcript", transcript.as_str()),
                ("summary", "## Notes\n\n- a point"),
                ("duration_secs", "600"),
                ("created_at", &(1_700_000_000 + n).to_string()),
                ("pipeline_status", statuses[(n % 4) as usize]),
            ]),
        )
        .unwrap();
    }
    let (store, takes) = (env.store(), env.takes());

    let started = std::time::Instant::now();
    let report = import_legacy(&store, &takes, &old).unwrap();
    let import_time = started.elapsed();
    assert_eq!(report.imported.len(), 200, "{report}");
    assert!(
        import_time < std::time::Duration::from_secs(5),
        "importing 200 recordings took {import_time:?}"
    );

    for n in 0..300u64 {
        store
            .apply(vec![Change::RecordingCreated(
                dory_core::domain::Recording {
                    id: rid(&format!("native-{n}")),
                    title: Some(format!("Native {n}")),
                    created_at: Timestamp(n as i64),
                    language: LanguagePref::Auto,
                    layout: ChannelLayout::MicSystem,
                    take: None,
                },
            )])
            .unwrap();
    }
    let started = std::time::Instant::now();
    let rows = store.recordings(&RecordingQuery::default()).unwrap();
    let list_time = started.elapsed();
    assert_eq!(rows.len(), 500);
    assert!(
        list_time < std::time::Duration::from_millis(100),
        "listing 500 recordings took {list_time:?}"
    );
    eprintln!("import 200: {import_time:?}, list 500: {list_time:?}");
}
