//! What the transcribe, translate, and summarize stages do. Each returns the changes to commit with its
//! `StageDone`, or why it could not finish.
//!
//! Transcribe plans every clip's windows first, so progress covers the whole job. Each finished
//! window is checkpointed on its own, and a rerun skips the windows its run already finished.

mod merge;
mod summary;
mod translate;
mod windows;

use std::collections::BTreeMap;

use super::events::{Db, Event};
use super::models::{Engines, Registries};
use super::pace::Budget;
use crate::domain::{
    Blocker, Channel, ChannelLayout, FailureCause, FailureClass, Fraction, Job, JobError, LangTag,
    LanguagePref, Ms, Revision, Segment, Settings, SpeakerId, Stage, Summary, TimeRange,
    Transcript, Translation,
};
use crate::models::{
    Diar, DiarNeeds, DiarizationNeed, DiarizationSupport, KindTag, Llm, LlmNeeds, Mismatch,
    ModelId, ModelKind, ModelRef, Resolution, Stt, SttNeeds,
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

/// An engine error with the model it came from, so a missing model or language becomes a
/// blocker the user can act on.
fn engine_failed(model: ModelRef) -> impl FnOnce(EngineError) -> StageError {
    move |error| match error {
        EngineError::ModelMissing(_) => {
            StageError::Blocked(Blocker::NeedsProvision { model, bytes: None })
        }
        EngineError::UnsupportedLanguage(_) => StageError::Blocked(Blocker::Incompatible {
            model,
            why: Mismatch::LanguageUnsupported,
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
        diarization: if any_diarized {
            DiarizationNeed::Required
        } else {
            DiarizationNeed::NotNeeded
        },
        input: take.duration,
    };
    let stt_id = settle(cx.models.get::<Stt>().resolve(&cx.settings.stt, &needs))?
        .ok_or(StageError::Blocked(Blocker::NoModel { kind: KindTag::Stt }))?;
    let caps = cx
        .models
        .get::<Stt>()
        .entry(&stt_id)
        .map(|e| e.caps.clone())
        .ok_or_else(|| permanent("the speech model left the catalog"))?;
    let native = caps.diarization == DiarizationSupport::Native;
    let diar_id = if any_diarized && !native {
        settle(
            cx.models
                .get::<Diar>()
                .resolve(&cx.settings.diar, &DiarNeeds),
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
            language: recording.language.clone(),
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
        language: &recording.language,
        hint: recording.title.as_deref(),
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
    let pinned = match &recording.language {
        LanguagePref::Pinned(tag) => Some(tag.clone()),
        LanguagePref::Auto => None,
    };
    let mut segments = Vec::new();
    let mut languages = Vec::new();
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
                    transcribe_window(&wx, run, *span, &mut progress)?
                }
            };
            progress.finished(span.len());
            languages.extend(
                checkpoint
                    .segments
                    .iter()
                    .filter_map(|s| s.language.clone().or(checkpoint.detected_language.clone())),
            );
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
    if languages.is_empty()
        && let LanguagePref::Pinned(tag) = &recording.language
    {
        languages.push(tag.clone());
    }

    let revision = store
        .transcript(id)
        .map_err(failed)?
        .map_or(Revision(0), |t| t.revision);
    Ok(vec![
        Change::CheckpointsCleared(id.clone()),
        Change::TranscriptReplaced {
            id: id.clone(),
            transcript: Transcript {
                revision: Revision(revision.0 + 1),
                segments: merge::merge(segments),
                detected_languages: rank_languages(languages),
                produced_by: model,
            },
        },
    ])
}

/// Runs one window through the engine and checkpoints what it produced.
fn transcribe_window(
    wx: &WindowCx<'_>,
    run: &ClipRun,
    span: TimeRange,
    progress: &mut Progress<'_>,
) -> Result<Checkpoint, StageError> {
    let fetched;
    let clip = if span.start() == Ms(0) && span.end() == run.whole.duration {
        &run.whole
    } else {
        fetched = wx.takes.slice(&run.whole, span).map_err(failed)?;
        &fetched
    };
    let threads = progress.budget.threads();
    let mut sink = Drafts {
        plan: run.plan,
        origin: clip.origin,
        window: span.len(),
        progress,
        raw: Vec::new(),
    };
    let request = TranscribeRequest {
        clip,
        language: wx.language.clone(),
        diarize: run.diarize,
        hint: wx.hint,
        threads,
    };
    let outcome = wx
        .stt
        .transcribe(&request, &mut sink, wx.cancel)
        .map_err(engine_failed(wx.model.clone()))?;
    let checkpoint = Checkpoint {
        span,
        segments: sink.raw,
        detected_language: outcome.detected_language,
    };
    wx.db
        .apply(vec![Change::CheckpointSaved {
            key: run.key.clone(),
            checkpoint: checkpoint.clone(),
        }])
        .map_err(failed)?;
    Ok(checkpoint)
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

/// Most used first, ties in order of first appearance.
fn rank_languages(tags: Vec<LangTag>) -> Vec<LangTag> {
    let mut counts: Vec<(LangTag, usize)> = Vec::new();
    for tag in tags {
        match counts.iter_mut().find(|(t, _)| *t == tag) {
            Some((_, n)) => *n += 1,
            None => counts.push((tag, 1)),
        }
    }
    counts.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    counts.into_iter().map(|(t, _)| t).collect()
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
    window: Ms,
    progress: &'p mut Progress<'a>,
    /// On the take's time axis.
    raw: Vec<RawSegment>,
}

impl SegmentSink for Drafts<'_, '_> {
    fn progress(&mut self, fraction: Fraction) {
        self.progress.within(self.window, fraction);
    }

    fn segment(&mut self, segment: RawSegment) {
        let segment = RawSegment {
            range: segment.range.shifted(self.origin),
            ..segment
        };
        publish_draft(
            self.progress.db,
            self.progress.job,
            self.plan,
            segment.clone(),
        );
        self.raw.push(segment);
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
    let needs = LlmNeeds {
        language: Some(target.clone()),
    };
    let Some(llm_id) = settle(cx.models.get::<Llm>().resolve(&cx.settings.llm, &needs))? else {
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

fn summarize(cx: StageCx<'_>) -> Result<Vec<Change>, StageError> {
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
    // With an output language the summary is written in it, from the translated lines when a
    // translation of this revision exists. Without one nothing here differs from before.
    let target = cx.settings.output_language.clone();
    let translation = match &target {
        Some(target) => store
            .translation(id)
            .map_err(failed)?
            .filter(|t| t.applies_to(&transcript) && t.target == *target),
        None => None,
    };
    let language = target.or_else(|| transcript.main_language().cloned());
    let needs = LlmNeeds {
        language: language.clone(),
    };
    let Some(llm_id) = settle(cx.models.get::<Llm>().resolve(&cx.settings.llm, &needs))? else {
        return Ok(Vec::new());
    };
    let context = cx
        .models
        .get::<Llm>()
        .entry(&llm_id)
        .map(|e| e.caps.context_tokens)
        .ok_or_else(|| permanent("the summary model left the catalog"))?;
    let names = store.speaker_names(id).map_err(failed)?;
    let lines = summary::transcript_lines(&transcript, &names, translation.as_ref());
    let llm = cx
        .engines
        .get::<Llm>(cx.models, &llm_id, cx.cancel, cx.now_mono)
        .map_err(engine_failed(llm_id.erase()))?;
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
    let instruction = store.summary_instruction(id).map_err(failed)?;
    let text = summary::summarize(
        &lines,
        language.as_ref(),
        instruction.as_deref(),
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
    .map_err(|error| match error {
        SummaryError::Engine(error) => engine_failed(llm_id.erase())(error),
        SummaryError::ContextTooSmall => {
            permanent("the summary model's context window is too small for its instructions")
        }
    })?;
    Ok(vec![Change::SummaryReplaced {
        id: id.clone(),
        summary: Summary {
            markdown: text.trim().to_owned(),
            language,
            produced_by: llm_id.erase(),
            transcript_revision: transcript.revision,
        },
    }])
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

    #[test]
    fn languages_rank_by_use_then_first_appearance() {
        let t = |s: &str| LangTag::parse(s).unwrap();
        assert_eq!(
            rank_languages(vec![t("en"), t("vi"), t("vi"), t("fr"), t("en"), t("vi")]),
            vec![t("vi"), t("en"), t("fr")]
        );
    }
}
