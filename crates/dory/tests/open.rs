use std::ffi::OsString;
use std::path::Path;

use dory::{Config, ConfigError, OpenError, legacy_data_dir};
use dory_core::domain::RecordingQuery;

fn os(s: &str) -> Option<OsString> {
    Some(OsString::from(s))
}

#[test]
fn opens_twice_on_the_same_folder_and_keeps_what_the_first_wrote() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path().join("data"));

    let first = dory::open(&config).unwrap();
    assert!(config.db_path().is_file());
    assert!(
        first
            .library()
            .list(&RecordingQuery::default())
            .unwrap()
            .is_empty()
    );
    drop(first);

    let second = dory::open(&config).unwrap();
    assert!(
        second
            .library()
            .list(&RecordingQuery::default())
            .unwrap()
            .is_empty()
    );
    assert!(second.recovery().sealed.is_empty());
}

#[test]
fn a_second_open_of_a_folder_fails_fast_until_the_first_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path());
    let first = dory::open(&config).unwrap();
    match dory::open(&config) {
        Err(OpenError::InUse(path)) => assert_eq!(path, dir.path()),
        other => panic!("expected InUse, got {:?}", other.err()),
    }
    assert!(matches!(
        dory::import_legacy(&config, dir.path()),
        Err(dory::ImportError::Open(OpenError::InUse(_)))
    ));
    let reader = dory::open_library(&config).unwrap();
    assert!(reader.list(&RecordingQuery::default()).unwrap().is_empty());
    drop(first);
    dory::open(&config).expect("the lock goes with the handle");
}

#[test]
fn data_dir_comes_from_the_variable() {
    let config = Config::from_vars(os("/tmp/lane-7"), os("/Users/me")).unwrap();
    assert_eq!(config.data_dir, Path::new("/tmp/lane-7"));
    assert_eq!(config.db_path(), Path::new("/tmp/lane-7/dory.sqlite"));
    assert_eq!(config.takes_dir(), Path::new("/tmp/lane-7/takes"));
}

#[test]
fn an_empty_variable_is_an_error_not_the_default() {
    assert_eq!(
        Config::from_vars(os(""), os("/Users/me")),
        Err(ConfigError::EmptyDataDir)
    );
}

#[cfg(target_os = "macos")]
#[test]
fn the_default_is_a_v2_folder_beside_the_old_apps_data_never_the_old_folder() {
    let home = Path::new("/Users/me");
    let config = Config::from_vars(None, os("/Users/me")).unwrap();
    let legacy = legacy_data_dir(home);
    assert_eq!(
        legacy,
        Path::new("/Users/me/Library/Application Support/com.dory.app")
    );
    assert_eq!(config.data_dir, legacy.join("v2"));
    assert_ne!(config.data_dir, legacy);
}

#[test]
fn pointing_the_variable_at_the_old_apps_folder_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let legacy = legacy_data_dir(home.path());
    std::fs::create_dir_all(&legacy).unwrap();
    let home_var = Some(home.path().as_os_str().to_owned());

    for spelling in [
        legacy.clone(),
        legacy.join("."),
        legacy.join("../com.dory.app"),
    ] {
        assert_eq!(
            Config::from_vars(Some(spelling.into_os_string()), home_var.clone()),
            Err(ConfigError::LegacyDir(legacy.clone()))
        );
    }
    assert!(Config::from_vars(Some(legacy.join("v2").into_os_string()), home_var).is_ok());
}

#[test]
fn without_a_home_the_variable_is_required() {
    assert_eq!(Config::from_vars(None, None), Err(ConfigError::NoDefault));
    assert!(Config::from_vars(os("/tmp/x"), None).is_ok());
}

