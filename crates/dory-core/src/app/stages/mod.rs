//! What the transcribe, translate, summarize, and translate-summary stages do. Each returns the
//! changes to commit with its `StageDone`, or why it could not finish.
//!
//! Transcribe plans every clip's windows first, so progress covers the whole job. Each finished
//! window is checkpointed on its own, and a rerun skips the windows its run already finished.

mod merge;
mod summary;
mod translate;
pub mod windows;

use std::collections::BTreeMap;

use super::events::{Db, Event};
use super::models::{Engines, Registries};
use super::pace::Budget;
use crate::domain::{
    Blocker, Channel, ChannelLayout, FailureCause, FailureClass, Fraction, Job, JobError, LangTag,
    LanguagePref, Ms, Revision, Segment, Settings, SpeakerId, Stage, Summary, SummaryTranslation,
    TimeRange, Transcript, Translation,
};
use crate::models::{
    Choice, Diar, DiarNeeds, DiarizationNeed, DiarizationSupport, KindTag, LangSupport, Llm,
    LlmNeeds, Mismatch, ModelId, ModelKind, ModelRef, Resolution, Stt, SttNeeds, run_language,
};
use crate::ports::{
    CancelToken, Change, Checkpoint, CheckpointKey, Classify, Clip, CompletionRequest, EngineError,
    RawSegment, SegmentSink, Takes, TextSink, TranscribeRequest, Transcriber, TranslationBatch,
    TranslationKey, Turn,
};
use summary::{Pass, SummaryError};

/// Why a stage stopped short.
#[derive(Debug)]
pub(crate) enum StageError {
    Blocked(Blocker),
    Failed(JobError),
}

pub(crate) fn failed(error: impl Classify + std::fmt::Display) -> StageError {
    StageError::Failed(JobError {
        class: error.class(),
        message: error.to_string(),
        cause: error.cause(),
    })
}

fn permanent(message: &str) -> StageError {
    StageError::Failed(JobError {
        class: FailureClass::Permanent,
        message: message.to_owned(),
        cause: FailureCause::Other,
    })
}

/// The language an engine refused, unless it could not name it.
fn named(tag: LangTag) -> Vec<LangTag> {
    if tag.as_str() == UNDETERMINED {
        Vec::new()
    } else {
        vec![tag]
    }
}

/// An engine error with the model it came from, so a missing model or language becomes a
/// blocker the user can act on.
fn engine_failed(model: ModelRef) -> impl FnOnce(EngineError) -> StageError {
    move |error| match error {
        EngineError::ModelMissing(_) => {
            StageError::Blocked(Blocker::NeedsProvision { model, bytes: None })
        }
        EngineError::UnsupportedLanguage(tag) => StageError::Blocked(Blocker::Incompatible {
            model,
            why: Mismatch::LanguageUnsupported(named(tag)),
        }),
        other => failed(other),
    }
}

fn settle<K: ModelKind>(resolution: Resolution<K>) -> Result<Option<ModelId<K>>, StageError> {
    match resolution {
        Resolution::Use(id) => Ok(Some(id)),
        Resolution::Skip => Ok(None),
        Resolution::Blocked(blocker) => Err(StageError::Blocked(blocker)),
    }
}

pub(crate) struct StageCx<'a> {
    pub job: &'a Job,
    pub db: &'a Db,
    pub takes: &'a dyn Takes,
    pub models: &'a Registries,
    pub engines: &'a mut Engines,
    pub cancel: &'a CancelToken,
    pub settings: &'a Settings,
    pub now_mono: u64,
    /// The budget for the next transcribe window.
    pub pace: &'a mut dyn FnMut() -> Budget,
}

pub(crate) fn run(stage: Stage, cx: StageCx<'_>) -> Result<Vec<Change>, StageError> {
    match stage {
        Stage::Transcribe => transcribe(cx),
        Stage::Translate => translate(cx),
        Stage::Summarize => summarize(cx),
        Stage::TranslateSummary => translate_summary(cx),
    }
}

/// Who speaks in a clip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Speakers {
    /// One known speaker, such as the local user on the mic channel.
    Fixed(SpeakerId),
    /// Separated by diarization, numbered from `first` in order of first appearance.
    Diarized { first: u16 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClipPlan {
    pub channel: Channel,
    pub speakers: Speakers,
}

/// `SpeakerId(0)` is reserved for the local user, which only a separate mic channel proves.
pub(crate) fn plan_clips(layout: ChannelLayout) -> Vec<ClipPlan> {
    let diarized = |channel| ClipPlan {
        channel,
        speakers: Speakers::Diarized { first: 1 },
    };
    match layout {
        ChannelLayout::MicSystem => vec![
            ClipPlan {
                channel: Channel::Mic,
                speakers: Speakers::Fixed(SpeakerId::LOCAL),
            },
            diarized(Channel::System),
        ],
        ChannelLayout::MicOnly => vec![diarized(Channel::Mic)],
        ChannelLayout::SystemOnly => vec![diarized(Channel::System)],
        ChannelLayout::Mixed => vec![diarized(Channel::Mixed)],
    }
}

/// One clip's share of a transcription run.
struct ClipRun {
    plan: ClipPlan,
    whole: Clip,
    /// The engine separates speakers itself, so the clip goes in one call.
    diarize: bool,
    windows: Vec<TimeRange>,
    key: CheckpointKey,
    saved: Vec<Checkpoint>,
}

/// What every window of a run asks the engine for.
struct WindowCx<'a> {
    db: &'a Db,
    takes: &'a dyn Takes,
    cancel: &'a CancelToken,
    stt: &'a dyn Transcriber,
    model: &'a ModelRef,
    language: &'a LanguagePref,
    hint: Option<&'a str>,
    /// Decode again the speech a window leaves without text, per `windows::recovers`.
    recover: bool,
    meeting: &'a [LangTag],
    /// `SttCaps::lead_in`.
    lead_in: Ms,
}

