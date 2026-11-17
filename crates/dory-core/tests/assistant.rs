use std::sync::{Arc, Mutex};

use dory_core::assistant::turns::TurnCursor;
use dory_core::assistant::{Assistant, Field, ItemScope, Search};
use dory_core::domain::{
    Access, AssistantChange, AssistantClient, AuditEntry, AuditOutcome, Channel, ChannelLayout,
    ClientKey, Exposure, LangTag, LanguagePref, Ms, Recording, RecordingId, Refusal, Revision,
    Segment, SpeakerId, Summary, TimeRange, Timestamp, Transcript, Writes,
};
use dory_core::models::ModelRef;
use dory_core::ports::{AuditError, AuditSink, Change, Store};
use dory_core::testing::ManualClock;
use dory_core::testing::store::MemStore;

#[derive(Default)]
struct Log {
    entries: Mutex<Vec<AuditEntry>>,
    broken: Mutex<bool>,
}

impl AuditSink for Log {
    fn append(&self, entry: &AuditEntry) -> Result<(), AuditError> {
        if *self.broken.lock().unwrap() {
            return Err(AuditError::Io(std::io::Error::other("disk full")));
        }
        self.entries.lock().unwrap().push(entry.clone());
        Ok(())
    }
}

fn rid(s: &str) -> RecordingId {
    RecordingId::parse(s).unwrap()
}