#[test]
fn import_reads_the_old_folder_and_writes_only_the_new_one() {
    let old = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(old.path().join("metadata")).unwrap();
    std::fs::create_dir_all(old.path().join("recordings")).unwrap();
    std::fs::write(old.path().join("recordings/rec_1_a.webm"), b"not audio").unwrap();
    std::fs::write(
        old.path().join("metadata/rec_1_a.json"),
        r#"{"id":"rec_1_a","display_name":"Old one","duration_secs":3.0,"created_at":1731234567}"#,
    )
    .unwrap();
    let before = snapshot(old.path());

    let data = tempfile::tempdir().unwrap();
    let config = Config::new(data.path());
    let report = dory::import_legacy(&config, old.path()).unwrap();
    assert_eq!(report.imported.len(), 1);
    assert_eq!(
        dory::import_legacy(&config, old.path())
            .unwrap()
            .imported
            .len(),
        0
    );

    assert_eq!(snapshot(old.path()), before);
    let dory = dory::open(&config).unwrap();
    let rows = dory.library().list(&RecordingQuery::default()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].recording.title.as_deref(), Some("Old one"));
}

fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(d).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let name = path.strip_prefix(dir).unwrap().display().to_string();
                files.push((name, std::fs::read(&path).unwrap()));
            }
        }
    }
    files.sort();
    files
}

#[cfg(feature = "fake-engines")]
mod fakes {
    use dory_core::app::{Dory, RecorderStatus, StartOptions};
    use dory_core::domain::{ChannelLayout, Goal, JobState, Ms, RecordingId, TakeContainer};
    use dory_core::models::Stt;
    use dory_core::testing::{block_on, eventually};

    use super::*;

    #[test]
    fn fake_engines_are_registered_in_both_catalogs() {
        let dir = tempfile::tempdir().unwrap();
        let dory = dory::open_with_fake_engines(&Config::new(dir.path())).unwrap();
        let stt = block_on(dory.models().list::<Stt>()).unwrap();
        let llm = block_on(dory.models().list::<dory_core::models::Llm>()).unwrap();
        assert_eq!(
            stt.iter()
                .map(|r| r.status.entry.id.to_string())
                .collect::<Vec<_>>(),
            [dory::FAKE_STT, "fake/en-locale-stt"]
        );
        assert_eq!(
            llm.iter()
                .map(|r| r.status.entry.id.to_string())
                .collect::<Vec<_>>(),
            [dory::FAKE_LLM, "fake/en-only-llm"]
        );
    }

    #[test]
    fn plain_open_registers_sherpa_and_ollama_with_models_under_the_data_folder() {
        let dir = tempfile::tempdir().unwrap();
        let dory = dory::open(&Config::new(dir.path())).unwrap();
        let stt: Vec<String> = block_on(dory.models().list::<Stt>())
            .unwrap()
            .iter()
            .map(|r| r.status.entry.id.to_string())
            .collect();
        assert_eq!(stt, ["sherpa/parakeet-tdt-0.6b-v3", "sherpa/whisper-turbo"]);
        let llm: Vec<String> = block_on(dory.models().list::<dory_core::models::Llm>())
            .unwrap()
            .iter()
            .map(|r| r.status.entry.id.to_string())
            .collect();
        assert_eq!(llm, ["ollama/gemma4:e4b", "ollama/gemma4:12b"]);
        assert_eq!(
            Config::new(dir.path()).models_dir(),
            dir.path().join("models")
        );
    }

    fn recorded(dory: &Dory) -> Option<Ms> {
        match dory.recorder().status() {
            RecorderStatus::Recording { recorded, .. } | RecorderStatus::Grace { recorded, .. } => {
                Some(recorded)
            }
            _ => None,
        }
    }

    /// Records at least `seconds` of the fake tone and stops. Returns the id and the length the
    /// recorder reported at the stop, which is what the sealed take must hold.
    fn record(dory: &Dory, seconds: u32) -> (RecordingId, Ms) {
        dory.recorder()
            .start(StartOptions {
                layout: ChannelLayout::MicSystem,
                ..StartOptions::default()
            })
            .unwrap();
        eventually("recording", || dory.recorder().status().id().is_some());
        let id = dory.recorder().status().id().cloned().unwrap();
        eventually("the fake clip to play out", || {
            recorded(dory).is_some_and(|r| r.0 >= u64::from(seconds) * 1000)
        });
        dory.recorder().stop().unwrap();
        eventually("grace", || {
            matches!(dory.recorder().status(), RecorderStatus::Grace { .. })
        });
        (id, recorded(dory).unwrap())
    }

