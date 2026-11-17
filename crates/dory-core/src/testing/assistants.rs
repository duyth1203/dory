//! What an assistant's search may find, run against any store: [`seed`] writes a small library and
//! three assistants, one per exposure, and [`check_search`] searches it through [`Assistant`]
//! over a read port. A store passes when no grant finds, marks, or corrects toward a word it
//! could not read, and a hidden recording never shows.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::assistant::turns::TurnCursor;
use crate::assistant::{Assistant, Field, Hit, Search, SearchCursor, SearchPage};
use crate::domain::{
    Access, AssistantChange, AssistantClient, AuditEntry, Channel, ChannelLayout, ClientKey,
    Exposure, LangTag, LanguagePref, Ms, Recording, RecordingId, Revision, Segment, SpeakerId,
    SpeakerNames, Summary, TimeRange, Timestamp, Transcript, Writes,
};
use crate::models::ModelRef;
use crate::ports::{AuditError, AuditSink, Change, Store, StoreRead};
use crate::search::Section;

use super::ManualClock;

/// The key of the assistant [`seed`] added at `exposure`.
pub fn key(exposure: Exposure) -> ClientKey {
    ClientKey::from_bytes([exposure as u8 + 1; 32])
}

fn rid(s: &str) -> RecordingId {
    RecordingId::parse(s).unwrap()
}

fn meeting(id: &str, at: i64, title: &str, said: &[&str], summary: &str) -> Vec<Change> {
    let model = ModelRef::parse("fake/stt").unwrap();
    vec![
        Change::RecordingCreated(Recording {
            id: rid(id),
            title: Some(title.into()),
            created_at: Timestamp(at),
            language: LanguagePref::Auto,
            layout: ChannelLayout::MicSystem,
            take: None,
        }),
        Change::TranscriptReplaced {
            id: rid(id),
            transcript: Transcript {
                revision: Revision(1),
                segments: said
                    .iter()
                    .enumerate()
                    .map(|(i, text)| Segment {
                        range: TimeRange::new(Ms(i as u64 * 5000), Ms(i as u64 * 5000 + 4000)),
                        speaker: SpeakerId(1 + i as u16),
                        channel: Channel::System,
                        text: (*text).into(),
                    })
                    .collect(),
                detected_languages: vec![],
                produced_by: model.clone(),
            },
        },
        Change::SummaryReplaced {
            id: rid(id),
            summary: Summary {
                markdown: summary.into(),
                language: Some(LangTag::parse("en").unwrap()),
                produced_by: model,
                transcript_revision: Revision(1),
            },
        },
    ]
}

/// Three meetings, newest first. "zanzibar" and "budgets" are said only in transcripts, "budget"
/// is only in a summary, the second speaker of `sync` is named "José Müller", and every word of
/// `secret`, "releases" among them, is in a recording hidden from assistants. A typo one edit
/// from two words, such as "budgetz" or "releasez", shows whether a correction names a word the
/// assistant could not read beside one it could.
pub fn seed(store: &dyn Store) {
    let mut changes = Vec::new();
    changes.extend(meeting(
        "sync",
        30,
        "Weekly sync",
        &[
            "Welcome everyone.",
            "The Zanzibar layoffs start Friday, budgets too.",
        ],
        "## Key points\n- Release plan\n## Decisions\n- Ship Friday\n",
    ));
    changes.push(Change::SpeakerNamesReplaced {
        id: rid("sync"),
        names: SpeakerNames {
            for_revision: Revision(1),
            names: BTreeMap::from([(SpeakerId(2), "José Müller".to_owned())]),
        },
    });
    changes.extend(meeting(
        "plan",
        20,
        "Planning",
        &["hello, zanzibar again"],
        "## Points\n- The **budget** is approved\n",
    ));
    changes.extend(meeting(
        "secret",
        10,
        "Roadmap offsite",
        &["kilimanjaro trip"],
        "Okavango notes and releases",
    ));
    changes.push(Change::Assistant(AssistantChange::RecordingHidden {
        id: rid("secret"),
        hidden: true,
    }));
    changes.push(Change::Assistant(AssistantChange::Enabled(true)));
    for exposure in Exposure::ALL {
        changes.push(Change::Assistant(AssistantChange::ClientAdded {
            client: AssistantClient {
                label: exposure.as_str().into(),
                exposure,
                writes: Writes::Never,
                added_at: Timestamp(0),
            },
            key: key(exposure),
        }));
    }
    store.apply(changes).unwrap();
}