fn transcribe(cx: StageCx<'_>) -> Result<Vec<Change>, StageError> {
    let id = &cx.job.recording;
    let store = cx.db.store.as_ref();
    let recording = store
        .recording(id)
        .map_err(failed)?
        .ok_or_else(|| permanent("the recording was deleted"))?;
    let take = recording.take.ok_or_else(|| {
        StageError::Failed(JobError {
            class: FailureClass::Permanent,
            message: "the recording has no audio".into(),
            cause: FailureCause::AudioUnreadable,
        })
    })?;
    let plan = plan_clips(recording.layout);
    let any_diarized = plan
        .iter()
        .any(|p| matches!(p.speakers, Speakers::Diarized { .. }));
    let needs = SttNeeds {
        language: recording.language.clone(),
        expected: cx.settings.meeting_languages.clone(),
        expected_guessed: cx.settings.meeting_languages_guessed,
        diarization: if any_diarized {
            DiarizationNeed::Required
        } else {
            DiarizationNeed::NotNeeded
        },
        input: take.duration,
    };
    let stt_id = settle(
        cx.models
            .get::<Stt>()
            .resolve(&cx.settings.stt, &needs, &[]),
    )?
    .ok_or(StageError::Blocked(Blocker::NoModel { kind: KindTag::Stt }))?;
    let caps = cx
        .models
        .get::<Stt>()
        .entry(&stt_id)
        .map(|e| e.caps.clone())
        .ok_or_else(|| permanent("the speech model left the catalog"))?;
    let native = caps.diarization == DiarizationSupport::Native;
    let language = run_language(&caps, &needs);
    let diar_id = if any_diarized && !native {
        settle(
            cx.models
                .get::<Diar>()
                .resolve(&cx.settings.diar, &DiarNeeds, &[]),
        )?
    } else {
        None
    };

    let budget = (cx.pace)();
    let stt = cx
        .engines
        .get::<Stt>(cx.models, &stt_id, cx.cancel, cx.now_mono)
        .map_err(engine_failed(stt_id.erase()))?;
    let diarizer = match diar_id {
        Some(d) => Some((
            d.erase(),
            cx.engines
                .get::<Diar>(cx.models, &d, cx.cancel, cx.now_mono)
                .map_err(engine_failed(d.erase()))?,
        )),
        None => None,
    };

    let model = stt_id.erase();
    let mut runs = Vec::new();
    for clip_plan in plan {
        let whole = cx.takes.clip(id, clip_plan.channel, None).map_err(failed)?;
        let diarize = native && matches!(clip_plan.speakers, Speakers::Diarized { .. });
        let cap = if diarize { None } else { caps.window };
        let key = CheckpointKey {
            recording: id.clone(),
            model: model.clone(),
            channel: clip_plan.channel,
            language: language.clone(),
        };
        runs.push(ClipRun {
            plan: clip_plan,
            windows: windows::plan_windows(whole.duration, &whole.activity, cap),
            saved: store.checkpoints(&key).map_err(failed)?,
            whole,
            diarize,
            key,
        });
    }

    let wx = WindowCx {
        db: cx.db,
        takes: cx.takes,
        cancel: cx.cancel,
        stt: stt.as_ref(),
        model: &model,
        language: &language,
        hint: recording.title.as_deref(),
        recover: windows::recovers(
            &caps,
            &language,
            &cx.settings.meeting_languages,
            cx.settings.switch_recovery,
        ),
        meeting: &cx.settings.meeting_languages,
        lead_in: caps.lead_in,
    };
    let total = runs
        .iter()
        .flat_map(|r| &r.windows)
        .map(|w| w.len().0)
        .sum();
    let mut progress = Progress {
        db: cx.db,
        job: cx.job,
        total,
        done: 0,
        budget,
    };
    let pinned = match &language {
        LanguagePref::Pinned(tag) => Some(tag.clone()),
        LanguagePref::Auto => None,
    };
    let mut segments = Vec::new();
    let mut speech = Ms(0);
    let mut words = 0;
    // Set by the meeting's first window, saved or decoded now, so a resume keeps it too.
    let mut locale: Option<Option<LangTag>> = None;
    for run in &runs {
        let mut raw = Vec::new();
        for span in &run.windows {
            let checkpoint = match run.saved.iter().find(|c| c.span == *span) {
                Some(saved) => {
                    for segment in &saved.segments {
                        publish_draft(cx.db, cx.job, run.plan, segment.clone());
                    }
                    saved.clone()
                }
                None => {
                    progress.start_window((cx.pace)());
                    let kept = locale.as_ref().and_then(Option::as_ref);
                    transcribe_window(&wx, run, *span, kept, &mut progress)?
                }
            };
            locale.get_or_insert_with(|| {
                kept_locale(pinned.as_ref(), checkpoint.detected_language.as_ref())
            });
            progress.finished(span.len());
            speech = Ms(speech.0 + windows::speech_time(&run.whole.activity, *span).0);
            words += checkpoint
                .segments
                .iter()
                .map(|s| s.text.split_whitespace().count())
                .sum::<usize>();
            let detected = checkpoint.detected_language;
            // An engine that detects nothing still spoke the language it was told to.
            raw.extend(checkpoint.segments.into_iter().map(|segment| {
                RawSegment {
                    language: segment
                        .language
                        .or_else(|| detected.clone())
                        .or_else(|| pinned.clone()),
                    ..segment
                }
            }));
        }
        let turns = match (&diarizer, run.plan.speakers) {
            (Some((model, diarizer)), Speakers::Diarized { .. }) if !raw.is_empty() => Some(
                diarizer
                    .diarize(&run.whole, &mut |_| {}, cx.cancel)
                    .map_err(engine_failed(model.clone()))?,
            ),
            _ => None,
        };
        segments.extend(place(raw, run.plan, turns.as_deref()));
    }
    if heard_nothing(speech, words) {
        return Err(StageError::Blocked(Blocker::Incompatible {
            model,
            why: Mismatch::NothingRecognized,
        }));
    }
    let revision = store
        .transcript(id)
        .map_err(failed)?
        .map_or(Revision(0), |t| t.revision);
    let mut transcript = Transcript {
        revision: Revision(revision.0 + 1),
        segments: merge::merge(segments),
        detected_languages: Vec::new(),
        produced_by: model,
    };
    transcript.detected_languages = transcript
        .language_shares()
        .into_iter()
        .map(|(tag, _)| tag)
        .collect();
    // A pinned meeting with nothing to time still spoke the language it was told to.
    if transcript.detected_languages.is_empty()
        && let LanguagePref::Pinned(tag) = &language
    {
        transcript.detected_languages.push(tag.clone());
    }
    Ok(vec![
        Change::CheckpointsCleared(id.clone()),
        Change::TranscriptReplaced {
            id: id.clone(),
            transcript,
        },
    ])
}

