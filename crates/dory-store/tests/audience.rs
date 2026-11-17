use dory_core::domain::{
    AssistantChange, AssistantClient, AssistantQuery, Channel, ChannelLayout, ClientKey, Exposure,
    Fields, LangTag, LanguagePref, Recording, RecordingId, Revision, Segment, SpeakerId, Summary,
    Timestamp, Transcript, Writes,
};
use dory_core::models::ModelRef;
use dory_core::ports::{Change, Store, StoreError, StoreRead};
use dory_core::testing::store::MemStore;
use dory_store::{SqliteReader, SqliteStore};

fn rid(s: &str) -> RecordingId {
    RecordingId::parse(s).unwrap()
}

fn meeting(id: &str, at: i64, title: &str, transcript: &str, summary: &str) -> Vec<Change> {
    let id = rid(id);
    let model = ModelRef::parse("fake/stt").unwrap();
    vec![
        Change::RecordingCreated(Recording {
            id: id.clone(),
            title: Some(title.into()),
            created_at: Timestamp(at),
            language: LanguagePref::Auto,
            layout: ChannelLayout::MicSystem,
            take: None,
        }),
        Change::TranscriptReplaced {
            id: id.clone(),
            transcript: Transcript {
                revision: Revision(1),
                segments: vec![Segment {
                    range: None,
                    speaker: SpeakerId(1),
                    channel: Channel::System,
                    text: transcript.into(),
                }],
                detected_languages: vec![],
                produced_by: model.clone(),
            },
        },
        Change::SummaryReplaced {
            id,
            summary: Summary {
                markdown: summary.into(),
                language: Some(LangTag::parse("en").unwrap()),
                produced_by: model,
                transcript_revision: Revision(1),
            },
        },
    ]
}

/// Three meetings: the word "layoffs" only in a transcript, "budget" only in a summary, and
/// "roadmap" in a title of a recording the user hid.
fn seed(store: &dyn Store) {
    let mut changes = Vec::new();
    changes.extend(meeting(
        "a",
        30,
        "Weekly sync",
        "we discussed layoffs",
        "Decisions",
    ));
    changes.extend(meeting("b", 20, "Planning", "hello", "The budget is set"));
    changes.extend(meeting(
        "c",
        10,
        "Roadmap review",
        "roadmap talk",
        "Roadmap notes",
    ));
    changes.push(Change::Assistant(AssistantChange::RecordingHidden {
        id: rid("c"),
        hidden: true,
    }));
    store.apply(changes).unwrap();
}

fn query(text: Option<&str>, exposure: Exposure) -> AssistantQuery {
    AssistantQuery {
        text: text.map(Into::into),
        fields: exposure.fields(),
        since: None,
        until: None,
        after: None,
        limit: 25,
    }
}

fn ids(read: &dyn StoreRead, q: &AssistantQuery) -> Vec<String> {
    read.assistant_recordings(q)
        .unwrap()
        .into_iter()
        .map(|r| r.recording.id.to_string())
        .collect()
}

fn check_audience(read: &dyn StoreRead) {
    let none: Vec<String> = Vec::new();
    assert_eq!(
        ids(read, &query(None, Exposure::Titles)),
        ["a", "b"],
        "hidden c never listed"
    );
    assert_eq!(ids(read, &query(Some("layoffs"), Exposure::Titles)), none);
    assert_eq!(
        ids(read, &query(Some("layoffs"), Exposure::Summaries)),
        none
    );
    assert_eq!(
        ids(read, &query(Some("LAYOFFS"), Exposure::Transcripts)),
        ["a"]
    );
    assert_eq!(ids(read, &query(Some("budget"), Exposure::Titles)), none);
    assert_eq!(
        ids(read, &query(Some("budget"), Exposure::Summaries)),
        ["b"]
    );
    assert_eq!(
        ids(read, &query(Some("roadmap"), Exposure::Transcripts)),
        none
    );
    assert!(read.hidden_from_assistants(&rid("c")).unwrap());
    assert!(!read.hidden_from_assistants(&rid("a")).unwrap());
    assert!(!read.hidden_from_assistants(&rid("nope")).unwrap());

    let mut one = query(None, Exposure::Transcripts);
    one.limit = 1;
    assert_eq!(ids(read, &one), ["a"], "limit counts only visible rows");
    one.after = Some((Timestamp(30), rid("a")));
    assert_eq!(ids(read, &one), ["b"]);
    one.after = Some((Timestamp(20), rid("b")));
    assert_eq!(
        ids(read, &one),
        none,
        "the keyset pages without gaps or repeats"
    );

    let mut window = query(None, Exposure::Titles);
    window.since = Some(Timestamp(20));
    window.until = Some(Timestamp(30));
    assert_eq!(
        ids(read, &window),
        ["b"],
        "since is inclusive and until exclusive"
    );

    let mut title_only = query(Some("sync"), Exposure::Titles);
    assert_eq!(ids(read, &title_only), ["a"]);
    title_only.fields = Fields {
        title: false,
        summary: false,
        transcript: false,
    };
    assert_eq!(
        ids(read, &title_only),
        none,
        "an empty mask matches no text"
    );
}

