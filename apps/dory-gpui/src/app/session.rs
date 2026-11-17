use std::sync::Arc;

use dory_core::domain::{
    Channel, Recording, RecordingId, RecordingRow, SpeakerId, SpeakerNames, Summary, TimeRange,
    Transcript,
};
use dory_core::ports::StoreError;
use dory_core::search::{self, Query};
use gpui_kit::{AppContext as _, Context, Entity, SharedString, Task};

use super::core::Core;
use super::player::PlayerHandle;
use crate::find::{MatchAt, Matches};

/// Everything the detail pane shows for the selected recording, read off the UI thread in one go
/// and swapped whole, so the summary, the transcript, and export never see two different reads.
#[derive(Clone, Debug)]
pub struct Detail {
    pub recording: Recording,
    pub transcript: Option<Transcript>,
    pub summary: Option<Summary>,
    pub names: SpeakerNames,
    pub turns: Vec<Turn>,
}

pub enum Loadable {
    Loading,
    Ready(Arc<Detail>),
    /// The recording is gone, or the store failed. The text says which.
    Failed(SharedString),
}

/// One speaker's consecutive segments (UX-3), labelled with the user's name for the speaker when
/// there is one.
#[derive(Clone, Debug, PartialEq)]
pub struct Turn {
    pub speaker: SpeakerId,
    pub label: SharedString,
    /// `None` for transcripts imported from the old app, which have no times and cannot seek.
    pub range: Option<TimeRange>,
    pub text: String,
}

/// Groups segments into turns. The local user on the mic channel reads "Me", as in the CLI.
pub fn turns(transcript: &Transcript, names: &SpeakerNames) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    for segment in &transcript.segments {
        match turns.last_mut() {
            Some(turn) if turn.speaker == segment.speaker => {
                turn.text.push(' ');
                turn.text.push_str(&segment.text);
                if let (Some(range), Some(next)) = (turn.range, segment.range) {
                    turn.range = TimeRange::new(range.start(), next.end().max(range.end()));
                }
            }
            _ => turns.push(Turn {
                speaker: segment.speaker,
                label: speaker_label(segment.speaker, segment.channel, transcript, names),
                range: segment.range,
                text: segment.text.clone(),
            }),
        }
    }
    turns
}

pub fn speaker_label(
    speaker: SpeakerId,
    channel: Channel,
    transcript: &Transcript,
    names: &SpeakerNames,
) -> SharedString {
    if let Some(name) = names.name_of(speaker, transcript.revision) {
        return name.to_owned().into();
    }
    if speaker == SpeakerId::LOCAL && channel == Channel::Mic {
        return "Me".into();
    }
    format!("Speaker {}", speaker.0).into()
}

/// The selected recording: its detail, read off-thread and reloaded when the core says it
/// changed, and its [`PlayerHandle`]. `AppState` creates one per selection and drops it when the
/// selection moves, which stops playback.
///
/// Views read `detail()` and write through `rename_speaker` and `set_title`. A write never
/// patches `Detail` in place: the core announces the change, `AppState` calls `reload`, and the
/// new `Detail` arrives whole.
///
/// While a search is active the session also holds the [`Matches`] of its query in this
/// recording (UX-32). `reveal` counts the times the views should bring the current match into
/// view: a hit opening, and each step. A view remembers the count it last acted on.
pub struct Session {
    core: Core,
    id: RecordingId,
    loaded: Loadable,
    player: Entity<PlayerHandle>,
    error: Option<SharedString>,
    generation: u64,
    query: Option<Query>,
    found: Option<Matches>,
    reveal: u64,
    _load: Option<Task<()>>,
}

/// The current match after a step, and what VoiceOver says about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Moved {
    pub at: MatchAt,
    pub spoken: String,
}

impl Session {
    pub(super) fn new(
        core: Core,
        row: &RecordingRow,
        query: Option<Query>,
        cx: &mut Context<Self>,
    ) -> Self {
        let id = row.recording.id.clone();
        let duration = row.recording.take.map(|t| t.duration).unwrap_or_default();
        let player = cx.new(|_| PlayerHandle::new(core.clone(), id.clone(), duration));
        let mut session = Self {
            core,
            id,
            loaded: Loadable::Loading,
            player,
            error: None,
            generation: 0,
            query,
            found: None,
            reveal: 0,
            _load: None,
        };
        session.reload(cx);
        session
    }

    pub fn id(&self) -> &RecordingId {
        &self.id
    }

    pub fn loaded(&self) -> &Loadable {
        &self.loaded
    }

    pub fn detail(&self) -> Option<Arc<Detail>> {
        match &self.loaded {
            Loadable::Ready(detail) => Some(Arc::clone(detail)),
            Loadable::Loading | Loadable::Failed(_) => None,
        }
    }