/// Runs one window through the engine and checkpoints what it produced. With recovery on, the
/// window's checkpoint also holds every stretch decoded again, so a resume never repeats one.
fn transcribe_window(
    wx: &WindowCx<'_>,
    run: &ClipRun,
    span: TimeRange,
    locale: Option<&LangTag>,
    progress: &mut Progress<'_>,
) -> Result<Checkpoint, StageError> {
    let started = std::time::Instant::now();
    let first = decode(wx, run, span, wx.language, locale, progress, Call::First)?;
    let heard = first.heard.clone();
    let (checkpoint, recovered) = if wx.recover {
        let first = settled(wx, run, span, first, progress)?;
        let speech = windows::speech_spans(&run.whole.activity, span);
        let said: Vec<TimeRange> = first.segments.iter().map(|s| s.range).collect();
        let chose = first.heard.clone();
        let mut segments = first.tagged();
        let recovered = windows::recover(&speech, &said, |stretch| {
            let again = decode(wx, run, stretch, wx.language, locale, progress, Call::Again)?;
            let again = settled(wx, run, stretch, again, progress)?;
            let said = again.segments.iter().map(|s| s.range).collect();
            segments.extend(again.tagged());
            Ok(said)
        })?;
        segments.sort_by_key(|s| s.range.start());
        for segment in &segments {
            publish_draft(wx.db, progress.job, run.plan, segment.clone());
        }
        let checkpoint = Checkpoint {
            span,
            segments,
            detected_language: chose,
        };
        (checkpoint, recovered)
    } else {
        let checkpoint = Checkpoint {
            span,
            segments: first.segments,
            detected_language: first.heard,
        };
        (checkpoint, 0)
    };
    log_window(
        wx.language,
        locale,
        heard.as_ref(),
        &checkpoint,
        recovered,
        started.elapsed(),
    );
    wx.db
        .apply(vec![Change::CheckpointSaved {
            key: run.key.clone(),
            checkpoint: checkpoint.clone(),
        }])
        .map_err(failed)?;
    Ok(checkpoint)
}

/// What one engine call produced, on the take's time axis.
struct Decoded {
    segments: Vec<RawSegment>,
    heard: Option<LangTag>,
}

impl Decoded {
    /// Each segment carries the language its call was heard in, so a window that holds recovered
    /// stretches keeps the language of each.
    fn tagged(self) -> Vec<RawSegment> {
        let heard = self.heard;
        self.segments
            .into_iter()
            .map(|s| RawSegment {
                language: s.language.or_else(|| heard.clone()),
                ..s
            })
            .collect()
    }
}

/// Which call of a window this is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Call {
    /// The window's first pass. It reports progress through the window, and publishes drafts as
    /// they come unless the window may still be decoded again.
    First,
    /// A redo or a recovered stretch, whose drafts wait until the window settles.
    Again,
}

/// Sends `span` of the clip to the engine, after up to the engine's lead-in of the audio before it.
fn decode(
    wx: &WindowCx<'_>,
    run: &ClipRun,
    span: TimeRange,
    language: &LanguagePref,
    locale: Option<&LangTag>,
    progress: &mut Progress<'_>,
    call: Call,
) -> Result<Decoded, StageError> {
    let from = Ms(span.start().0.saturating_sub(wx.lead_in.0)).max(run.whole.origin);
    let sent = TimeRange::new(from, span.end()).expect("the lead-in ends where the span starts");
    let fetched;
    let clip = if sent.start() == Ms(0) && sent.end() == run.whole.duration {
        &run.whole
    } else {
        fetched = wx.takes.slice(&run.whole, sent).map_err(failed)?;
        &fetched
    };
    let threads = progress.budget.threads();
    let mut sink = Drafts {
        plan: run.plan,
        origin: clip.origin,
        span,
        window: (call == Call::First).then_some(span.len()),
        live: call == Call::First && !wx.recover,
        progress,
        raw: Vec::new(),
    };
    let request = TranscribeRequest {
        clip,
        language: language.clone(),
        diarize: run.diarize,
        hint: wx.hint,
        threads,
        sentence_times: wx.recover,
        locale: locale.cloned(),
    };
    let outcome = wx
        .stt
        .transcribe(&request, &mut sink, wx.cancel)
        .map_err(engine_failed(wx.model.clone()))?;
    Ok(Decoded {
        segments: sink.raw,
        heard: outcome.detected_language,
    })
}