#[test]
fn mem_store_scopes_assistant_search() {
    let store = MemStore::new();
    seed(&store);
    check_audience(&store);
}

#[test]
fn sqlite_store_and_reader_scope_assistant_search() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dory.sqlite");
    let store = SqliteStore::open(&path).unwrap();
    seed(&store);
    check_audience(&store);
    check_audience(&SqliteReader::open(&path).unwrap());
}

fn client(label: &str, exposure: Exposure) -> AssistantClient {
    AssistantClient {
        label: label.into(),
        exposure,
        writes: Writes::Never,
        added_at: Timestamp(5),
    }
}

fn check_access(store: &dyn Store) {
    let key = ClientKey::from_bytes([7; 32]);
    let other = ClientKey::from_bytes([8; 32]);
    assert!(!store.assistant_access().unwrap().enabled, "off by default");
    store
        .apply(vec![
            Change::Assistant(AssistantChange::Enabled(true)),
            Change::Assistant(AssistantChange::ClientAdded {
                client: client("Claude", Exposure::Titles),
                key: key.clone(),
            }),
        ])
        .unwrap();
    let access = store.assistant_access().unwrap();
    assert!(access.enabled);
    assert_eq!(access.clients, [client("Claude", Exposure::Titles)]);
    assert_eq!(
        store.assistant_client(&key).unwrap(),
        Some(client("Claude", Exposure::Titles))
    );
    assert_eq!(store.assistant_client(&other).unwrap(), None);

    for duplicate in [
        AssistantChange::ClientAdded {
            client: client("Claude", Exposure::Titles),
            key: other.clone(),
        },
        AssistantChange::ClientAdded {
            client: client("Other", Exposure::Titles),
            key: key.clone(),
        },
    ] {
        assert!(store.apply(vec![Change::Assistant(duplicate)]).is_err());
    }

    store
        .apply(vec![Change::Assistant(AssistantChange::ClientChanged(
            AssistantClient {
                writes: Writes::Ask,
                ..client("Claude", Exposure::Transcripts)
            },
        ))])
        .unwrap();
    let changed = store.assistant_client(&key).unwrap().unwrap();
    assert_eq!(
        (changed.exposure, changed.writes),
        (Exposure::Transcripts, Writes::Ask)
    );

    store
        .apply(vec![Change::Assistant(AssistantChange::ClientRemoved {
            label: "Claude".into(),
        })])
        .unwrap();
    assert_eq!(store.assistant_client(&key).unwrap(), None);
    assert!(matches!(
        store.apply(vec![Change::Assistant(AssistantChange::ClientRemoved {
            label: "Claude".into(),
        })]),
        Err(StoreError::NotFound(_))
    ));
    assert!(matches!(
        store.apply(vec![Change::Assistant(AssistantChange::RecordingHidden {
            id: rid("nope"),
            hidden: true,
        })]),
        Err(StoreError::NotFound(_))
    ));
}

#[test]
fn mem_store_keeps_assistant_access() {
    check_access(&MemStore::new());
}

#[test]
fn sqlite_store_keeps_assistant_access_and_only_a_key_digest() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dory.sqlite");
    let store = SqliteStore::open(&path).unwrap();
    check_access(&store);
    let key = ClientKey::from_bytes([9; 32]);
    store
        .apply(vec![Change::Assistant(AssistantChange::ClientAdded {
            client: client("Kept", Exposure::Summaries),
            key: key.clone(),
        })])
        .unwrap();
    drop(store);
    let bytes = std::fs::read(&path).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        !text.contains(key.as_str()),
        "the key itself is never stored"
    );
    let reader = SqliteReader::open(&path).unwrap();
    assert_eq!(
        reader.assistant_client(&key).unwrap().map(|c| c.label),
        Some("Kept".to_owned())
    );
}

#[test]
fn v4_to_v5_keeps_every_row() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dory.sqlite");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        for (n, sql) in [
            include_str!("../src/sqlite/schema_v1.sql"),
            include_str!("../src/sqlite/schema_v2.sql"),
            include_str!("../src/sqlite/schema_v3.sql"),
            include_str!("../src/sqlite/schema_v4.sql"),
        ]
        .iter()
        .enumerate()
        {
            conn.execute_batch(sql).unwrap();
            conn.execute_batch(&format!("PRAGMA user_version = {}", n + 1))
                .unwrap();
        }
        conn.execute(
            "INSERT INTO recordings (id, title, created_at, layout) VALUES ('old', 'Old', 1, 'mic_system')",
            [],
        )
        .unwrap();
    }
    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), 5);
    let rows = store
        .assistant_recordings(&query(None, Exposure::Titles))
        .unwrap();
    assert_eq!(rows.len(), 1, "an old recording is visible until hidden");
    assert_eq!(rows[0].recording.title.as_deref(), Some("Old"));
}
