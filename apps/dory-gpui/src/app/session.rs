use std::sync::Arc;

use dory_core::domain::{
    Channel, LangTag, Recording, RecordingId, RecordingRow, SpeakerId, SpeakerNames, Summary,
    TimeRange, Transcript, Translation,
};
use dory_core::ports::StoreError;
use dory_core::search::Query;
use gpui_kit::component::text::{RenderedText, TextViewState};
use gpui_kit::{AppContext as _, Context, Entity, SharedString, Subscription, Task};

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
    /// The turns as spoken.
    pub turns: Vec<Turn>,
    /// The transcript translated into the output language, when this recording has a translation
    /// of the transcript now stored that changes any text.
    pub translation: Option<Translation>,
    /// The same turns, written in the output language. Present exactly when `translation` is, and
    /// the same length as `turns`, so a turn number means the same turn in both.
    pub translated_turns: Option<Vec<Turn>>,
}

/// Which text of a translated transcript the transcript tab shows, and so what Copy and Export
/// put out. Only a recording with a translation has a choice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TranscriptText {
    /// The translation, as soon as there is one.
    #[default]
    Translated,
    Spoken,
}

impl Detail {
    /// The turns `text` asks for. A recording with no translation only has what was spoken.
    pub fn shown(&self, text: TranscriptText) -> &[Turn] {
        match (text, &self.translated_turns) {
            (TranscriptText::Translated, Some(translated)) => translated,
            _ => &self.turns,
        }
    }

    /// Whether `text` ends up showing the translation.
    pub fn translated(&self, text: TranscriptText) -> bool {
        text == TranscriptText::Translated && self.translated_turns.is_some()
    }
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
    /// Set on a translated turn: the languages its translated segments were spoken in, which may
    /// be none when the engine did not say. `None` on a turn as spoken.
    pub translated_from: Option<Vec<LangTag>>,
}

impl Turn {
    /// What VoiceOver adds to a translated turn: "translated from Tagalog".
    pub fn translated_note(&self) -> Option<String> {
        let from = self.translated_from.as_ref()?;
        let names: Vec<String> = from.iter().map(crate::languages::primary_name).collect();
        Some(match names.as_slice() {
            [] => "translated".to_owned(),
            names => format!(
                "translated from {}",
                crate::languages::name_list(names, "and")
            ),
        })
    }
}

/// Groups segments into turns. The local user on the mic channel reads "Me", as in the CLI.
pub fn turns(transcript: &Transcript, names: &SpeakerNames) -> Vec<Turn> {
    group(transcript, names, None)
}

/// The same turns with each translated segment as translated. Turns split on speakers alone, as
/// in [`turns`], so turn `n` here is turn `n` there.
pub fn translated_turns(
    transcript: &Transcript,
    names: &SpeakerNames,
    translation: &Translation,
) -> Vec<Turn> {
    group(transcript, names, Some(translation))
}