/// The locale a pinned meeting keeps once its first window heard `heard`: that tag, when it is a
/// variant of the pinned language. An automatic meeting keeps none, because each window may be
/// in another language.
fn kept_locale(pinned: Option<&LangTag>, heard: Option<&LangTag>) -> Option<LangTag> {
    let (pinned, heard) = (pinned?, heard?);
    (pinned.primary() == heard.primary()).then(|| heard.clone())
}

/// Decodes `span` again pinned to a meeting language when the engine heard one outside them.
fn settled(
    wx: &WindowCx<'_>,
    run: &ClipRun,
    span: TimeRange,
    decoded: Decoded,
    progress: &mut Progress<'_>,
) -> Result<Decoded, StageError> {
    match settle_language(decoded.heard.as_ref(), wx.meeting) {
        Settle::Keep => Ok(decoded),
        Settle::Redo(tag) => {
            let pinned = LanguagePref::Pinned(tag.clone());
            let again = decode(wx, run, span, &pinned, None, progress, Call::Again)?;
            Ok(Decoded {
                heard: Some(tag),
                ..again
            })
        }
    }
}

/// What to do with the language a window or stretch was heard in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Settle {
    Keep,
    /// Decode it again pinned to this meeting language.
    Redo(LangTag),
}

/// A language heard outside a non-empty list of meeting languages is a misheard window, so it is
/// decoded again pinned to the first meeting language. No language heard, or no meeting
/// languages, keeps what the engine wrote.
pub fn settle_language(heard: Option<&LangTag>, meeting: &[LangTag]) -> Settle {
    match (heard, meeting.first()) {
        (Some(heard), Some(first)) if !meeting.iter().any(|m| m.covers(heard)) => {
            Settle::Redo(first.clone())
        }
        _ => Settle::Keep,
    }
}

/// Three stderr lines per window, like `search ms=`, so a live run shows how long each window
/// took, which language the engine reported for it, and which language its text was kept in.
fn log_window(
    language: &LanguagePref,
    locale: Option<&LangTag>,
    heard: Option<&LangTag>,
    checkpoint: &Checkpoint,
    recovered: usize,
    elapsed: std::time::Duration,
) {
    let wanted = match language {
        LanguagePref::Auto => "auto",
        LanguagePref::Pinned(tag) => tag.as_str(),
    };
    let heard = heard
        .or_else(|| checkpoint.segments.iter().find_map(|s| s.language.as_ref()))
        .map_or("none", LangTag::as_str);
    let chose = checkpoint
        .detected_language
        .as_ref()
        .map_or(heard, LangTag::as_str);
    let (start, end) = (checkpoint.span.start().0, checkpoint.span.end().0);
    let kept = locale.map(|l| format!(" locale={l}")).unwrap_or_default();
    eprintln!("window ms={} lang={wanted}{kept}", elapsed.as_millis());
    eprintln!("language heard={heard} window={start}-{end}");
    eprintln!("language chose={chose} heard={heard} window={start}-{end} recovered={recovered}");
}

/// Turns one clip's raw segments, already on the take's time axis, into transcript segments.
/// Diarized speakers are numbered by first appearance across the whole clip, because an engine's
/// indices are arbitrary. A diarizer's turns, when given, decide the speaker by largest overlap.
pub(crate) fn place(raw: Vec<RawSegment>, plan: ClipPlan, turns: Option<&[Turn]>) -> Vec<Segment> {
    let mut seen: Vec<u16> = Vec::new();
    raw.into_iter()
        .map(|segment| {
            let speaker = match plan.speakers {
                Speakers::Fixed(id) => id,
                Speakers::Diarized { first } => {
                    let index = turns
                        .and_then(|t| dominant_speaker(t, segment.range))
                        .or(segment.speaker)
                        .unwrap_or(0);
                    let order = seen.iter().position(|s| *s == index).unwrap_or_else(|| {
                        seen.push(index);
                        seen.len() - 1
                    });
                    SpeakerId(first.saturating_add(u16::try_from(order).unwrap_or(u16::MAX)))
                }
            };
            Segment {
                range: Some(segment.range),
                speaker,
                channel: plan.channel,
                text: segment.text.trim().to_owned(),
                language: segment.language,
            }
        })
        .collect()
}

fn dominant_speaker(turns: &[Turn], range: TimeRange) -> Option<u16> {
    turns
        .iter()
        .map(|t| (t.range.overlap(&range), t.speaker))
        .filter(|(overlap, _)| *overlap > Ms(0))
        .max_by_key(|(overlap, _)| *overlap)
        .map(|(_, speaker)| speaker)
}

/// Speech in the windows a run sent that makes an empty transcript a mismatch, not a silent take.
const HEARD_NOTHING_SPEECH: Ms = Ms(10_000);

