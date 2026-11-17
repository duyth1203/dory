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

use crate::domain::{
    Access, Actor, AssistantQuery, AuditEntry, AuditOutcome, ChannelLayout, ClientKey, Exposure,
    LangTag, LanguagePref, Ms, Recording, RecordingId, Refusal, Revision, Sections, SummaryFlag,
    Timestamp, list_items, sections,
};
use crate::models::ModelRef;
use crate::ports::{AuditSink, Clock, StoreError, StoreRead};
use policy::Grant;
use turns::{Turn, TurnCursor};

pub const MAX_SEARCH_RESULTS: u32 = 25;
pub const DEFAULT_SEARCH_RESULTS: u32 = 10;
pub const MAX_ITEM_MEETINGS: u32 = 20;
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
    Summary,
    Transcript,
}

impl Field {
    pub fn as_str(self) -> &'static str {
        match self {
            Field::Title => "title",
            Field::Summary => "summary",
            Field::Transcript => "transcript",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub id: RecordingId,
    pub title: Option<String>,
    pub created_at: Timestamp,
    pub duration: Option<Ms>,
    pub has_transcript: bool,
    pub summary: SummaryFlag,
    /// Text around the match, cut only from a field this assistant may read.
    pub snippet: Option<(Field, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchPage {
    pub hits: Vec<Hit>,
    pub next: Option<SearchCursor>,
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptPage {
    pub id: RecordingId,
    pub title: Option<String>,
    pub revision: Revision,
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
        }
    }

    /// True once a read found the library's schema changed under it, which happens when Dory was
    /// updated. The adapter should exit so its client restarts it from the new build.
    pub fn library_changed(&self) -> bool {
        self.library_changed.load(Ordering::Relaxed)
    }

    fn lib(&self, e: StoreError) -> Refusal {
        if matches!(e, StoreError::SchemaChanged { .. }) {
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

    fn find(&self, grant: &Grant, search: &Search) -> Result<SearchPage, Refusal> {
        let limit = search
            .limit
            .unwrap_or(DEFAULT_SEARCH_RESULTS)
            .clamp(1, MAX_SEARCH_RESULTS);
        let fields = grant.fields();
        let rows = self
            .store
            .assistant_recordings(&AssistantQuery {
                text: search.text.clone(),
                fields,
                since: search.since,
                until: search.until,
                after: search.cursor.as_ref().map(|c| (c.created_at, c.id.clone())),
                limit,
            })
            .map_err(|e| self.lib(e))?;
        let needle = search
            .text
            .as_deref()
            .map(|t| t.trim().to_lowercase())
            .filter(|t| !t.is_empty());
        let mut hits = Vec::with_capacity(rows.len());
        for row in &rows {
            let r = &row.recording;
            let snippet = match &needle {
                Some(n) => self.snippet(r, n, fields)?,
                None => None,
            };
            hits.push(Hit {
                id: r.id.clone(),
                title: r.title.clone(),
                created_at: r.created_at,
                duration: r.take.map(|t| t.duration),
                has_transcript: row.has_transcript,
                summary: row.summary,
                snippet,
            });
        }
        let next = (rows.len() as u32 == limit)
            .then(|| rows.last())
            .flatten()
            .map(|row| SearchCursor {
                created_at: row.recording.created_at,
                id: row.recording.id.clone(),
            });
        Ok(SearchPage { hits, next })
    }

    fn snippet(
        &self,
        recording: &Recording,
        needle: &str,
        fields: crate::domain::Fields,
    ) -> Result<Option<(Field, String)>, Refusal> {
        if let Some(title) = &recording.title
            && let Some(s) = cut_around(title, needle)
        {
            return Ok(Some((Field::Title, s)));
        }
        if fields.summary
            && let Some(summary) = self.store.summary(&recording.id).map_err(|e| self.lib(e))?
            && let Some(s) = cut_around(&summary.markdown, needle)
        {
            return Ok(Some((Field::Summary, s)));
        }
        if fields.transcript
            && let Some(t) = self
                .store
                .transcript(&recording.id)
                .map_err(|e| self.lib(e))?
        {
            for segment in &t.segments {
                if let Some(s) = cut_around(&segment.text, needle) {
                    return Ok(Some((Field::Transcript, s)));
                }
            }
        }
        Ok(None)
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
                let all = turns::turns(&transcript, &names);
                let from = cursor.map_or(0, |c| c.turn as usize);
                let (page, next) = turns::page(&all, from, budget);
                Ok(TranscriptPage {
                    id: r.id,
                    title: r.title,
                    revision: transcript.revision,
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
                                text: None,
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
                            .map(|row| row.recording.id)
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

/// About [`SNIPPET_CHARS`] of `text` around the first case-insensitive match of `needle`, which
/// is already lowercase.
fn cut_around(text: &str, needle: &str) -> Option<String> {
    let lower: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let needle: Vec<char> = needle.chars().collect();
    let at = lower
        .windows(needle.len())
        .position(|w| w == needle.as_slice())?;
    let chars: Vec<char> = text.chars().collect();
    let at = at.min(chars.len());
    let half = SNIPPET_CHARS.saturating_sub(needle.len()) / 2;
    let start = at.saturating_sub(half);
    let end = (start + SNIPPET_CHARS).min(chars.len());
    let mut s: String = chars[start..end].iter().collect();
    if start > 0 {
        s.insert(0, '…');
    }
    if end < chars.len() {
        s.push('…');
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets_cut_around_the_match() {
        assert_eq!(
            cut_around("Weekly Sync", "sync").as_deref(),
            Some("Weekly Sync")
        );
        assert_eq!(cut_around("Weekly", "budget"), None);
        let long = format!("{}budget{}", "a".repeat(500), "b".repeat(500));
        let s = cut_around(&long, "budget").unwrap();
        assert!(s.contains("budget") && s.starts_with('…') && s.ends_with('…'));
        assert!(s.chars().count() <= SNIPPET_CHARS + 2);
    }

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
