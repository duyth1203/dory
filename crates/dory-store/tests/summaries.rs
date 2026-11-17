use dory_core::domain::{
    ChannelLayout, LangTag, LanguagePref, Recording, RecordingId, Revision, Summary, Timestamp,
};
use dory_core::models::ModelRef;
use dory_core::ports::{Change, Store, StoreError, StoreRead};
use dory_core::testing::store::MemStore;
use dory_store::SqliteStore;

fn rid() -> RecordingId {
    RecordingId::parse("sync").unwrap()
}

fn summary(text: &str) -> Summary {
    Summary {
        markdown: text.into(),
        language: Some(LangTag::parse("en").unwrap()),
        produced_by: ModelRef::parse("fake/llm").unwrap(),
        transcript_revision: Revision(1),
    }
}

fn replace(store: &dyn Store, text: &str) {
    store
        .apply(vec![Change::SummaryReplaced {
            id: rid(),
            summary: summary(text),
        }])
        .unwrap();
}

fn current(store: &dyn Store) -> (Option<String>, Option<String>) {
    (
        store.summary(&rid()).unwrap().map(|s| s.markdown),
        store.previous_summary(&rid()).unwrap().map(|s| s.markdown),
    )
}

fn check(store: &dyn Store) {
    store
        .apply(vec![Change::RecordingCreated(Recording {
            id: rid(),
            title: None,
            created_at: Timestamp(1),
            language: LanguagePref::Auto,
            layout: ChannelLayout::MicSystem,
            take: None,
        })])
        .unwrap();
    assert!(matches!(
        store.apply(vec![Change::SummaryRestored(rid())]),
        Err(StoreError::NotFound(_))
    ));
    replace(store, "first");
    assert_eq!(current(store), (Some("first".into()), None));
    replace(store, "second");
    replace(store, "third");
    assert_eq!(
        current(store),
        (Some("third".into()), Some("second".into())),
        "one previous summary is kept"
    );
    store.apply(vec![Change::SummaryRestored(rid())]).unwrap();
    assert_eq!(
        current(store),
        (Some("second".into()), Some("third".into()))
    );
    store.apply(vec![Change::SummaryRestored(rid())]).unwrap();
    assert_eq!(
        current(store),
        (Some("third".into()), Some("second".into())),
        "restoring twice undoes it"
    );

    assert_eq!(store.summary_instruction(&rid()).unwrap(), None);
    store
        .apply(vec![Change::SummaryInstructionSet {
            id: rid(),
            instruction: Some("focus on the budget".into()),
        }])
        .unwrap();
    assert_eq!(
        store.summary_instruction(&rid()).unwrap().as_deref(),
        Some("focus on the budget")
    );
    store
        .apply(vec![Change::SummaryInstructionSet {
            id: rid(),
            instruction: None,
        }])
        .unwrap();
    assert_eq!(store.summary_instruction(&rid()).unwrap(), None);
}

#[test]
fn mem_store_keeps_one_previous_summary_and_an_instruction() {
    check(&MemStore::new());
}

#[test]
fn sqlite_store_keeps_one_previous_summary_and_an_instruction() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(dir.path().join("dory.sqlite")).unwrap();
    check(&store);
    let search = |text: &str| {
        store
            .recordings(&dory_core::domain::RecordingQuery {
                text: Some(text.into()),
                limit: None,
            })
            .unwrap()
            .len()
    };
    store.apply(vec![Change::SummaryRestored(rid())]).unwrap();
    assert_eq!(
        (search("second"), search("third")),
        (1, 0),
        "search follows the restored summary"
    );
}