/// Whether a model was sent real speech and recognized no word of it.
fn heard_nothing(speech: Ms, words: usize) -> bool {
    speech >= HEARD_NOTHING_SPEECH && words == 0
}

/// Transcription progress as the share of planned speech done, over every clip of the job.
struct Progress<'a> {
    db: &'a Db,
    job: &'a Job,
    total: u64,
    done: u64,
    /// The running window's.
    budget: Budget,
}

impl Progress<'_> {
    fn publish(&self, done: f32) {
        self.db.publish(Event::StageProgress {
            job: self.job.id,
            stage: Stage::Transcribe,
            fraction: Fraction::new(done / self.total.max(1) as f32),
            budget: Some(self.budget),
        });
    }

    fn start_window(&mut self, budget: Budget) {
        self.budget = budget;
        self.publish(self.done as f32);
    }

    fn within(&self, window: Ms, fraction: Fraction) {
        self.publish(self.done as f32 + window.0 as f32 * fraction.get());
    }

    fn finished(&mut self, window: Ms) {
        self.done += window.0;
        self.publish(self.done as f32);
    }
}

fn publish_draft(db: &Db, job: &Job, plan: ClipPlan, segment: RawSegment) {
    if let Some(draft) = place(vec![segment], plan, None).pop() {
        db.publish(Event::DraftSegment {
            job: job.id,
            segment: draft,
        });
    }
}

struct Drafts<'p, 'a> {
    plan: ClipPlan,
    origin: Ms,
    /// What the call transcribes, on the take's time axis. The clip may start earlier, by the
    /// engine's lead-in, and a sentence centered before the span belongs to the window before.
    span: TimeRange,
    /// The window this call reports progress through, if any.
    window: Option<Ms>,
    /// Publish each segment as a draft as it comes.
    live: bool,
    progress: &'p mut Progress<'a>,
    /// On the take's time axis.
    raw: Vec<RawSegment>,
}

impl SegmentSink for Drafts<'_, '_> {
    fn progress(&mut self, fraction: Fraction) {
        if let Some(window) = self.window.filter(|w| w.0 > 0) {
            let lead = self.span.start().0 - self.origin.0;
            let heard = (lead + window.0) as f64;
            let into = (f64::from(fraction.get()) * heard - lead as f64).max(0.0) / window.0 as f64;
            self.progress.within(window, Fraction::new(into as f32));
        }
    }

    fn segment(&mut self, segment: RawSegment) {
        let segment = RawSegment {
            range: segment.range.shifted(self.origin),
            ..segment
        };
        let middle = (segment.range.start().0 + segment.range.end().0) / 2;
        if middle < self.span.start().0 {
            return;
        }
        if self.live {
            publish_draft(
                self.progress.db,
                self.progress.job,
                self.plan,
                segment.clone(),
            );
        }
        self.raw.push(segment);
    }
}

/// The languages a transcript's engine heard, as evidence of what the meeting used. A model that
/// runs in one locale reports only that locale, so what it heard says nothing about the rest of
/// the meeting and counts for nothing.
fn heard_languages<'a>(models: &Registries, transcript: &'a Transcript) -> &'a [LangTag] {
    let one_locale = transcript
        .produced_by
        .as_str()
        .parse::<ModelId<Stt>>()
        .ok()
        .and_then(|id| models.get::<Stt>().entry(&id))
        .is_some_and(|e| matches!(e.caps.languages, LangSupport::FixedLocale { .. }));
    if one_locale {
        &[]
    } else {
        &transcript.detected_languages
    }
}

