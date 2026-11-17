use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use dory_core::domain::{
    Access, Actor, AuditEntry, AuditOutcome, Exposure, RecordingId, Refusal, Timestamp,
};
use dory_core::ports::{AuditError, AuditRead, AuditSink};
use dory_store::JsonlAudit;

const DAY: i64 = 86_400_000;
const T0: i64 = 20_000 * DAY;

fn rid(s: &str) -> RecordingId {
    RecordingId::parse(s).unwrap()
}

fn entry(at: i64, access: Access, outcome: AuditOutcome) -> AuditEntry {
    AuditEntry {
        at: Timestamp(at),
        actor: Actor::Helper,
        client: Some("Claude".into()),
        reported_name: Some("claude-code".into()),
        access,
        outcome,
    }
}

fn served(at: i64) -> AuditEntry {
    entry(
        at,
        Access::Recording(rid("a")),
        AuditOutcome::Served { items: 1 },
    )
}

fn root(dir: &tempfile::TempDir) -> std::path::PathBuf {
    let root = dir.path().join("assistants").join("audit");
    JsonlAudit::create_root(&root).unwrap();
    root
}

fn files(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for day in std::fs::read_dir(root).unwrap() {
        for f in std::fs::read_dir(day.unwrap().path()).unwrap() {
            out.push(f.unwrap().path());
        }
    }
    out.sort();
    out
}

#[test]
fn every_entry_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    let log = JsonlAudit::writer(&root, Actor::Helper, Timestamp(T0));
    let all = vec![
        entry(
            T0 + 1,
            Access::search("budget", 2),
            AuditOutcome::Served { items: 2 },
        ),
        entry(
            T0 + 2,
            Access::Recording(rid("a")),
            AuditOutcome::Refused(Refusal::NotFound),
        ),
        entry(
            T0 + 3,
            Access::Summary(rid("a")),
            AuditOutcome::Refused(Refusal::NotShared {
                need: Exposure::Summaries,
            }),
        ),
        entry(
            T0 + 4,
            Access::Transcript {
                id: rid("a"),
                turns: (0, 41),
            },
            AuditOutcome::Served { items: 41 },
        ),
        entry(
            T0 + 5,
            Access::ActionItems {
                ids: vec![rid("a"), rid("b")],
            },
            AuditOutcome::Served { items: 3 },
        ),
        entry(
            T0 + 6,
            Access::ResourceList,
            AuditOutcome::Refused(Refusal::ServerOff),
        ),
        entry(
            T0 + 7,
            Access::Prompt {
                name: "prepare_one_on_one".into(),
            },
            AuditOutcome::Refused(Refusal::UnknownKey),
        ),
        entry(
            T0 + 8,
            Access::Write {
                what: "set the title".into(),
                id: Some(rid("a")),
            },
            AuditOutcome::Refused(Refusal::WritesOff),
        ),
        AuditEntry {
            client: None,
            reported_name: None,
            actor: Actor::App,
            ..served(T0 + 9)
        },
    ];
    for e in &all {
        log.append(e).unwrap();
    }
    let mut read = JsonlAudit::reader(&root).since(Timestamp(0), 100).unwrap();
    read.reverse();
    assert_eq!(read, all);
}

#[test]
fn two_writers_never_share_a_file_and_the_reader_merges_by_time() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    let a = JsonlAudit::writer(&root, Actor::Helper, Timestamp(T0));
    let b = JsonlAudit::writer(&root, Actor::Helper, Timestamp(T0 + 1));
    for i in 0..5 {
        a.append(&served(T0 + 10 * i)).unwrap();
        b.append(&served(T0 + 10 * i + 5)).unwrap();
    }
    assert_eq!(files(&root).len(), 2);
    let read = JsonlAudit::reader(&root).since(Timestamp(T0), 100).unwrap();
    let times: Vec<i64> = read.iter().map(|e| e.at.0 - T0).collect();
    assert_eq!(
        times,
        [45, 40, 35, 30, 25, 20, 15, 10, 5, 0],
        "newest first"
    );
    let limited = JsonlAudit::reader(&root)
        .since(Timestamp(T0 + 20), 3)
        .unwrap();
    assert_eq!(limited.len(), 3);
    assert_eq!(limited[0].at.0, T0 + 45);
}

#[test]
fn a_new_day_starts_a_new_folder() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    let log = JsonlAudit::writer(&root, Actor::Helper, Timestamp(T0));
    log.append(&served(T0)).unwrap();
    log.append(&served(T0 + DAY)).unwrap();
    assert_eq!(files(&root).len(), 2);
    assert_eq!(
        JsonlAudit::reader(&root)
            .since(Timestamp(T0 + DAY), 10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_torn_line_and_an_unknown_version_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    let log = JsonlAudit::writer(&root, Actor::Helper, Timestamp(T0));
    log.append(&served(T0)).unwrap();
    let file = files(&root).pop().unwrap();
    let mut text = std::fs::read_to_string(&file).unwrap();
    let good = text.clone();
    text.push_str("{\"v\":2,\"at\":1}\n");
    text.push_str(&good[..good.len() / 2]);
    std::fs::write(&file, text).unwrap();
    assert_eq!(
        JsonlAudit::reader(&root).since(Timestamp(0), 10).unwrap(),
        [served(T0)]
    );
}

#[test]
fn files_are_private_to_the_user() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    JsonlAudit::writer(&root, Actor::Helper, Timestamp(T0))
        .append(&served(T0))
        .unwrap();
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&root), 0o700);
    let file = files(&root).pop().unwrap();
    assert_eq!(mode(file.parent().unwrap()), 0o700);
    assert_eq!(mode(&file), 0o600);
}

#[test]
fn a_helper_never_creates_the_audit_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("assistants").join("audit");
    let log = JsonlAudit::writer(&root, Actor::Helper, Timestamp(T0));
    assert!(matches!(
        log.append(&served(T0)),
        Err(AuditError::Missing(_))
    ));
    assert!(!dir.path().join("assistants").exists());
    assert!(
        JsonlAudit::reader(&root)
            .since(Timestamp(0), 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn prune_keeps_thirty_days() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    for days_ago in [0, 29, 30, 31] {
        JsonlAudit::writer(&root, Actor::Helper, Timestamp(days_ago))
            .append(&served(T0 - days_ago * DAY))
            .unwrap();
    }
    assert_eq!(JsonlAudit::prune(&root, Timestamp(T0 + 5)).unwrap(), 2);
    let left = JsonlAudit::reader(&root).since(Timestamp(0), 10).unwrap();
    let ages: Vec<i64> = left.iter().map(|e| (T0 - e.at.0) / DAY).collect();
    assert_eq!(ages, [0, 29]);
}