#[derive(Default)]
struct Log(Mutex<Vec<AuditEntry>>);

impl AuditSink for Log {
    fn append(&self, entry: &AuditEntry) -> Result<(), AuditError> {
        self.0.lock().unwrap().push(entry.clone());
        Ok(())
    }
}

struct Asker {
    assistant: Assistant,
    log: Arc<Log>,
}

impl Asker {
    fn new(read: &Arc<dyn StoreRead>, exposure: Exposure) -> Asker {
        let log = Arc::new(Log::default());
        let assistant = Assistant::new(
            read.clone(),
            log.clone(),
            Arc::new(ManualClock::new(Timestamp(99))),
            Some(key(exposure)),
        );
        Asker { assistant, log }
    }

    fn page(&self, text: &str, limit: Option<u32>, cursor: Option<SearchCursor>) -> SearchPage {
        self.assistant
            .search(
                None,
                &Search {
                    text: Some(text.into()),
                    limit,
                    cursor,
                    ..Search::default()
                },
            )
            .unwrap()
    }

    fn search(&self, text: &str) -> SearchPage {
        self.page(text, None, None)
    }
}

fn ids(page: &SearchPage) -> Vec<String> {
    page.hits.iter().map(|h| h.id.to_string()).collect()
}

fn corrections(page: &SearchPage) -> Vec<(String, Vec<String>)> {
    page.corrections
        .iter()
        .map(|c| (c.typed.clone(), c.used.clone()))
        .collect()
}

/// The snippet's field and the words its marks cover.
fn marked(hit: &Hit) -> (Field, Vec<String>) {
    let cut = hit
        .snippet
        .as_ref()
        .expect("a search hit carries a snippet");
    (
        cut.field,
        cut.snippet
            .marks
            .iter()
            .map(|m| cut.snippet.text[m.clone()].to_owned())
            .collect(),
    )
}

/// Finds nothing, and names no correction, for every text in `texts`.
fn finds_nothing(asker: &Asker, texts: &[&str], why: &str) {
    for text in texts {
        let page = asker.search(text);
        assert_eq!(
            (ids(&page), corrections(&page)),
            (Vec::<String>::new(), Vec::new()),
            "{text:?}: {why}"
        );
    }
}

fn no_words() -> Vec<String> {
    Vec::new()
}