/// Writes the transcript in the output language, beside the spoken one. The policy is in
/// `translate.rs`.
fn translate(cx: StageCx<'_>) -> Result<Vec<Change>, StageError> {
    // A stage queued before the user went back to Same as spoken has nothing to do.
    let Some(target) = cx.settings.output_language.clone() else {
        return Ok(Vec::new());
    };
    let id = &cx.job.recording;
    let store = cx.db.store.as_ref();
    let transcript = store
        .transcript(id)
        .map_err(failed)?
        .ok_or_else(|| permanent("there is no transcript to translate"))?;
    let todo = translate::pending(&transcript, &target);
    if todo.is_empty() {
        return Ok(Vec::new());
    }
    let recording = store
        .recording(id)
        .map_err(failed)?
        .ok_or_else(|| permanent("the recording was deleted"))?;
    // The model reads every spoken language and writes the target.
    let needs = LlmNeeds::new(
        Some(target.clone()),
        &recording.language,
        cx.settings.chosen_meeting_languages(),
        heard_languages(cx.models, &transcript),
    );
    let Some(llm_id) = settle(
        cx.models
            .get::<Llm>()
            .resolve(&cx.settings.llm, &needs, &[]),
    )?
    else {
        return Ok(Vec::new());
    };
    let context = cx
        .models
        .get::<Llm>()
        .entry(&llm_id)
        .map(|e| e.caps.context_tokens)
        .ok_or_else(|| permanent("the summary model left the catalog"))?;
    let model = llm_id.erase();
    let llm = cx
        .engines
        .get::<Llm>(cx.models, &llm_id, cx.cancel, cx.now_mono)
        .map_err(engine_failed(model.clone()))?;

    let key = TranslationKey {
        recording: id.clone(),
        revision: transcript.revision,
        target: target.clone(),
        model: model.clone(),
    };
    let mut done: BTreeMap<usize, String> = store
        .translation_batches(&key)
        .map_err(failed)?
        .into_iter()
        .flat_map(|batch| batch.texts)
        .filter(|(ix, _)| todo.contains(ix))
        .collect();
    let total = todo.len();
    let remaining: Vec<usize> = todo
        .iter()
        .copied()
        .filter(|ix| !done.contains_key(ix))
        .collect();

    let system = translate::system_prompt(&target);
    let system_tokens = llm.count_tokens(&system);
    let budget = translate::input_budget(context, system_tokens);
    if budget == 0 {
        return Err(permanent(
            "the summary model's context window is too small for its instructions",
        ));
    }
    let costs: Vec<u32> = remaining
        .iter()
        .map(|ix| llm.count_tokens(&translate::line(99, &transcript.segments[*ix].text)))
        .collect();
    for range in translate::plan_batches(&costs, budget) {
        if cx.cancel.is_cancelled() {
            return Err(failed(EngineError::Cancelled));
        }
        cx.db.publish(Event::StageProgress {
            job: cx.job.id,
            stage: Stage::Translate,
            fraction: Fraction::new(done.len() as f32 / total as f32),
            budget: Some((cx.pace)()),
        });
        let indexes = &remaining[range];
        let texts: Vec<&str> = indexes
            .iter()
            .map(|ix| transcript.segments[*ix].text.as_str())
            .collect();
        let lines = translate::translate_batch(
            &texts,
            &system,
            &|user| translate::max_output(context, system_tokens, llm.count_tokens(user)),
            &mut |call| {
                let request = CompletionRequest {
                    system: call.system,
                    user: call.user,
                    max_output_tokens: call.max_output_tokens,
                    temperature: 0.1,
                };
                llm.complete(&request, &mut Discard, cx.cancel)
            },
        )
        .map_err(engine_failed(model.clone()))?;
        // A blank reply for a spoken line keeps the line as spoken.
        let texts: Vec<(usize, String)> = indexes
            .iter()
            .copied()
            .zip(lines)
            .filter(|(_, text)| !text.trim().is_empty())
            .collect();
        cx.db
            .apply(vec![Change::TranslationBatchSaved {
                key: key.clone(),
                batch: TranslationBatch {
                    texts: texts.clone(),
                },
            }])
            .map_err(failed)?;
        done.extend(texts);
    }
    cx.db.publish(Event::StageProgress {
        job: cx.job.id,
        stage: Stage::Translate,
        fraction: Fraction::new(1.0),
        budget: Some((cx.pace)()),
    });

    let mut texts = vec![None; transcript.segments.len()];
    for (ix, text) in done {
        texts[ix] = Some(text);
    }
    Ok(vec![
        Change::TranslationReplaced {
            id: id.clone(),
            translation: Translation {
                target,
                transcript_revision: transcript.revision,
                produced_by: model,
                texts,
            },
        },
        Change::TranslationBatchesCleared(id.clone()),
    ])
}

/// Writes the stored summary in the language the user asked for, beside the original. The
/// model is resolved as Summarize resolves it, so the same blockers apply. The policy is in
/// `translate.rs`.
fn translate_summary(cx: StageCx<'_>) -> Result<Vec<Change>, StageError> {
    let id = &cx.job.recording;
    let store = cx.db.store.as_ref();
    // A summary written since the request took the wish with it, and nothing is left to do.
    let Some(target) = store.summary_translation_wanted(id).map_err(failed)? else {
        return Ok(Vec::new());
    };
    let summary = store
        .summary(id)
        .map_err(failed)?
        .ok_or_else(|| permanent("there is no summary to translate"))?;
    let recording = store
        .recording(id)
        .map_err(failed)?
        .ok_or_else(|| permanent("the recording was deleted"))?;
    // The model reads the summary's language and any meeting language it quotes, and writes the
    // target.
    let mut read = store
        .transcript(id)
        .map_err(failed)?
        .map(|t| heard_languages(cx.models, &t).to_vec())
        .unwrap_or_default();
    read.extend(summary.language.clone());
    let needs = LlmNeeds::new(
        Some(target.clone()),
        &recording.language,
        cx.settings.chosen_meeting_languages(),
        &read,
    );
    // Turning summaries off or having no model is not a quiet skip here: the user asked for this.
    let llm_id = settle(
        cx.models
            .get::<Llm>()
            .resolve(&cx.settings.llm, &needs, &[]),
    )?
    .ok_or(StageError::Blocked(Blocker::NoModel { kind: KindTag::Llm }))?;
    let context = cx
        .models
        .get::<Llm>()
        .entry(&llm_id)
        .map(|e| e.caps.context_tokens)
        .ok_or_else(|| permanent("the summary model left the catalog"))?;
    let model = llm_id.erase();
    let llm = cx
        .engines
        .get::<Llm>(cx.models, &llm_id, cx.cancel, cx.now_mono)
        .map_err(engine_failed(model.clone()))?;

    let system = translate::summary_system_prompt(&target);
    let system_tokens = llm.count_tokens(&system);
    let budget = translate::input_budget(context, system_tokens);
    if budget == 0 {
        return Err(permanent(
            "the summary model's context window is too small for its instructions",
        ));
    }
    let progress = |fraction: f32, pace: Budget| {
        cx.db.publish(Event::StageProgress {
            job: cx.job.id,
            stage: Stage::TranslateSummary,
            fraction: Fraction::new(fraction),
            budget: Some(pace),
        });
    };
    let markdown = translate::translate_summary(
        &summary.markdown,
        translate::SummaryRun {
            system: &system,
            budget,
            cost: &|text| llm.count_tokens(text),
            output: &|user| translate::max_output(context, system_tokens, llm.count_tokens(user)),
            before_batch: &mut |done, all| {
                if cx.cancel.is_cancelled() {
                    return Err(EngineError::Cancelled);
                }
                progress(done as f32 / all as f32, (cx.pace)());
                Ok(())
            },
            call: &mut |call| {
                let request = CompletionRequest {
                    system: call.system,
                    user: call.user,
                    max_output_tokens: call.max_output_tokens,
                    temperature: 0.1,
                };
                llm.complete(&request, &mut Discard, cx.cancel)
            },
        },
    )
    .map_err(engine_failed(model.clone()))?;
    let Some(markdown) = markdown else {
        return Ok(Vec::new());
    };
    progress(1.0, (cx.pace)());
    Ok(vec![Change::SummaryTranslated {
        id: id.clone(),
        translation: SummaryTranslation {
            target,
            produced_by: model,
            markdown,
        },
    }])
}