    pub fn player(&self) -> &Entity<PlayerHandle> {
        &self.player
    }

    /// The last write that failed, for the detail pane to show. Cleared by the next write.
    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    /// The matches of the active search in this recording, `None` without a search or a match.
    pub fn found(&self) -> Option<&Matches> {
        self.found.as_ref()
    }

    /// How many times the views have been asked to bring the current match into view.
    pub fn reveal(&self) -> u64 {
        self.reveal
    }

    /// Follows the search: the new query's matches replace the old, starting where a hit opens
    /// and asking the views to show them. `None` clears them. The same query changes nothing.
    pub(super) fn set_query(&mut self, query: Option<Query>, cx: &mut Context<Self>) {
        if self.query == query {
            return;
        }
        self.query = query;
        self.found = None;
        self.rematch(true, cx);
    }

    /// Moves the current match `delta` places, wrapping, and asks the views to show it.
    pub fn step_match(&mut self, delta: isize, cx: &mut Context<Self>) -> Option<Moved> {
        let detail = self.detail()?;
        let found = self.found.as_mut()?;
        let at = found.step(delta).clone();
        let spoken = found.spoken(&detail.turns);
        self.reveal += 1;
        cx.notify();
        Some(Moved { at, spoken })
    }

    /// Lists the query's matches in the loaded detail. A reload keeps the current match, and only
    /// a new query or the first load asks the views to jump to it.
    fn rematch(&mut self, jump: bool, cx: &mut Context<Self>) {
        let kept = self.found.take();
        if let (Some(query), Some(detail)) = (&self.query, self.detail()) {
            let title = crate::library::given_title(&detail.recording).unwrap_or_default();
            let turns: Vec<&str> = detail.turns.iter().map(|t| t.text.as_str()).collect();
            let blocks = detail
                .summary
                .as_ref()
                .map(|s| search::summary_blocks(&s.markdown))
                .unwrap_or_default();
            let blocks: Vec<&str> = blocks.iter().map(String::as_str).collect();
            self.found = Matches::open(query, title, &turns, &blocks).map(|opened| match kept {
                Some(old) if !jump && old.query() == query => opened.on(old.current()),
                _ => opened,
            });
            if jump && self.found.is_some() {
                self.reveal += 1;
            }
        }
        cx.notify();
    }

    /// Names a speaker for the whole recording. `None` or blank goes back to "Speaker N".
    pub fn rename_speaker(
        &mut self,
        speaker: SpeakerId,
        name: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let id = self.id.clone();
        self.write(cx, move |dory| {
            dory.library()
                .rename_speaker(&id, speaker, name)
                .map_err(|e| e.to_string())
        });
    }

    /// `None` or blank goes back to the date-based title.
    pub fn set_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        let id = self.id.clone();
        let title = title.filter(|t| !t.trim().is_empty());
        self.write(cx, move |dory| {
            dory.library()
                .set_title(&id, title)
                .map_err(|e| e.to_string())
        });
    }

    pub(super) fn reload(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let id = self.id.clone();
        let read = self.core.run(cx, move |dory| read_detail(dory, &id));
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = read.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                if let Ok(Some(detail)) = &result {
                    let duration = detail
                        .recording
                        .take
                        .map(|t| t.duration)
                        .unwrap_or_default();
                    this.player
                        .update(cx, |player, cx| player.set_duration(duration, cx));
                }
                let first = !matches!(this.loaded, Loadable::Ready(_));
                this.loaded = match result {
                    Ok(Some(detail)) => Loadable::Ready(Arc::new(detail)),
                    Ok(None) => Loadable::Failed("This recording was deleted.".into()),
                    Err(error) => Loadable::Failed(error.to_string().into()),
                };
                this.rematch(first, cx);
            });
        }));
    }

    fn write(
        &mut self,
        cx: &mut Context<Self>,
        work: impl FnOnce(&dory_core::app::Dory) -> Result<(), String> + Send + 'static,
    ) {
        self.error = None;
        let write = self.core.run(cx, work);
        cx.spawn(async move |this, cx| {
            if let Err(error) = write.await {
                let _ = this.update(cx, |this, cx| {
                    this.error = Some(error.into());
                    cx.notify();
                });
            }
        })
        .detach();
    }
}

fn read_detail(
    dory: &dory_core::app::Dory,
    id: &RecordingId,
) -> Result<Option<Detail>, StoreError> {
    let library = dory.library();
    let Some(recording) = library.recording(id)? else {
        return Ok(None);
    };
    let transcript = library.transcript(id)?;
    let names = library.speaker_names(id)?;
    let turns = transcript
        .as_ref()
        .map(|t| turns(t, &names))
        .unwrap_or_default();
    Ok(Some(Detail {
        recording,
        transcript,
        summary: library.summary(id)?,
        names,
        turns,
    }))
}