fn group(
    transcript: &Transcript,
    names: &SpeakerNames,
    translation: Option<&Translation>,
) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    for (ix, segment) in transcript.segments.iter().enumerate() {
        let translated = translation.and_then(|t| t.text(ix));
        let text = translated.unwrap_or(&segment.text);
        // Every translated segment marks its turn, and the languages it was spoken in ride along.
        let marked = |turn: &mut Turn| {
            if translated.is_some() {
                let from = turn.translated_from.get_or_insert_with(Vec::new);
                if let Some(language) = &segment.language
                    && !from.iter().any(|l| l.primary() == language.primary())
                {
                    from.push(language.clone());
                }
            }
        };
        match turns.last_mut() {
            Some(turn) if turn.speaker == segment.speaker => {
                turn.text.push(' ');
                turn.text.push_str(text);
                if let (Some(range), Some(next)) = (turn.range, segment.range) {
                    turn.range = TimeRange::new(range.start(), next.end().max(range.end()));
                }
                marked(turn);
            }
            _ => {
                let mut turn = Turn {
                    speaker: segment.speaker,
                    label: speaker_label(segment.speaker, segment.channel, transcript, names),
                    range: segment.range,
                    text: text.to_owned(),
                    translated_from: None,
                };
                marked(&mut turn);
                turns.push(turn);
            }
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
///
/// The session owns the summary's text view state, and the Summary tab draws that same state. A
/// match in the summary is counted over the text the state renders, so what is counted is what
/// is marked, and a link's address is neither.
pub struct Session {
    core: Core,
    id: RecordingId,
    loaded: Loadable,
    player: Entity<PlayerHandle>,
    error: Option<SharedString>,
    text: TranscriptText,
    generation: u64,
    query: Option<Query>,
    found: Option<Matches>,
    reveal: u64,
    summary: Entity<TextViewState>,
    /// The rendered summary the matches were counted over.
    counted: Option<RenderedText>,
    /// A jump that waited for the summary to be rendered.
    jump_pending: bool,
    _load: Option<Task<()>>,
    _summary_watch: Subscription,
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
        let summary = cx.new(|cx| TextViewState::markdown("", cx));
        let watch = cx.observe(&summary, |this, _, cx| this.on_summary_rendered(cx));
        let mut session = Self {
            core,
            id,
            loaded: Loadable::Loading,
            player,
            error: None,
            text: TranscriptText::default(),
            generation: 0,
            query,
            found: None,
            reveal: 0,
            summary,
            counted: None,
            jump_pending: false,
            _load: None,
            _summary_watch: watch,
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

    /// Which text of a translated transcript the tab shows, and Copy and Export use.
    pub fn text(&self) -> TranscriptText {
        self.text
    }

    pub fn set_text(&mut self, text: TranscriptText, cx: &mut Context<Self>) {
        if self.text != text {
            self.text = text;
            cx.notify();
        }
    }

    /// The last write that failed, for the detail pane to show. Cleared by the next write.
    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    /// The matches of the active search in this recording, `None` without a search or a match.
    pub fn found(&self) -> Option<&Matches> {
        self.found.as_ref()
    }

    /// The state the Summary tab draws. Only the session sets its text.
    pub fn summary_state(&self) -> &Entity<TextViewState> {
        &self.summary
    }

    /// The rendered summary the matches were counted over, `None` until it is rendered.
    pub fn counted_summary(&self) -> Option<&RenderedText> {
        self.counted.as_ref()
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
        // Matches are found in what was spoken, so stepping shows that text, where they are marked.
        self.text = TranscriptText::Spoken;
        self.reveal += 1;
        cx.notify();
        Some(Moved { at, spoken })
    }

    /// Lists the query's matches in the loaded detail. A reload keeps the current match, and only
    /// a new query or the first load asks the views to jump to it. The summary's matches wait for
    /// its text to be rendered, and the jump with them.
    fn rematch(&mut self, jump: bool, cx: &mut Context<Self>) {
        let kept = self.found.take();
        self.counted = None;
        if let (Some(query), Some(detail)) = (self.query.clone(), self.detail()) {
            let rendered = self.summary.read(cx).rendered_text();
            let ready = rendered.source() == markdown_of(&detail);
            let summary = if ready { rendered.as_str() } else { "" };
            let title = crate::library::given_title(&detail.recording).unwrap_or_default();
            let turns: Vec<&str> = detail.turns.iter().map(|t| t.text.as_str()).collect();
            self.found = Matches::open(&query, title, &turns, summary).map(|opened| match kept {
                Some(old) if !jump && old.query() == &query => opened.on(old.current()),
                _ => opened,
            });
            if ready {
                self.counted = Some(rendered);
                self.jump_pending = false;
            } else {
                self.jump_pending |= jump;
            }
            if jump && self.found.is_some() {
                self.text = TranscriptText::Spoken;
                self.reveal += 1;
            }
        }
        cx.notify();
    }

    /// The summary's text view parsed new text, so its matches are counted again.
    fn on_summary_rendered(&mut self, cx: &mut Context<Self>) {
        if self.query.is_none()
            || self.counted.as_ref() == Some(&self.summary.read(cx).rendered_text())
        {
            return;
        }
        let jump = std::mem::take(&mut self.jump_pending);
        self.rematch(jump, cx);
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
                let markdown = this
                    .detail()
                    .map(|d| markdown_of(&d).to_owned())
                    .unwrap_or_default();
                this.summary
                    .update(cx, |state, cx| state.set_text(&markdown, cx));
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

/// The summary's Markdown, empty without a summary.
fn markdown_of(detail: &Detail) -> &str {
    detail.summary.as_ref().map_or("", |s| s.markdown.as_str())
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
    let translation = match &transcript {
        Some(transcript) => library
            .translation(id)?
            .filter(|t| t.applies_to(transcript) && t.has_text()),
        None => None,
    };
    let translated_turns = transcript
        .as_ref()
        .zip(translation.as_ref())
        .map(|(transcript, translation)| translated_turns(transcript, &names, translation));
    Ok(Some(Detail {
        recording,
        transcript,
        summary: library.summary(id)?,
        names,
        turns,
        translation,
        translated_turns,
    }))
}