fn summarize(mut cx: StageCx<'_>) -> Result<Vec<Change>, StageError> {
    let id = &cx.job.recording;
    let store = cx.db.store.as_ref();
    let transcript = store
        .transcript(id)
        .map_err(failed)?
        .ok_or_else(|| permanent("there is no transcript to summarize"))?;
    // A model asked to summarize nothing writes a reply about the missing transcript.
    if transcript.segments.iter().all(|s| s.text.trim().is_empty()) {
        return Ok(Vec::new());
    }
    let recording = store
        .recording(id)
        .map_err(failed)?
        .ok_or_else(|| permanent("the recording was deleted"))?;
    // With an output language the summary is written in it, from the translated lines when a
    // translation of this revision exists. The model still reads every spoken language, because
    // a line left blank in translation stays as spoken.
    let target = cx.settings.output_language.clone();
    let translation = match &target {
        Some(target) => store
            .translation(id)
            .map_err(failed)?
            .filter(|t| t.applies_to(&transcript) && t.target == *target),
        None => None,
    };
    let mut needs = LlmNeeds::new(
        target.or_else(|| transcript.main_language().cloned()),
        &recording.language,
        cx.settings.chosen_meeting_languages(),
        heard_languages(cx.models, &transcript),
    );
    let names = store.speaker_names(id).map_err(failed)?;
    let lines = summary::transcript_lines(&transcript, &names, translation.as_ref());
    let instruction = store.summary_instruction(id).map_err(failed)?;
    // Models that refused this meeting. `Auto` resolves again without them and starts over from
    // the transcript, so notes from two models never mix.
    let mut refused: Vec<ModelId<Llm>> = Vec::new();
    let mut last_refusal = None;
    loop {
        let resolution = cx
            .models
            .get::<Llm>()
            .resolve(&cx.settings.llm, &needs, &refused);
        let llm_id = match (resolution, last_refusal) {
            (Resolution::Use(id), _) => id,
            (Resolution::Blocked(blocker), None) => return Err(StageError::Blocked(blocker)),
            (Resolution::Skip, None) => return Ok(Vec::new()),
            // Fall back only to a model that can run now. Otherwise the refusal is the answer, not
            // a blocker on a model that never saw the meeting.
            (_, Some((model, error))) => return Err(engine_failed(model)(error)),
        };
        let context = cx
            .models
            .get::<Llm>()
            .entry(&llm_id)
            .map(|e| e.caps.context_tokens)
            .ok_or_else(|| permanent("the summary model left the catalog"))?;
        let language = needs.main.as_ref();
        match summarize_with(
            &mut cx,
            &llm_id,
            &lines,
            language,
            instruction.as_deref(),
            context,
        ) {
            Ok(text) => {
                return Ok(vec![Change::SummaryReplaced {
                    id: id.clone(),
                    summary: Summary {
                        markdown: text.trim().to_owned(),
                        language: needs.main.clone(),
                        produced_by: llm_id.erase(),
                        transcript_revision: transcript.revision,
                    },
                }]);
            }
            Err(SummaryError::Engine(
                error @ (EngineError::UnsupportedLanguage(_) | EngineError::InputRejected(_)),
            )) if cx.settings.llm == Choice::Auto => {
                cx.db.publish(Event::DraftReset { job: cx.job.id });
                // A refusal that names the language rules out every other model that cannot read it.
                if let EngineError::UnsupportedLanguage(tag) = &error
                    && tag.as_str() != UNDETERMINED
                    && !needs.spoken.contains(tag)
                {
                    needs.spoken.push(tag.clone());
                }
                last_refusal = Some((llm_id.erase(), error));
                refused.push(llm_id);
            }
            Err(SummaryError::Engine(error)) => return Err(engine_failed(llm_id.erase())(error)),
            Err(SummaryError::ContextTooSmall) => {
                return Err(permanent(
                    "the summary model's context window is too small for its instructions",
                ));
            }
        }
    }
}

/// The BCP 47 tag for a language an engine could not name.
const UNDETERMINED: &str = "und";