pub fn check_search(read: Arc<dyn StoreRead>) {
    let titles = Asker::new(&read, Exposure::Titles);
    let summaries = Asker::new(&read, Exposure::Summaries);
    let transcripts = Asker::new(&read, Exposure::Transcripts);

    for asker in [&titles, &summaries, &transcripts] {
        finds_nothing(
            asker,
            &["roadmap", "roadmp", "kilimanjaro", "kilimanjro", "okavngo"],
            "a hidden recording lends no hit and no correction",
        );
    }

    finds_nothing(
        &titles,
        &[
            "budget", "budgt", "budgetz", "zanzibar", "zanzibr", "muller", "mullr",
        ],
        "a Titles grant reads titles only",
    );
    let weekly = titles.search("SYNC week");
    assert_eq!(ids(&weekly), ["sync"], "any order, any case");
    assert_eq!(weekly.hits[0].section, Some(Section::Titles));
    assert_eq!(
        marked(&weekly.hits[0]),
        (Field::Title, vec!["Weekly".into(), "sync".into()])
    );

    finds_nothing(
        &summaries,
        &["zanzibar", "zanzibr", "layoffs", "layofs"],
        "a word only a transcript holds is not found, marked, or corrected toward",
    );
    let budget = summaries.search("budgt");
    assert_eq!(ids(&budget), ["plan"]);
    assert_eq!(
        corrections(&budget),
        [("budgt".into(), vec!["budget".into()])],
        "a summary word corrects a typo under Summaries"
    );
    assert_eq!(budget.hits[0].section, Some(Section::Conversations));
    assert_eq!(
        marked(&budget.hits[0]),
        (Field::Summary, vec!["budget".into()]),
        "the snippet drops Markdown"
    );
    assert_eq!(budget.hits[0].snippet.as_ref().unwrap().turn, None);
    assert_eq!(
        corrections(&summaries.search("budgetz")),
        [("budgetz".into(), vec!["budget".into()])],
        "budgets is said only in a transcript, so the correction never names it"
    );
    assert_eq!(
        corrections(&transcripts.search("budgetz")),
        [("budgetz".into(), vec!["budget".into(), "budgets".into()])],
        "a Transcripts grant reads it"
    );
    for asker in [&summaries, &transcripts] {
        assert_eq!(
            corrections(&asker.search("releasez")),
            [("releasez".into(), vec!["release".into()])],
            "releases is only in a hidden recording, so the correction never names it"
        );
    }
    let friday = summaries.search("friday");
    assert_eq!(ids(&friday), ["sync"]);
    assert_eq!(
        marked(&friday.hits[0]),
        (Field::Summary, vec!["Friday".into()]),
        "the snippet comes from the summary even though the transcript says it first"
    );
    for text in ["muller", "Müller josé", "MULLER"] {
        let named = summaries.search(text);
        assert_eq!(ids(&named), ["sync"], "{text}: a speaker name");
        assert_eq!(marked(&named.hits[0]).0, Field::Speakers, "{text}");
    }
    assert_eq!(
        corrections(&summaries.search("mullr")),
        [("mullr".into(), vec!["muller".into()])],
        "a speaker name corrects a typo under Summaries"
    );

    let zanzibar = transcripts.search("zanzibr");
    assert_eq!(ids(&zanzibar), ["sync", "plan"], "newest first");
    assert_eq!(
        corrections(&zanzibar),
        [("zanzibr".into(), vec!["zanzibar".into()])]
    );
    let first = &zanzibar.hits[0];
    assert_eq!(first.section, Some(Section::Conversations));
    assert_eq!(marked(first), (Field::Transcript, vec!["Zanzibar".into()]));
    let turn = first.snippet.as_ref().unwrap().turn.as_ref().unwrap();
    assert_eq!(
        (turn.cursor, turn.speaker.as_str(), turn.start),
        (
            TurnCursor {
                revision: Revision(1),
                turn: 1
            },
            "José Müller",
            Some(Ms(5000))
        ),
        "the turn to read around, by the name the user gave"
    );
    for text in ["layoffs zanzibar", "\"zanzibar layoffs\"", "zanz lay"] {
        assert_eq!(ids(&transcripts.search(text)), ["sync"], "{text}");
    }
    let folded = transcripts.search("ZÁNZÍBAR");
    assert_eq!(ids(&folded), ["sync", "plan"], "case and accents fold");
    assert!(folded.corrections.is_empty());
    finds_nothing(
        &transcripts,
        &["\"layoffs zanzibar\""],
        "a quoted phrase keeps its order",
    );
    assert_eq!(
        marked(&transcripts.search("friday").hits[0]).0,
        Field::Transcript
    );

    let one = transcripts.page("zanzibr", Some(1), None);
    assert_eq!(ids(&one), ["sync"]);
    let two = transcripts.page("zanzibr", Some(1), one.next.clone());
    assert_eq!(
        (ids(&two), corrections(&two)),
        (vec!["plan".into()], corrections(&one)),
        "the next page of a corrected search is corrected the same way"
    );
    let three = transcripts.page("zanzibr", Some(1), two.next.clone());
    assert_eq!(ids(&three), no_words());

    let entries = transcripts.log.0.lock().unwrap();
    assert_eq!(
        entries.last().map(|e| e.access.clone()),
        Some(Access::search("zanzibr", 0)),
        "every search is audited with the text and the hit count"
    );
}