    #[test]
    fn a_reader_lists_while_another_handle_records_and_the_live_wav_survives() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let recorder = dory::open_with_fake_capture(&config).unwrap();
        recorder.recorder().start(StartOptions::default()).unwrap();
        eventually("some audio", || {
            recorded(&recorder).is_some_and(|r| r.0 >= 300)
        });
        let id = recorder.recorder().status().id().cloned().unwrap();
        let wav = config.takes_dir().join(format!("{id}.wav"));

        assert!(matches!(
            dory::open_with_fake_engines(&config),
            Err(OpenError::InUse(_))
        ));
        let reader = dory::open_library(&config).unwrap();
        let rows = reader.list(&RecordingQuery::default()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].recording.id, id);
        assert_eq!(rows[0].recording.take, None, "still recording");
        assert!(wav.is_file(), "the live WAV is left alone");

        let before = recorded(&recorder).unwrap();
        eventually("the recording to go on", || {
            recorded(&recorder).is_some_and(|r| r.0 >= before.0 + 300)
        });
        recorder.recorder().stop().unwrap();
        eventually("grace", || {
            matches!(recorder.recorder().status(), RecorderStatus::Grace { .. })
        });
        let length = recorded(&recorder).unwrap();
        drop(recorder);

        let take = reader.recording(&id).unwrap().unwrap().take.unwrap();
        assert_eq!(take.container, TakeContainer::Flac);
        assert_eq!(take.duration, length);
        assert!(!wav.exists());
    }

    #[test]
    fn fake_capture_then_fake_engines_reach_a_summary() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(dir.path());
        let (id, length) = {
            let dory = dory::open_with_fake_capture(&config).unwrap();
            let recorded = record(&dory, 1);
            // Dropping `Dory` finalizes the stop and waits for the take to seal.
            drop(dory);
            recorded
        };

        let dory = dory::open_with_fake_engines(&config).unwrap();
        let row = &dory.library().list(&RecordingQuery::default()).unwrap()[0];
        assert_eq!(row.recording.id, id);
        let take = row.recording.take.expect("sealed");
        assert_eq!(take.duration, length);

        // The recording's job blocked on the missing engine under the first handle. Opening
        // again re-checks blocked jobs, so it runs with the engines this handle has, and
        // enqueueing again joins it instead of adding a second job.
        dory.pipeline().enqueue(&id, Goal::ALL).unwrap();
        assert_eq!(dory.library().jobs(&id).unwrap().len(), 1);
        eventually("the job to finish", || {
            dory.library()
                .jobs(&id)
                .unwrap()
                .iter()
                .any(|j| matches!(j.state, JobState::Succeeded | JobState::Failed { .. }))
        });
        let jobs = dory.library().jobs(&id).unwrap();
        assert!(
            jobs.iter().any(|j| j.state == JobState::Succeeded),
            "{jobs:?}"
        );
        assert!(
            dory.library()
                .summary(&id)
                .unwrap()
                .unwrap()
                .markdown
                .contains("Fake summary")
        );
    }
}

#[test]
fn open_library_leaves_the_library_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(dir.path().join("data"));
    drop(dory::open(&config).unwrap());
    let before = std::fs::read(config.db_path()).unwrap();
    let reader = dory::open_library(&config).unwrap();
    assert!(reader.list(&RecordingQuery::default()).unwrap().is_empty());
    drop(reader);
    assert_eq!(std::fs::read(config.db_path()).unwrap(), before);

    let absent = Config::new(dir.path().join("absent"));
    assert!(matches!(
        dory::open_library(&absent),
        Err(OpenError::Store(dory_core::ports::StoreError::Missing(_)))
    ));
    assert!(!absent.data_dir.exists(), "a reader creates no folder");
}
