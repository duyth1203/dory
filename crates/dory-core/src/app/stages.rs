//! What the transcribe and summarize stages do. Each returns the changes to commit with its
//! `StageDone`, or why it could not finish.
//!
//! P1 runs each clip in one engine call. Windowing at silence, checkpoints, mic bleed dedupe, and
//! map-reduce summaries come later.

use std::fmt::Write as _;

use super::events::{Event, EventHub};
use super::models::{Engines, Registries};
use crate::domain::{
    Blocker, Channel, ChannelLayout, FailureClass, Fraction, Job, JobError, LangTag, Ms, Revision,
    Segment, Settings, SpeakerId, SpeakerNames, Stage, Summary, TimeRange, Transcript,
};
use crate::models::{
    Diar, DiarNeeds, DiarizationNeed, DiarizationSupport, KindTag, Llm, LlmNeeds, Mismatch,
    ModelId, ModelKind, ModelRef, Resolution, Stt, SttNeeds,
};
use crate::ports::{
    CancelToken, Change, Classify, Clip, CompletionRequest, EngineError, RawSegment, SegmentSink,
    StoreRead, Takes, TextSink, TranscribeRequest, Turn,
};

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
    })
}

fn permanent(message: &str) -> StageError {
    StageError::Failed(JobError {
        class: FailureClass::Permanent,
        message: message.to_owned(),
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
    pub store: &'a dyn StoreRead,
    pub takes: &'a dyn Takes,
    pub models: &'a Registries,
    pub engines: &'a mut Engines,
    pub hub: &'a EventHub,
    pub cancel: &'a CancelToken,
    pub settings: &'a Settings,
    pub now_mono: u64,
}

pub(crate) fn run(stage: Stage, cx: StageCx<'_>) -> Result<Vec<Change>, StageError> {
    match stage {
        Stage::Transcribe => transcribe(cx),
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

fn transcribe(cx: StageCx<'_>) -> Result<Vec<Change>, StageError> {
    let id = &cx.job.recording;
    let recording = cx
        .store
        .recording(id)
        .map_err(failed)?
        .ok_or_else(|| permanent("the recording was deleted"))?;
    let take = recording
        .take
        .ok_or_else(|| permanent("the recording has no audio"))?;
    let plan = plan_clips(recording.layout);
    let any_diarized = plan
        .iter()
        .any(|p| matches!(p.speakers, Speakers::Diarized { .. }));
    let needs = SttNeeds {
        language: recording.language.clone(),
        diarization: if any_diarized {
            DiarizationNeed::Required
        } else {
            DiarizationNeed::NotNeeded
        },
        input: take.duration,
    };
    let stt_id = settle(cx.models.get::<Stt>().resolve(&cx.settings.stt, &needs))?
        .ok_or(StageError::Blocked(Blocker::NoModel { kind: KindTag::Stt }))?;
    let native = cx
        .models
        .get::<Stt>()
        .entry(&stt_id)
        .is_some_and(|e| e.caps.diarization == DiarizationSupport::Native);
    let diar_id = if any_diarized && !native {
        settle(
            cx.models
                .get::<Diar>()
                .resolve(&cx.settings.diar, &DiarNeeds),
        )?
    } else {
        None
    };

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

    let mut segments = Vec::new();
    let mut languages = Vec::new();
    let clips = plan.len() as f32;
    for (index, clip_plan) in plan.iter().enumerate() {
        let clip = cx.takes.clip(id, clip_plan.channel, None).map_err(failed)?;
        let mut sink = Drafts {
            hub: cx.hub,
            job: cx.job,
            clip: &clip,
            plan: *clip_plan,
            base: index as f32 / clips,
            scale: 1.0 / clips,
            raw: Vec::new(),
        };
        let request = TranscribeRequest {
            clip: &clip,
            language: recording.language.clone(),
            diarize: native && matches!(clip_plan.speakers, Speakers::Diarized { .. }),
            hint: recording.title.as_deref(),
        };
        let outcome = stt
            .transcribe(&request, &mut sink, cx.cancel)
            .map_err(engine_failed(stt_id.erase()))?;
        let raw = sink.raw;
        let turns = match (&diarizer, clip_plan.speakers) {
            (Some((model, diarizer)), Speakers::Diarized { .. }) => Some(
                diarizer
                    .diarize(&clip, &mut |_| {}, cx.cancel)
                    .map_err(engine_failed(model.clone()))?,
            ),
            _ => None,
        };
        languages.extend(
            raw.iter()
                .filter_map(|s| s.language.clone().or(outcome.detected_language.clone())),
        );
        segments.extend(place(raw, &clip, clip_plan.speakers, turns.as_deref()));
    }
    segments.sort_by_key(|s| s.range.map(|r| r.start()));
    if languages.is_empty()
        && let crate::domain::LanguagePref::Pinned(tag) = &recording.language
    {
        languages.push(tag.clone());
    }

    let revision = cx
        .store
        .transcript(id)
        .map_err(failed)?
        .map_or(Revision(0), |t| t.revision);
    Ok(vec![Change::TranscriptReplaced {
        id: id.clone(),
        transcript: Transcript {
            revision: Revision(revision.0 + 1),
            segments,
            detected_languages: rank_languages(languages),
            produced_by: stt_id.erase(),
        },
    }])
}

/// Turns one clip's raw segments into transcript segments on the take's time axis. Diarized
/// speakers are numbered by first appearance, because an engine's indices are arbitrary and only
/// hold within one call. A diarizer's turns, when given, decide the speaker by largest overlap.
pub(crate) fn place(
    raw: Vec<RawSegment>,
    clip: &Clip,
    speakers: Speakers,
    turns: Option<&[Turn]>,
) -> Vec<Segment> {
    let mut seen: Vec<u16> = Vec::new();
    raw.into_iter()
        .map(|segment| {
            let speaker = match speakers {
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
                range: Some(segment.range.shifted(clip.origin)),
                speaker,
                channel: clip.channel,
                text: segment.text.trim().to_owned(),
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

struct Drafts<'a> {
    hub: &'a EventHub,
    job: &'a Job,
    clip: &'a Clip,
    plan: ClipPlan,
    base: f32,
    scale: f32,
    raw: Vec<RawSegment>,
}

impl SegmentSink for Drafts<'_> {
    fn progress(&mut self, fraction: Fraction) {
        self.hub.publish(Event::StageProgress {
            job: self.job.id,
            stage: Stage::Transcribe,
            fraction: Fraction::new(self.base + fraction.get() * self.scale),
        });
    }

    fn segment(&mut self, segment: RawSegment) {
        if let Some(draft) = place(vec![segment.clone()], self.clip, self.plan.speakers, None).pop()
        {
            self.hub.publish(Event::DraftSegment {
                job: self.job.id,
                segment: draft,
            });
        }
        self.raw.push(segment);
    }
}

fn summarize(cx: StageCx<'_>) -> Result<Vec<Change>, StageError> {
    let id = &cx.job.recording;
    let transcript = cx
        .store
        .transcript(id)
        .map_err(failed)?
        .ok_or_else(|| permanent("there is no transcript to summarize"))?;
    let language = transcript.main_language().cloned();
    let needs = LlmNeeds {
        language: language.clone(),
    };
    let Some(llm_id) = settle(cx.models.get::<Llm>().resolve(&cx.settings.llm, &needs))? else {
        return Ok(Vec::new());
    };
    let names = cx.store.speaker_names(id).map_err(failed)?;
    let prompt = SummaryPrompt::new(&transcript, &names);
    let llm = cx
        .engines
        .get::<Llm>(cx.models, &llm_id, cx.cancel, cx.now_mono)
        .map_err(engine_failed(llm_id.erase()))?;
    let request = CompletionRequest {
        system: &prompt.system,
        user: &prompt.user,
        max_output_tokens: 1024,
        temperature: 0.2,
    };
    let mut sink = SummaryDrafts {
        hub: cx.hub,
        job: cx.job,
    };
    let text = llm
        .complete(&request, &mut sink, cx.cancel)
        .map_err(engine_failed(llm_id.erase()))?;
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
    hub: &'a EventHub,
    job: &'a Job,
}

impl TextSink for SummaryDrafts<'_> {
    fn text(&mut self, piece: &str) {
        self.hub.publish(Event::DraftSummary {
            job: self.job.id,
            text: piece.to_owned(),
        });
    }
}

pub(crate) struct SummaryPrompt {
    pub system: String,
    pub user: String,
}

impl SummaryPrompt {
    pub fn new(transcript: &Transcript, names: &SpeakerNames) -> SummaryPrompt {
        let language = match transcript.main_language() {
            Some(tag) => format!("the language with BCP 47 tag {tag}"),
            None => "the language the meeting was held in".to_owned(),
        };
        let system = format!(
            "You summarize meeting transcripts. Write the whole summary, headings included, in \
             {language}. Use Markdown with three sections in this order: key points, decisions, \
             and action items. Give each action item its owner when the transcript names one. \
             Write \"None\" under a section with nothing to report. Use only what the transcript \
             says."
        );
        let mut user = String::new();
        for segment in &transcript.segments {
            if let Some(range) = segment.range {
                let secs = range.start().0 / 1000;
                let _ = write!(user, "[{:02}:{:02}] ", secs / 60, secs % 60);
            }
            let _ = writeln!(
                user,
                "{}: {}",
                speaker_label(segment, names, transcript.revision),
                segment.text
            );
        }
        SummaryPrompt { system, user }
    }
}

fn speaker_label(segment: &Segment, names: &SpeakerNames, revision: Revision) -> String {
    match names.name_of(segment.speaker, revision) {
        Some(name) => name.to_owned(),
        None if segment.speaker == SpeakerId::LOCAL && segment.channel == Channel::Mic => {
            "Me".to_owned()
        }
        None => format!("Speaker {}", segment.speaker.0),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

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

    fn clip(channel: Channel, origin: u64) -> Clip {
        Clip::new(
            PathBuf::from("x"),
            channel,
            Ms(60_000),
            Ms(origin),
            vec![],
            None,
        )
    }

    #[test]
    fn only_mic_system_reserves_the_mic_for_the_local_user() {
        let mic_system = plan_clips(ChannelLayout::MicSystem);
        assert_eq!(
            mic_system,
            vec![
                ClipPlan {
                    channel: Channel::Mic,
                    speakers: Speakers::Fixed(SpeakerId::LOCAL)
                },
                ClipPlan {
                    channel: Channel::System,
                    speakers: Speakers::Diarized { first: 1 }
                },
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
                raw(0, 10, Some(7)),
                raw(10, 20, Some(3)),
                raw(20, 30, Some(7)),
                raw(30, 40, None),
            ],
            &clip(Channel::System, 1_000),
            Speakers::Diarized { first: 1 },
            None,
        );
        let speakers: Vec<u16> = segments.iter().map(|s| s.speaker.0).collect();
        assert_eq!(speakers, vec![1, 2, 1, 3]);
        assert_eq!(
            segments[0].range,
            Some(r(1_000, 1_010)),
            "shifted to take time"
        );
        assert_eq!(segments[0].text, "0");
        assert!(segments.iter().all(|s| s.channel == Channel::System));
    }

    #[test]
    fn a_fixed_speaker_ignores_engine_indices() {
        let segments = place(
            vec![raw(0, 10, Some(4))],
            &clip(Channel::Mic, 0),
            Speakers::Fixed(SpeakerId::LOCAL),
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
            &clip(Channel::System, 0),
            Speakers::Diarized { first: 1 },
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

    #[test]
    fn the_prompt_names_the_language_and_labels_speakers() {
        let segment = |speaker: u16, channel, start: u64, text: &str| Segment {
            range: Some(r(start, start + 1_000)),
            speaker: SpeakerId(speaker),
            channel,
            text: text.into(),
        };
        let transcript = Transcript {
            revision: Revision(2),
            segments: vec![
                segment(0, Channel::Mic, 0, "Ship it Friday."),
                segment(1, Channel::System, 65_000, "Agreed."),
                segment(2, Channel::System, 70_000, "I'll write the notes."),
            ],
            detected_languages: vec![LangTag::parse("vi").unwrap()],
            produced_by: ModelRef::parse("fake/stt").unwrap(),
        };
        let mut names = SpeakerNames::default();
        names.rename(Revision(2), SpeakerId(2), Some("Ana".into()));
        let prompt = SummaryPrompt::new(&transcript, &names);
        assert!(prompt.system.contains("BCP 47 tag vi"));
        assert!(prompt.system.contains("decisions"));
        assert!(prompt.system.contains("action items"));
        assert_eq!(
            prompt.user,
            "[00:00] Me: Ship it Friday.\n[01:05] Speaker 1: Agreed.\n[01:10] Ana: I'll write the notes.\n"
        );
    }
}