/// One whole summary from the transcript with one model.
fn summarize_with(
    cx: &mut StageCx<'_>,
    llm_id: &ModelId<Llm>,
    lines: &[String],
    language: Option<&LangTag>,
    instruction: Option<&str>,
    context: u32,
) -> Result<String, SummaryError<EngineError>> {
    let llm = cx
        .engines
        .get::<Llm>(cx.models, llm_id, cx.cancel, cx.now_mono)
        .map_err(SummaryError::Engine)?;
    let mut drafts = SummaryDrafts {
        db: cx.db,
        job: cx.job,
    };
    let mut publish = |fraction: Fraction| {
        cx.db.publish(Event::StageProgress {
            job: cx.job.id,
            stage: Stage::Summarize,
            fraction,
            budget: None,
        });
    };
    publish(Fraction::new(0.0));
    summary::summarize(
        lines,
        language,
        instruction,
        context,
        &|text| llm.count_tokens(text),
        &mut |call| {
            let request = CompletionRequest {
                system: call.system,
                user: call.user,
                max_output_tokens: call.max_output_tokens,
                temperature: 0.2,
            };
            match call.pass {
                Pass::Summary => llm.complete(&request, &mut drafts, cx.cancel),
                Pass::Notes => llm.complete(&request, &mut Discard, cx.cancel),
            }
        },
        &mut publish,
    )
}

struct SummaryDrafts<'a> {
    db: &'a Db,
    job: &'a Job,
}

impl TextSink for SummaryDrafts<'_> {
    fn text(&mut self, piece: &str) {
        self.db.publish(Event::DraftSummary {
            job: self.job.id,
            text: piece.to_owned(),
        });
    }
}

/// Notes are a step toward the summary, not a draft of it.
struct Discard;

impl TextSink for Discard {
    fn text(&mut self, _: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(a: u64, b: u64) -> TimeRange {
        TimeRange::new(Ms(a), Ms(b)).unwrap()
    }

    fn raw(a: u64, b: u64, speaker: Option<u16>) -> RawSegment {
        RawSegment {
            range: r(a, b),
            text: format!(" {a} "),
            speaker,
            language: None,
        }
    }

    fn plan(channel: Channel, speakers: Speakers) -> ClipPlan {
        ClipPlan { channel, speakers }
    }

    #[test]
    fn only_mic_system_reserves_the_mic_for_the_local_user() {
        let mic_system = plan_clips(ChannelLayout::MicSystem);
        assert_eq!(
            mic_system,
            vec![
                plan(Channel::Mic, Speakers::Fixed(SpeakerId::LOCAL)),
                plan(Channel::System, Speakers::Diarized { first: 1 }),
            ]
        );
        for layout in [
            ChannelLayout::MicOnly,
            ChannelLayout::SystemOnly,
            ChannelLayout::Mixed,
        ] {
            let plan = plan_clips(layout);
            assert_eq!(plan.len(), 1, "{layout:?}");
            assert_eq!(plan[0].speakers, Speakers::Diarized { first: 1 });
            assert_eq!(plan[0].channel, layout.channels()[0]);
        }
    }

    #[test]
    fn diarized_speakers_are_renumbered_by_first_appearance_after_the_local_user() {
        let segments = place(
            vec![
                raw(1_000, 1_010, Some(7)),
                raw(1_010, 1_020, Some(3)),
                raw(1_020, 1_030, Some(7)),
                raw(1_030, 1_040, None),
            ],
            plan(Channel::System, Speakers::Diarized { first: 1 }),
            None,
        );
        let speakers: Vec<u16> = segments.iter().map(|s| s.speaker.0).collect();
        assert_eq!(speakers, vec![1, 2, 1, 3]);
        assert_eq!(segments[0].range, Some(r(1_000, 1_010)));
        assert_eq!(segments[0].text, "1000");
        assert!(segments.iter().all(|s| s.channel == Channel::System));
    }

    #[test]
    fn a_fixed_speaker_ignores_engine_indices() {
        let segments = place(
            vec![raw(0, 10, Some(4))],
            plan(Channel::Mic, Speakers::Fixed(SpeakerId::LOCAL)),
            None,
        );
        assert_eq!(segments[0].speaker, SpeakerId::LOCAL);
    }

    #[test]
    fn diarizer_turns_override_engine_indices_by_largest_overlap() {
        let turns = [
            Turn {
                range: r(0, 12),
                speaker: 5,
            },
            Turn {
                range: r(12, 40),
                speaker: 9,
            },
        ];
        let segments = place(
            vec![raw(0, 10, Some(1)), raw(10, 30, Some(1))],
            plan(Channel::System, Speakers::Diarized { first: 1 }),
            Some(&turns),
        );
        let speakers: Vec<u16> = segments.iter().map(|s| s.speaker.0).collect();
        assert_eq!(speakers, vec![1, 2]);
    }

    fn t(tag: &str) -> LangTag {
        LangTag::parse(tag).unwrap()
    }

    #[test]
    fn a_language_outside_the_meeting_languages_is_redone_in_the_first() {
        let meeting = [t("tl"), t("en")];
        assert_eq!(settle_language(Some(&t("en")), &meeting), Settle::Keep);
        assert_eq!(settle_language(Some(&t("en-US")), &meeting), Settle::Keep);
        assert_eq!(
            settle_language(Some(&t("es")), &meeting),
            Settle::Redo(t("tl"))
        );
        assert_eq!(settle_language(None, &meeting), Settle::Keep);
        assert_eq!(settle_language(Some(&t("es")), &[]), Settle::Keep);
        assert_eq!(settle_language(None, &[]), Settle::Keep);
    }

    #[test]
    fn heard_nothing_needs_ten_seconds_of_speech_and_no_words() {
        for (speech, words, want) in [
            (9_999, 0, false),
            (10_000, 0, true),
            (30_000, 0, true),
            (10_000, 1, false),
            (0, 0, false),
            (30_000, 12, false),
        ] {
            assert_eq!(
                heard_nothing(Ms(speech), words),
                want,
                "{speech} ms, {words} words"
            );
        }
    }
}
