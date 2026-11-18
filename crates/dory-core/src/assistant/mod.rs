//! What an assistant may read, and the one read path it goes through.
//!
//! [`Assistant`] is all a driving adapter such as `dory-mcp` sees of the library. Every call
//! resolves the assistant's grant from the store, so a change the user makes applies on the next
//! call. It reads only what the grant allows, treats a hidden recording as missing, and writes an
//! audit entry before it returns. If the entry cannot be written, the read is refused.

pub mod policy;
pub mod turns;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::app::find;
use crate::domain::{
    Access, Actor, AssistantQuery, AuditEntry, AuditOutcome, ChannelLayout, ClientKey, Exposure,
    LangTag, LanguagePref, Ms, Recording, RecordingId, Refusal, RemoteCommand, RemoteOutcome,
    RemoteRequest, Revision, Sections, SpeakerNames, SummaryFlag, Timestamp, list_items, sections,
};
use crate::models::ModelRef;
use crate::ports::{AuditSink, Clock, Remote, StoreError, StoreRead};
use crate::search::{self, Correction, Found, Query, Section, Snippet, Source};
use policy::Grant;
use turns::{Turn, TurnCursor};

pub const MAX_SEARCH_RESULTS: u32 = 25;
pub const DEFAULT_SEARCH_RESULTS: u32 = 10;
pub const MAX_ITEM_MEETINGS: u32 = 20;
/// Characters a search snippet aims for: enough for an assistant to judge the hit, while the
/// turn it came from is one transcript page away.
pub const SNIPPET_CHARS: usize = 200;

/// A search on behalf of an assistant.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Search {
    pub text: Option<String>,
    pub since: Option<Timestamp>,
    pub until: Option<Timestamp>,
    pub cursor: Option<SearchCursor>,
    pub limit: Option<u32>,
}

/// Where the next page of search results starts: after this recording, newest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchCursor {
    pub created_at: Timestamp,
    pub id: RecordingId,
}

impl SearchCursor {
    pub fn parse(s: &str) -> Option<SearchCursor> {
        let (at, id) = s.strip_prefix('c')?.split_once('.')?;
        Some(SearchCursor {
            created_at: Timestamp(at.parse().ok()?),
            id: RecordingId::parse(id).ok()?,
        })
    }
}

impl std::fmt::Display for SearchCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "c{}.{}", self.created_at.0, self.id)
    }
}

/// Which text a snippet was cut from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Title,
    /// A name the user gave a speaker.
    Speakers,
    Summary,
    Transcript,
}

impl Field {
    pub fn as_str(self) -> &'static str {
        match self {
            Field::Title => "title",
            Field::Speakers => "speakers",
            Field::Summary => "summary",
            Field::Transcript => "transcript",
        }
    }
}

/// The turn of [`Assistant::transcript_page`] a transcript snippet was cut from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnAt {
    /// Starts a transcript page at this turn.
    pub cursor: TurnCursor,
    pub speaker: String,
    /// `None` for transcripts imported without times.
    pub start: Option<Ms>,
}