fn meeting(id: &str, at: i64, title: &str, lines: &[&str], summary: &str) -> Vec<Change> {
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
                segments: lines
                    .iter()
                    .enumerate()
                    .map(|(i, text)| Segment {
                        range: TimeRange::new(Ms(i as u64 * 1000), Ms(i as u64 * 1000 + 900)),
                        speaker: SpeakerId(1 + (i % 2) as u16),
                        channel: Channel::System,
                        text: (*text).into(),
                    })
                    .collect(),
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

struct Rig {
    store: Arc<MemStore>,
    log: Arc<Log>,
    key: ClientKey,
}

impl Rig {
    fn new(exposure: Exposure) -> Rig {
        let store = Arc::new(MemStore::new());
        let key = ClientKey::from_bytes([1; 32]);
        let mut changes = Vec::new();
        changes.extend(meeting(
            "sync",
            30,
            "Weekly sync",
            &["We talked about layoffs.", "The release ships Friday."],
            "## Key points\n- Release\n## Decisions\n- Ship Friday\n## Action items\n- Ana: notes\n- Bo: tests\n",
        ));
        changes.extend(meeting(
            "plan",
            20,
            "Planning",
            &["hello"],
            "## Points\n- The budget is set\n## Decisions\nNone.\n## Actions\nNone.\n",
        ));
        changes.extend(meeting("secret", 10, "Secret", &["layoffs again"], "Notes"));
        changes.extend([
            Change::Assistant(AssistantChange::RecordingHidden {
                id: rid("secret"),
                hidden: true,
            }),
            Change::Assistant(AssistantChange::Enabled(true)),
            Change::Assistant(AssistantChange::ClientAdded {
                client: AssistantClient {
                    label: "Claude".into(),
                    exposure,
                    writes: Writes::Never,
                    added_at: Timestamp(0),
                },
                key: key.clone(),
            }),
        ]);
        store.apply(changes).unwrap();
        Rig {
            store,
            log: Arc::default(),
            key,
        }
    }

    fn assistant(&self, key: Option<ClientKey>) -> Assistant {
        Assistant::new(
            self.store.clone(),
            self.log.clone(),
            Arc::new(ManualClock::new(Timestamp(99))),
            key,
        )
    }

    fn me(&self) -> Assistant {
        self.assistant(Some(self.key.clone()))
    }

    fn entries(&self) -> Vec<AuditEntry> {
        self.log.entries.lock().unwrap().clone()
    }

    fn set_exposure(&self, exposure: Exposure) {
        self.store
            .apply(vec![Change::Assistant(AssistantChange::ClientChanged(
                AssistantClient {
                    label: "Claude".into(),
                    exposure,
                    writes: Writes::Never,
                    added_at: Timestamp(0),
                },
            ))])
            .unwrap();
    }
}

fn ids(page: &dory_core::assistant::SearchPage) -> Vec<String> {
    page.hits.iter().map(|h| h.id.to_string()).collect()
}

fn search(text: &str) -> Search {
    Search {
        text: Some(text.into()),
        ..Search::default()
    }
}

#[test]
fn every_read_is_audited_before_it_returns() {
    let rig = Rig::new(Exposure::Transcripts);
    let a = rig.me();
    a.search(Some("claude-code"), &search("layoffs")).unwrap();
    a.summary(None, &rid("sync")).unwrap();
    assert_eq!(a.summary(None, &rid("nope")), Err(Refusal::NotFound));
    let entries = rig.entries();
    assert_eq!(entries.len(), 3, "refusals are audited too");
    assert_eq!(entries[0].client.as_deref(), Some("Claude"));
    assert_eq!(entries[0].reported_name.as_deref(), Some("claude-code"));
    assert_eq!(
        entries[0].access,
        Access::Search {
            text: "layoffs".into(),
            hits: 1
        }
    );
    assert_eq!(entries[1].outcome, AuditOutcome::Served { items: 1 });
    assert_eq!(entries[2].outcome, AuditOutcome::Refused(Refusal::NotFound));
    for e in &entries {
        let text = format!("{e:?}");
        assert!(!text.contains("talked about"), "no meeting text in the log");
    }
}

#[test]
fn an_audit_failure_refuses_the_read() {
    let rig = Rig::new(Exposure::Transcripts);
    *rig.log.broken.lock().unwrap() = true;
    assert!(matches!(
        rig.me().summary(None, &rid("sync")),
        Err(Refusal::Library(_))
    ));
}

#[test]
fn a_hidden_recording_reads_as_not_found() {
    let rig = Rig::new(Exposure::Transcripts);
    let a = rig.me();
    assert_eq!(
        ids(&a.search(None, &Search::default()).unwrap()),
        ["sync", "plan"]
    );
    assert_eq!(
        ids(&a.search(None, &search("secret")).unwrap()),
        Vec::<String>::new()
    );
    for id in ["secret", "made-up"] {
        assert_eq!(a.recording(None, &rid(id)), Err(Refusal::NotFound), "{id}");
        assert_eq!(a.summary(None, &rid(id)), Err(Refusal::NotFound), "{id}");
        assert_eq!(
            a.transcript_page(None, &rid(id), None, None),
            Err(Refusal::NotFound),
            "{id}"
        );
    }
}

#[test]
fn each_level_reads_only_what_it_may() {
    let rig = Rig::new(Exposure::Titles);
    let a = rig.me();
    let sync = rid("sync");
    assert!(a.recording(None, &sync).unwrap().speakers.is_none());
    assert_eq!(
        a.summary(None, &sync),
        Err(Refusal::NotShared {
            need: Exposure::Summaries
        })
    );
    assert_eq!(
        a.action_items(None, &ItemScope::default()),
        Err(Refusal::NotShared {
            need: Exposure::Summaries
        })
    );
    assert!(a.search(None, &search("layoffs")).unwrap().hits.is_empty());
    assert!(a.search(None, &search("budget")).unwrap().hits.is_empty());

    rig.set_exposure(Exposure::Summaries);
    assert_eq!(
        a.recording(None, &sync).unwrap().speakers,
        Some(vec!["Speaker 1".into(), "Speaker 2".into()])
    );
    assert!(
        a.summary(None, &sync).is_ok(),
        "the change applies on the next call"
    );
    let budget = a.search(None, &search("budget")).unwrap();
    assert_eq!(ids(&budget), ["plan"]);
    assert_eq!(budget.hits[0].snippet.as_ref().unwrap().0, Field::Summary);
    assert!(a.search(None, &search("layoffs")).unwrap().hits.is_empty());
    assert_eq!(
        a.transcript_page(None, &sync, None, None),
        Err(Refusal::NotShared {
            need: Exposure::Transcripts
        })
    );

    rig.set_exposure(Exposure::Transcripts);
    let layoffs = a.search(None, &search("LAYOFFS")).unwrap();
    assert_eq!(ids(&layoffs), ["sync"]);
    assert_eq!(
        layoffs.hits[0].snippet.as_ref().unwrap().0,
        Field::Transcript
    );
}

#[test]
fn revoking_and_turning_off_apply_on_the_next_call() {
    let rig = Rig::new(Exposure::Summaries);
    let a = rig.me();
    assert!(a.summary(None, &rid("sync")).is_ok());
    rig.store
        .apply(vec![Change::Assistant(AssistantChange::Enabled(false))])
        .unwrap();
    assert_eq!(a.summary(None, &rid("sync")), Err(Refusal::ServerOff));
    assert_eq!(
        rig.entries().last().unwrap().client.as_deref(),
        Some("Claude"),
        "a known key is named even while access is off"
    );
    rig.store
        .apply(vec![
            Change::Assistant(AssistantChange::Enabled(true)),
            Change::Assistant(AssistantChange::ClientRemoved {
                label: "Claude".into(),
            }),
        ])
        .unwrap();
    assert_eq!(a.summary(None, &rid("sync")), Err(Refusal::UnknownKey));
    assert_eq!(
        rig.assistant(None).summary(Some("x"), &rid("sync")),
        Err(Refusal::NoKey)
    );
    let last = rig.entries().pop().unwrap();
    assert_eq!(last.client, None);
    assert_eq!(last.reported_name.as_deref(), Some("x"));
}

#[test]
fn action_items_come_from_the_third_section() {
    let rig = Rig::new(Exposure::Summaries);
    let found = rig.me().action_items(None, &ItemScope::default()).unwrap();
    let got: Vec<_> = found
        .iter()
        .map(|m| (m.id.to_string(), m.items.clone()))
        .collect();
    assert_eq!(
        got,
        [
            (
                "sync".to_owned(),
                Some(vec!["Ana: notes".to_owned(), "Bo: tests".to_owned()])
            ),
            ("plan".to_owned(), Some(vec![])),
        ]
    );
}

#[test]
fn transcript_pages_follow_their_cursor_and_refuse_a_stale_one() {
    let rig = Rig::new(Exposure::Transcripts);
    let a = rig.me();
    let page = a.transcript_page(None, &rid("sync"), None, None).unwrap();
    assert_eq!((page.first, page.total, page.turns.len()), (0, 2, 2));
    assert_eq!(page.next, None);
    let stale = TurnCursor {
        revision: Revision(9),
        turn: 0,
    };
    assert_eq!(
        a.transcript_page(None, &rid("sync"), Some(stale), None),
        Err(Refusal::StaleRevision)
    );
    let past_end = TurnCursor {
        revision: Revision(1),
        turn: 50,
    };
    let empty = a
        .transcript_page(None, &rid("sync"), Some(past_end), None)
        .unwrap();
    assert!(empty.turns.is_empty() && empty.next.is_none());
}

#[test]
fn search_pages_with_its_cursor() {
    let rig = Rig::new(Exposure::Titles);
    let a = rig.me();
    let first = a
        .search(
            None,
            &Search {
                limit: Some(1),
                ..Search::default()
            },
        )
        .unwrap();
    assert_eq!(ids(&first), ["sync"]);
    let second = a
        .search(
            None,
            &Search {
                limit: Some(1),
                cursor: first.next.clone(),
                ..Search::default()
            },
        )
        .unwrap();
    assert_eq!(ids(&second), ["plan"]);
    let third = a
        .search(
            None,
            &Search {
                limit: Some(1),
                cursor: second.next.clone(),
                ..Search::default()
            },
        )
        .unwrap();
    assert!(third.hits.is_empty() && third.next.is_none());
}