/// Text around a match, cut only from a field this assistant may read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HitSnippet {
    pub field: Field,
    /// `marks` are the byte ranges of `text` that hold matched words.
    pub snippet: Snippet,
    pub turn: Option<TurnAt>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub id: RecordingId,
    pub title: Option<String>,
    pub created_at: Timestamp,
    pub duration: Option<Ms>,
    pub has_transcript: bool,
    pub summary: SummaryFlag,
    /// Where the search matched: the title alone, or the conversation. `None` in a listing.
    pub section: Option<Section>,
    pub snippet: Option<HitSnippet>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchPage {
    pub hits: Vec<Hit>,
    pub next: Option<SearchCursor>,
    /// When nothing matched the words as typed, each word the search replaced, and with what.
    pub corrections: Vec<Correction>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordingView {
    pub id: RecordingId,
    pub title: Option<String>,
    pub created_at: Timestamp,
    pub duration: Option<Ms>,
    pub language: LanguagePref,
    pub layout: ChannelLayout,
    pub has_transcript: bool,
    pub summary: SummaryFlag,
    /// The speakers' names, at the Summaries level and above.
    pub speakers: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SummaryView {
    pub id: RecordingId,
    pub title: Option<String>,
    pub markdown: String,
    pub language: Option<LangTag>,
    /// Made from an older transcript than the one stored now.
    pub stale: bool,
    pub produced_by: ModelRef,
    /// What the user asked this summary to focus on, if anything.
    pub instruction: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptPage {
    pub id: RecordingId,
    pub title: Option<String>,
    pub revision: Revision,
    /// The language the translated turns are written in, when the transcript has a translation
    /// that applies.
    pub translated_into: Option<LangTag>,
    /// The index of the first turn on this page.
    pub first: u32,
    pub total: u32,
    pub turns: Vec<Turn>,
    pub next: Option<TurnCursor>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemScope {
    pub id: Option<RecordingId>,
    pub since: Option<Timestamp>,
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeetingItems {
    pub id: RecordingId,
    pub title: Option<String>,
    pub created_at: Timestamp,
    /// `None` when the summary does not have its three sections, so no section can be trusted
    /// to be the action items.
    pub items: Option<Vec<String>>,
    pub stale: bool,
}

pub struct Assistant {
    store: Arc<dyn StoreRead>,
    audit: Arc<dyn AuditSink>,
    clock: Arc<dyn Clock>,
    key: Option<ClientKey>,
    library_changed: AtomicBool,
    remote: Option<Arc<dyn Remote>>,
}

fn library(e: StoreError) -> Refusal {
    Refusal::Library(e.to_string())
}

impl Assistant {
    pub fn new(
        store: Arc<dyn StoreRead>,
        audit: Arc<dyn AuditSink>,
        clock: Arc<dyn Clock>,
        key: Option<ClientKey>,
    ) -> Assistant {
        Assistant {
            store,
            audit,
            clock,
            key,
            library_changed: AtomicBool::new(false),
            remote: None,
        }
    }

    /// Carries change requests to the running app. Without one, every change is refused as if
    /// Dory were not open.
    pub fn with_remote(mut self, remote: Arc<dyn Remote>) -> Assistant {
        self.remote = Some(remote);
        self
    }

    /// Asks the app to make a change. The app decides and asks the user; this only forwards the
    /// request and logs it.
    pub fn write(&self, reported: Option<&str>, command: RemoteCommand) -> RemoteOutcome {
        let client = self
            .key
            .as_ref()
            .and_then(|key| self.store.assistant_client(key).ok().flatten())
            .map(|c| c.label);
        let entry = |outcome: &RemoteOutcome| AuditEntry {
            at: self.clock.now(),
            actor: Actor::Helper,
            client: client.clone(),
            reported_name: reported.map(str::to_owned),
            access: Access::Write {
                what: command.what().to_owned(),
                id: Some(command.id().clone()),
            },
            outcome: match outcome {
                Ok(()) => AuditOutcome::Served { items: 1 },
                Err(why) => AuditOutcome::Refused(why.clone()),
            },
        };
        let outcome = match &self.remote {
            Some(remote) => remote.send(&RemoteRequest {
                key: self.key.clone(),
                reported_name: reported.map(str::to_owned),
                command: command.clone(),
            }),
            None => Err(Refusal::AppNotRunning),
        };
        if let Err(e) = self.audit.append(&entry(&outcome)) {
            eprintln!("assistant: could not log a change request: {e}");
        }
        outcome
    }

    /// True once a read found the library's schema changed under it, which happens when Dory was
    /// updated. The adapter should exit so its client restarts it from the new build.
    pub fn library_changed(&self) -> bool {
        self.library_changed.load(Ordering::Relaxed)
    }

    fn lib(&self, e: StoreError) -> Refusal {
        if matches!(
            e,
            StoreError::SchemaChanged { .. } | StoreError::SearchIndexStale
        ) {
            self.library_changed.store(true, Ordering::Relaxed);
        }
        library(e)
    }

    /// The client that holds this helper's key, and whether it may read now. The label comes
    /// back even when access is off, so the log names a known assistant.
    fn grant(&self) -> (Option<String>, Result<Grant, Refusal>) {
        let client = match &self.key {
            Some(key) => match self.store.assistant_client(key) {
                Ok(client) => client,
                Err(e) => return (None, Err(self.lib(e))),
            },
            None => None,
        };
        let label = client.as_ref().map(|c| c.label.clone());
        let grant = self
            .store
            .assistant_access()
            .map_err(|e| self.lib(e))
            .and_then(|access| policy::grant(access.enabled, self.key.is_some(), client));
        (label, grant)
    }

    /// Runs one read under the grant and records it, refusals included. A read whose entry
    /// cannot be written is refused.
    fn audited<T>(
        &self,
        reported: Option<&str>,
        access: impl FnOnce(Option<&T>) -> Access,
        items: impl FnOnce(&T) -> u32,
        read: impl FnOnce(&Grant) -> Result<T, Refusal>,
    ) -> Result<T, Refusal> {
        let (client, grant) = self.grant();
        let result = grant.and_then(|g| read(&g));
        let outcome = match &result {
            Ok(value) => AuditOutcome::Served {
                items: items(value),
            },
            Err(why) => AuditOutcome::Refused(why.clone()),
        };
        let entry = AuditEntry {
            at: self.clock.now(),
            actor: Actor::Helper,
            client,
            reported_name: reported.map(str::to_owned),
            access: access(result.as_ref().ok()),
            outcome,
        };
        match (self.audit.append(&entry), result) {
            (Ok(()), result) => result,
            (Err(_), Err(why)) => Err(why),
            (Err(e), Ok(_)) => Err(Refusal::Library(format!(
                "the read was refused because it could not be logged: {e}"
            ))),
        }
    }

    /// The recording, if it exists and is not hidden, at the `need` level.
    fn visible(
        &self,
        grant: &Grant,
        id: &RecordingId,
        need: Exposure,
    ) -> Result<Recording, Refusal> {
        let recording = self
            .store
            .recording(id)
            .map_err(|e| self.lib(e))?
            .ok_or(Refusal::NotFound)?;
        let hidden = self
            .store
            .hidden_from_assistants(id)
            .map_err(|e| self.lib(e))?;
        policy::reveal(grant, hidden, need)?;
        Ok(recording)
    }

    fn summary_flag(&self, id: &RecordingId) -> Result<(bool, SummaryFlag), Refusal> {
        let transcript = self.store.transcript(id).map_err(|e| self.lib(e))?;
        let summary = self.store.summary(id).map_err(|e| self.lib(e))?;
        Ok((
            transcript.is_some(),
            SummaryFlag::derive(
                summary.map(|s| s.transcript_revision),
                transcript.map(|t| t.revision),
            ),
        ))
    }

    pub fn search(&self, reported: Option<&str>, search: &Search) -> Result<SearchPage, Refusal> {
        let text = search.text.clone().unwrap_or_default();
        self.audited(
            reported,
            |page: Option<&SearchPage>| {
                Access::search(&text, page.map_or(0, |p| p.hits.len() as u32))
            },
            |page| page.hits.len() as u32,
            |grant| self.find(grant, search),
        )
    }

    /// The newest meetings this assistant may see, a page at a time, for a resource listing.
    pub fn list(
        &self,
        reported: Option<&str>,
        cursor: Option<SearchCursor>,
    ) -> Result<SearchPage, Refusal> {
        self.audited(
            reported,
            |_| Access::ResourceList,
            |page: &SearchPage| page.hits.len() as u32,
            |grant| {
                self.find(
                    grant,
                    &Search {
                        cursor: cursor.clone(),
                        limit: Some(MAX_SEARCH_RESULTS),
                        ..Search::default()
                    },
                )
            },
        )
    }

    /// The meetings a prompt points the assistant at: those matching `query`, or the one `id`.
    /// Prompts carry links to summaries, so they need the Summaries level.
    pub fn prompt_meetings(
        &self,
        reported: Option<&str>,
        prompt: &str,
        query: Option<&str>,
        id: Option<&RecordingId>,
        limit: u32,
    ) -> Result<Vec<Hit>, Refusal> {
        self.audited(
            reported,
            |_| Access::Prompt {
                name: prompt.to_owned(),
            },
            |hits: &Vec<Hit>| hits.len() as u32,
            |grant| {
                if grant.exposure() < Exposure::Summaries {
                    return Err(Refusal::NotShared {
                        need: Exposure::Summaries,
                    });
                }
                if let Some(id) = id {
                    let r = self.visible(grant, id, Exposure::Summaries)?;
                    let (has_transcript, summary) = self.summary_flag(id)?;
                    return Ok(vec![Hit {
                        id: r.id,
                        title: r.title,
                        created_at: r.created_at,
                        duration: r.take.map(|t| t.duration),
                        has_transcript,
                        summary,
                        section: None,
                        snippet: None,
                    }]);
                }
                Ok(self
                    .find(
                        grant,
                        &Search {
                            text: query.map(str::to_owned),
                            limit: Some(limit),
                            ..Search::default()
                        },
                    )?
                    .hits)
            },
        )
    }

    /// The page `search` asks for, by the same rules as the app's search, inside the grant. A
    /// correction draws only on words the grant could find, so it never names one it could not.
    fn find(&self, grant: &Grant, search: &Search) -> Result<SearchPage, Refusal> {
        let limit = search
            .limit
            .unwrap_or(DEFAULT_SEARCH_RESULTS)
            .clamp(1, MAX_SEARCH_RESULTS);
        let fields = grant.fields();
        let ask = |query: Option<Query>| AssistantQuery {
            query,
            fields,
            since: search.since,
            until: search.until,
            after: search.cursor.as_ref().map(|c| (c.created_at, c.id.clone())),
            limit,
        };
        let (query, found, corrections) = match search.text.as_deref().and_then(Query::parse) {
            None => (
                None,
                self.store
                    .assistant_recordings(&ask(None))
                    .map_err(|e| self.lib(e))?,
                Vec::new(),
            ),
            Some(query) => {
                let (query, found, corrections) = find(
                    query,
                    |q| self.store.assistant_recordings(&ask(Some(q.clone()))),
                    || self.store.assistant_vocabulary(fields).map(Arc::new),
                )
                .map_err(|e| self.lib(e))?;
                (Some(query), found, corrections)
            }
        };
        let mut hits = Vec::with_capacity(found.len());
        for f in &found {
            let r = &f.row.recording;
            let (section, snippet) = match &query {
                Some(query) => self.matched(grant, query, f)?,
                None => (None, None),
            };
            hits.push(Hit {
                id: r.id.clone(),
                title: r.title.clone(),
                created_at: r.created_at,
                duration: r.take.map(|t| t.duration),
                has_transcript: f.row.has_transcript,
                summary: f.row.summary,
                section,
                snippet,
            });
        }
        let next = (found.len() as u32 == limit)
            .then(|| found.last())
            .flatten()
            .map(|f| SearchCursor {
                created_at: f.row.recording.created_at,
                id: f.row.recording.id.clone(),
            });
        Ok(SearchPage {
            hits,
            next,
            corrections,
        })
    }

    /// Where `query` matched `found`, as the app's sidebar shows it: a title hit marks the title,
    /// and a conversation hit takes the first matching turn, translated before spoken, else
    /// summary line, translated before written, else speaker name, out of only the fields the
    /// grant reads.
    fn matched(
        &self,
        grant: &Grant,
        query: &Query,
        found: &Found,
    ) -> Result<(Option<Section>, Option<HitSnippet>), Refusal> {
        let r = &found.row.recording;
        if found.in_title {
            let title = r.title.clone().unwrap_or_default();
            let marks = search::highlights(query, &title);
            return Ok((
                Some(Section::Titles),
                Some(HitSnippet {
                    field: Field::Title,
                    snippet: Snippet { text: title, marks },
                    turn: None,
                }),
            ));
        }
        let fields = grant.fields();
        let transcript = match fields.transcript || fields.speakers {
            true => self.store.transcript(&r.id).map_err(|e| self.lib(e))?,
            false => None,
        };
        let names = match &transcript {
            Some(_) => self.store.speaker_names(&r.id).map_err(|e| self.lib(e))?,
            None => SpeakerNames::default(),
        };
        let translation = match (&transcript, fields.transcript) {
            (Some(_), true) => self.store.translation(&r.id).map_err(|e| self.lib(e))?,
            _ => None,
        };
        // The turns as get_transcript reads them, so a turn cursor lands on the same turn. A hit
        // is quoted from a translation first, then from what was spoken.
        let turns = match (&transcript, fields.transcript) {
            (Some(t), true) => turns::turns(t, &names, translation.as_ref()),
            _ => Vec::new(),
        };
        let speakers: Vec<&str> = match (&transcript, fields.speakers) {
            (Some(t), true) => t
                .speakers()
                .into_iter()
                .filter_map(|speaker| names.name_of(speaker, t.revision))
                .collect(),
            _ => Vec::new(),
        };
        let (translated_blocks, blocks) = match fields.summary {
            true => (
                self.store
                    .summary_translation(&r.id)
                    .map_err(|e| self.lib(e))?
                    .map(|t| search::summary_blocks(&t.markdown))
                    .unwrap_or_default(),
                self.store
                    .summary(&r.id)
                    .map_err(|e| self.lib(e))?
                    .map(|s| search::summary_blocks(&s.markdown))
                    .unwrap_or_default(),
            ),
            false => (Vec::new(), Vec::new()),
        };
        let translated: Vec<&str> = match turns.iter().any(|t| t.translated.is_some()) {
            true => turns.iter().map(|t| t.text.as_str()).collect(),
            false => Vec::new(),
        };
        let spoken: Vec<&str> = turns
            .iter()
            .map(|t| {
                t.translated
                    .as_ref()
                    .map_or(t.text.as_str(), |t| &t.original)
            })
            .collect();
        let translated_blocks: Vec<&str> = translated_blocks.iter().map(String::as_str).collect();
        let blocks: Vec<&str> = blocks.iter().map(String::as_str).collect();
        let excerpt = search::excerpt_within(
            query,
            &translated,
            &spoken,
            &translated_blocks,
            &blocks,
            &speakers,
            SNIPPET_CHARS,
        );
        let snippet = excerpt.map(|e| {
            let turn = match (e.source, &transcript) {
                (Source::Transcript, Some(t)) => turns.get(e.item).map(|turn| TurnAt {
                    cursor: TurnCursor {
                        revision: t.revision,
                        turn: e.item as u32,
                    },
                    speaker: turn.speaker.clone(),
                    start: turn.start,
                }),
                _ => None,
            };
            HitSnippet {
                field: match e.source {
                    Source::Transcript => Field::Transcript,
                    Source::Summary => Field::Summary,
                    Source::Speakers => Field::Speakers,
                },
                snippet: e.snippet,
                turn,
            }
        });
        Ok((Some(Section::Conversations), snippet))
    }

    pub fn recording(
        &self,
        reported: Option<&str>,
        id: &RecordingId,
    ) -> Result<RecordingView, Refusal> {
        self.audited(
            reported,
            |_| Access::Recording(id.clone()),
            |_| 1,
            |grant| {
                let r = self.visible(grant, id, Exposure::Titles)?;
                let (has_transcript, summary) = self.summary_flag(id)?;
                let speakers = if grant.exposure() >= Exposure::Summaries {
                    let names = self.store.speaker_names(id).map_err(|e| self.lib(e))?;
                    let transcript = self.store.transcript(id).map_err(|e| self.lib(e))?;
                    Some(match transcript {
                        Some(t) => {
                            let mut seen: Vec<String> = Vec::new();
                            for segment in &t.segments {
                                let label = names.label(segment, t.revision);
                                if !seen.contains(&label) {
                                    seen.push(label);
                                }
                            }
                            seen
                        }
                        None => Vec::new(),
                    })
                } else {
                    None
                };
                Ok(RecordingView {
                    id: r.id,
                    title: r.title,
                    created_at: r.created_at,
                    duration: r.take.map(|t| t.duration),
                    language: r.language,
                    layout: r.layout,
                    has_transcript,
                    summary,
                    speakers,
                })
            },
        )
    }

    pub fn summary(
        &self,
        reported: Option<&str>,
        id: &RecordingId,
    ) -> Result<SummaryView, Refusal> {
        self.audited(
            reported,
            |_| Access::Summary(id.clone()),
            |_| 1,
            |grant| {
                let r = self.visible(grant, id, Exposure::Summaries)?;
                let summary = self
                    .store
                    .summary(id)
                    .map_err(|e| self.lib(e))?
                    .ok_or(Refusal::NotFound)?;
                let (_, flag) = self.summary_flag(id)?;
                Ok(SummaryView {
                    id: r.id,
                    title: r.title,
                    markdown: summary.markdown,
                    language: summary.language,
                    stale: flag == SummaryFlag::Stale,
                    produced_by: summary.produced_by,
                    instruction: self
                        .store
                        .summary_instruction(id)
                        .map_err(|e| self.lib(e))?,
                })
            },
        )
    }

    /// A whole transcript, for a resource the user attached. Resources have no paging, so the
    /// client decides whether it fits.
    pub fn transcript(
        &self,
        reported: Option<&str>,
        id: &RecordingId,
    ) -> Result<TranscriptPage, Refusal> {
        self.page_of(reported, id, None, usize::MAX)
    }

    pub fn transcript_page(
        &self,
        reported: Option<&str>,
        id: &RecordingId,
        cursor: Option<TurnCursor>,
        page_chars: Option<usize>,
    ) -> Result<TranscriptPage, Refusal> {
        self.page_of(reported, id, cursor, turns::page_chars(page_chars))
    }

    fn page_of(
        &self,
        reported: Option<&str>,
        id: &RecordingId,
        cursor: Option<TurnCursor>,
        budget: usize,
    ) -> Result<TranscriptPage, Refusal> {
        self.audited(
            reported,
            |page: Option<&TranscriptPage>| Access::Transcript {
                id: id.clone(),
                turns: page.map_or((0, 0), |p| (p.first, p.first + p.turns.len() as u32)),
            },
            |page| page.turns.len() as u32,
            |grant| {
                let r = self.visible(grant, id, Exposure::Transcripts)?;
                let transcript = self
                    .store
                    .transcript(id)
                    .map_err(|e| self.lib(e))?
                    .ok_or(Refusal::NotFound)?;
                if cursor.is_some_and(|c| c.revision != transcript.revision) {
                    return Err(Refusal::StaleRevision);
                }
                let names = self.store.speaker_names(id).map_err(|e| self.lib(e))?;
                let translation = self.store.translation(id).map_err(|e| self.lib(e))?;
                let all = turns::turns(&transcript, &names, translation.as_ref());
                let from = cursor.map_or(0, |c| c.turn as usize);
                let (page, next) = turns::page(&all, from, budget);
                Ok(TranscriptPage {
                    id: r.id,
                    title: r.title,
                    revision: transcript.revision,
                    translated_into: translation
                        .filter(|t| t.applies_to(&transcript) && t.has_text())
                        .map(|t| t.target),
                    first: from.min(all.len()) as u32,
                    total: all.len() as u32,
                    turns: page.to_vec(),
                    next: next.map(|n| TurnCursor {
                        revision: transcript.revision,
                        turn: n as u32,
                    }),
                })
            },
        )
    }

    pub fn action_items(
        &self,
        reported: Option<&str>,
        scope: &ItemScope,
    ) -> Result<Vec<MeetingItems>, Refusal> {
        self.audited(
            reported,
            |found: Option<&Vec<MeetingItems>>| Access::ActionItems {
                ids: found.map_or_else(
                    || scope.id.iter().cloned().collect(),
                    |m| m.iter().map(|m| m.id.clone()).collect(),
                ),
            },
            |found| {
                found
                    .iter()
                    .map(|m| m.items.as_ref().map_or(0, Vec::len))
                    .sum::<usize>() as u32
            },
            |grant| {
                let ids: Vec<RecordingId> = match &scope.id {
                    Some(id) => {
                        self.visible(grant, id, Exposure::Summaries)?;
                        vec![id.clone()]
                    }
                    None => {
                        if grant.exposure() < Exposure::Summaries {
                            return Err(Refusal::NotShared {
                                need: Exposure::Summaries,
                            });
                        }
                        self.store
                            .assistant_recordings(&AssistantQuery {
                                query: None,
                                fields: grant.fields(),
                                since: scope.since,
                                until: None,
                                after: None,
                                limit: scope
                                    .limit
                                    .unwrap_or(MAX_ITEM_MEETINGS)
                                    .clamp(1, MAX_ITEM_MEETINGS),
                            })
                            .map_err(|e| self.lib(e))?
                            .into_iter()
                            .map(|found| found.row.recording.id)
                            .collect()
                    }
                };
                let mut out = Vec::new();
                for id in ids {
                    let Some(summary) = self.store.summary(&id).map_err(|e| self.lib(e))? else {
                        continue;
                    };
                    let Some(r) = self.store.recording(&id).map_err(|e| self.lib(e))? else {
                        continue;
                    };
                    let (_, flag) = self.summary_flag(&id)?;
                    out.push(MeetingItems {
                        id,
                        title: r.title,
                        created_at: r.created_at,
                        items: match sections(&summary.markdown) {
                            Sections::Three { action_items, .. } => Some(list_items(&action_items)),
                            Sections::Unstructured => None,
                        },
                        stale: flag == SummaryFlag::Stale,
                    });
                }
                Ok(out)
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_cursors_round_trip() {
        let c = SearchCursor {
            created_at: Timestamp(1_700_000_000_000),
            id: RecordingId::parse("2026-11-16T041159-0000").unwrap(),
        };
        assert_eq!(SearchCursor::parse(&c.to_string()), Some(c));
        assert_eq!(SearchCursor::parse("x"), None);
    }
}
