//! Imports the old Tauri app's data folder.
//!
//! The old app keeps `recordings/<id>.webm|wav` and `metadata/<id>.json` (transcript and summary as
//! text, a pipeline status string, seconds-since-epoch creation time). This module reads them and
//! writes only to the new store and to the new takes folder. The old folder is opened read-only,
//! so the Tauri app keeps working on it.
//!
//! One recording is one transaction, keyed by the SHA-256 of its metadata. A file whose hash is
//! already recorded is skipped, so a second run changes nothing, and a run killed halfway resumes
//! where it stopped. A file whose content changed since the last import (the old app finished a
//! transcription meanwhile) updates the recording. A recording deleted in the new app stays
//! deleted until its old metadata changes.
//!
//! What the old format could not say is filled in plainly: the layout is `Mixed`, the transcript
//! has no times, and the model is `legacy/unknown`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use dory_core::domain::{
    Channel, ChannelLayout, FailureClass, Goal, JobError, JobEvent, JobId, JobState, LanguagePref,
    Ms, Priority, Recording, RecordingEdit, RecordingId, Revision, Segment, SpeakerId, Stage,
    Summary, TakeContainer, TakeInfo, Timestamp, Transcript,
};
use dory_core::models::ModelRef;
use dory_core::ports::{Change, StoreError};
use rusqlite::{OptionalExtension, params};
use serde_json::Value;
use thiserror::Error;

use crate::sha256::sha256_hex;
use crate::sqlite::{
    SqliteStore, apply_change, db, load_job, load_summary, load_transcript, recording_exists,
};
use crate::takes::FsTakes;

/// What a run did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// Recordings new to the store.
    pub imported: Vec<RecordingId>,
    /// Recordings whose old metadata changed since they were imported.
    pub updated: Vec<RecordingId>,
    /// Recordings whose old metadata is the same as when they were last imported.
    pub unchanged: usize,
    /// Files that could not be imported, each with the reason. The rest still imported.
    pub skipped: Vec<Skipped>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    pub path: PathBuf,
    pub reason: String,
}

impl std::fmt::Display for ImportReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} new, {} updated, {} unchanged, {} skipped",
            self.imported.len(),
            self.updated.len(),
            self.unchanged,
            self.skipped.len()
        )?;
        for s in &self.skipped {
            write!(f, "\n  skipped {}: {}", s.path.display(), s.reason)?;
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("{0} has neither a metadata nor a recordings folder, so it is not a Dory data folder")]
    NotADataFolder(PathBuf),
    #[error("cannot read {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Imports every recording in `old_data_dir` that is new or changed since the last run.
pub fn import_legacy(
    store: &SqliteStore,
    takes: &FsTakes,
    old_data_dir: &Path,
) -> Result<ImportReport, ImportError> {
    let root = old_data_dir
        .canonicalize()
        .map_err(|source| ImportError::Io {
            path: old_data_dir.to_owned(),
            source,
        })?;
    let meta_dir = root.join("metadata");
    let rec_dir = root.join("recordings");
    if !meta_dir.is_dir() && !rec_dir.is_dir() {
        return Err(ImportError::NotADataFolder(root));
    }

    let mut stems = BTreeSet::new();
    for (dir, exts) in [(&meta_dir, &["json"][..]), (&rec_dir, &["webm", "wav"][..])] {
        if !dir.is_dir() {
            continue;
        }
        let entries = std::fs::read_dir(dir).map_err(|source| ImportError::Io {
            path: dir.clone(),
            source,
        })?;
        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default();
            if let (true, Some(stem)) = (
                exts.contains(&ext),
                path.file_stem().and_then(|s| s.to_str()),
            ) {
                stems.insert(stem.to_owned());
            }
        }
    }

    let mut report = ImportReport::default();
    for stem in stems {
        let meta_path = meta_dir.join(format!("{stem}.json"));
        let outcome = import_one(store, takes, &stem, &rec_dir, &meta_path);
        match outcome {
            Ok(Outcome::Imported(id)) => report.imported.push(id),
            Ok(Outcome::Updated(id)) => report.updated.push(id),
            Ok(Outcome::Unchanged) => report.unchanged += 1,
            Err(reason) => report.skipped.push(Skipped {
                path: if meta_path.is_file() {
                    meta_path
                } else {
                    rec_dir.join(&stem)
                },
                reason,
            }),
        }
    }
    Ok(report)
}

enum Outcome {
    Imported(RecordingId),
    Updated(RecordingId),
    Unchanged,
}

fn audio_for(rec_dir: &Path, stem: &str) -> Option<(PathBuf, TakeContainer)> {
    [
        ("webm", TakeContainer::LegacyWebm),
        ("wav", TakeContainer::Wav),
    ]
    .into_iter()
    .map(|(ext, container)| (rec_dir.join(format!("{stem}.{ext}")), container))
    .find(|(path, _)| path.is_file())
}

fn import_one(
    store: &SqliteStore,
    takes: &FsTakes,
    stem: &str,
    rec_dir: &Path,
    meta_path: &Path,
) -> Result<Outcome, String> {
    let id = RecordingId::parse(stem).map_err(|e| e.to_string())?;
    let (audio, container) = audio_for(rec_dir, stem).ok_or("there is no audio file for it")?;

    let (hash, meta) = if meta_path.is_file() {
        let bytes = std::fs::read(meta_path).map_err(|e| format!("cannot read it: {e}"))?;
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|e| format!("corrupt JSON: {e}"))?;
        if !value.is_object() {
            return Err("corrupt JSON: the top level is not an object".into());
        }
        (sha256_hex(&bytes), value)
    } else {
        // Audio with no metadata file is listed by the old app too, with nothing else known.
        let len = std::fs::metadata(&audio).map(|m| m.len()).unwrap_or(0);
        let name = audio
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        (
            sha256_hex(format!("audio-only\n{name}\n{len}").as_bytes()),
            Value::Object(Default::default()),
        )
    };

    let known: Option<(String, Option<i64>)> = store
        .write(|tx| {
            tx.prepare_cached(
                "SELECT content_hash, job_id FROM legacy_imports WHERE recording_id = ?1",
            )
            .map_err(db)?
            .query_row([id.as_str()], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()
            .map_err(db)
        })
        .map_err(|e| e.to_string())?;
    if known.as_ref().is_some_and(|(old, _)| *old == hash) {
        return Ok(Outcome::Unchanged);
    }

    let legacy = Legacy::parse(&meta, &audio, container);
    let take = TakeInfo {
        container,
        rate_hz: legacy.rate_hz,
        duration: legacy.duration,
    };
    takes
        .link_legacy(&id, &audio, container, take.rate_hz, take.duration)
        .map_err(|e| e.to_string())?;

    let updated = store
        .write(|tx| {
            let existed = recording_exists(tx, &id)?;
            let old_job = known.as_ref().and_then(|(_, job)| *job);
            write_recording(tx, &id, &legacy, take, existed, old_job, &hash)?;
            Ok(existed)
        })
        .map_err(|e| e.to_string())?;
    Ok(if updated {
        Outcome::Updated(id)
    } else {
        Outcome::Imported(id)
    })
}

/// What the old metadata said, with every missing or mistyped field already defaulted.
struct Legacy {
    title: Option<String>,
    created_at: Timestamp,
    duration: Ms,
    rate_hz: u32,
    transcript: Option<String>,
    summary: Option<String>,
    status: Option<String>,
    error: Option<String>,
    front: bool,
}

fn text_field(meta: &Value, key: &str) -> Option<String> {
    meta.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

impl Legacy {
    fn parse(meta: &Value, audio: &Path, container: TakeContainer) -> Legacy {
        let file_time = || {
            std::fs::metadata(audio)
                .ok()
                .and_then(|m| m.created().or_else(|_| m.modified()).ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0))
        };
        let created_ms = meta
            .get("created_at")
            .and_then(Value::as_u64)
            .filter(|secs| *secs > 0)
            .map_or_else(file_time, |secs| {
                i64::try_from(secs.saturating_mul(1000)).unwrap_or(i64::MAX)
            });
        let secs = meta
            .get("duration_secs")
            .and_then(Value::as_f64)
            .filter(|d| d.is_finite() && *d > 0.0);
        let (wav_rate, wav_duration) = match container {
            TakeContainer::Wav => wav_facts(audio),
            TakeContainer::Flac | TakeContainer::LegacyWebm => (None, None),
        };
        Legacy {
            title: text_field(meta, "display_name"),
            created_at: Timestamp(created_ms),
            duration: secs
                .map(|d| Ms((d * 1000.0).round() as u64))
                .or(wav_duration)
                .unwrap_or(Ms(0)),
            // MediaRecorder's Opus in WebM is always 48 kHz.
            rate_hz: wav_rate.unwrap_or(48_000),
            transcript: text_field(meta, "transcript"),
            summary: text_field(meta, "summary"),
            status: text_field(meta, "pipeline_status"),
            error: text_field(meta, "pipeline_error"),
            front: meta
                .get("pipeline_priority")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }
    }
}

fn wav_facts(path: &Path) -> (Option<u32>, Option<Ms>) {
    let Ok(reader) = hound::WavReader::open(path) else {
        return (None, None);
    };
    let spec = reader.spec();
    let frames = u64::from(reader.duration());
    let duration =
        std::num::NonZeroU32::new(spec.sample_rate).map(|rate| Ms::from_frames(frames, rate));
    (Some(spec.sample_rate), duration)
}

/// Parses the old `[Speaker N] text` lines into untimed segments. `[Speaker N]` keeps its number,
/// because `SpeakerId(0)` means the local user and a legacy mixed take has no mic channel. A line
/// with no label belongs to the first speaker.
pub(crate) fn parse_transcript(text: &str) -> Vec<Segment> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let (speaker, body) = match line
                .strip_prefix("[Speaker ")
                .and_then(|rest| rest.split_once(']'))
                .and_then(|(n, rest)| Some((n.trim().parse::<u16>().ok()?, rest)))
            {
                Some((n, rest)) => (SpeakerId(n.max(1)), rest.trim()),
                None => (SpeakerId(1), line),
            };
            (!body.is_empty()).then(|| Segment {
                range: None,
                speaker,
                channel: Channel::Mixed,
                text: body.to_owned(),
            })
        })
        .collect()
}

fn legacy_model() -> ModelRef {
    ModelRef::parse("legacy/unknown").expect("a constant model id is valid")
}

/// The job a legacy recording gets: its goal, the events that walk it to the old state, and the
/// priority it starts with. `None` when the old app had nothing in the pipeline for it.
pub(crate) struct JobPlan {
    pub goal: Goal,
    pub priority: Priority,
    pub events: Vec<JobEvent>,
}

pub(crate) fn plan_job(
    status: Option<&str>,
    has_transcript: bool,
    has_summary: bool,
    front: bool,
    error: Option<&str>,
) -> Option<JobPlan> {
    let (t, s) = (Stage::Transcribe, Stage::Summarize);
    let finish = |stage| [JobEvent::Started { stage }, JobEvent::StageDone { stage }];
    let normal = |goal, events| JobPlan {
        goal,
        priority: Priority::Normal,
        events,
    };
    let complete = || match (has_transcript, has_summary) {
        (true, true) => Some(normal(
            Goal::ALL,
            finish(t).into_iter().chain(finish(s)).collect(),
        )),
        (true, false) => Some(normal(Goal::TRANSCRIBE, finish(t).into())),
        (false, true) => Some(normal(Goal::SUMMARIZE, finish(s).into())),
        (false, false) => None,
    };
    match status? {
        "done" => complete(),
        _ if has_transcript && has_summary => complete(),
        "queued" | "transcribing" | "summarizing" => Some(JobPlan {
            goal: Goal::ALL,
            priority: if front {
                Priority::Front
            } else {
                Priority::Normal
            },
            events: if has_transcript {
                finish(t).into()
            } else {
                Vec::new()
            },
        }),
        "failed" => {
            let stage = if has_transcript { s } else { t };
            let mut events: Vec<JobEvent> = if has_transcript {
                finish(t).into()
            } else {
                Vec::new()
            };
            events.push(JobEvent::Started { stage });
            events.push(JobEvent::Failed {
                stage,
                error: JobError {
                    class: FailureClass::Permanent,
                    message: error
                        .unwrap_or("failed in the previous version of Dory")
                        .to_owned(),
                },
            });
            Some(normal(Goal::ALL, events))
        }
        _ => None,
    }
}

fn write_recording(
    tx: &rusqlite::Connection,
    id: &RecordingId,
    legacy: &Legacy,
    take: TakeInfo,
    existed: bool,
    old_job: Option<i64>,
    hash: &str,
) -> Result<(), StoreError> {
    let mut changes = Vec::new();
    if existed {
        if legacy.title.is_some() {
            changes.push(Change::RecordingEdited {
                id: id.clone(),
                edit: RecordingEdit::Title(legacy.title.clone()),
            });
        }
        changes.push(Change::RecordingEdited {
            id: id.clone(),
            edit: RecordingEdit::Take(take),
        });
    } else {
        changes.push(Change::RecordingCreated(Recording {
            id: id.clone(),
            title: legacy.title.clone(),
            created_at: legacy.created_at,
            language: LanguagePref::Auto,
            layout: ChannelLayout::Mixed,
            take: Some(take),
        }));
    }

    let current = load_transcript(tx, id)?;
    let mut revision = current.as_ref().map(|t| t.revision);
    if let Some(text) = &legacy.transcript {
        let segments = parse_transcript(text);
        let same = current.as_ref().is_some_and(|t| t.segments == segments);
        if !same {
            let next = current.as_ref().map_or(Revision(1), |t| t.revision.next());
            revision = Some(next);
            changes.push(Change::TranscriptReplaced {
                id: id.clone(),
                transcript: Transcript {
                    revision: next,
                    segments,
                    detected_languages: Vec::new(),
                    produced_by: legacy_model(),
                },
            });
        }
    }
    if let Some(markdown) = &legacy.summary {
        let summary = Summary {
            markdown: markdown.clone(),
            language: None,
            produced_by: legacy_model(),
            transcript_revision: revision.unwrap_or(Revision(0)),
        };
        if load_summary(tx, id)?.as_ref() != Some(&summary) {
            changes.push(Change::SummaryReplaced {
                id: id.clone(),
                summary,
            });
        }
    }
    for change in &changes {
        apply_change(tx, change)?;
    }

    // The job the previous import made is replaced, unless the new pipeline has taken it up.
    let mut job_id = old_job;
    let mut replace = true;
    if let Some(old) = old_job {
        match load_job(tx, JobId(u64::try_from(old).unwrap_or(0)))? {
            Some(job) if matches!(job.state, JobState::Running { .. }) => replace = false,
            Some(_) => {
                tx.execute("DELETE FROM jobs WHERE id = ?1", [old])
                    .map_err(db)?;
                job_id = None;
            }
            None => job_id = None,
        }
    }
    if replace {
        job_id = None;
        let plan = plan_job(
            legacy.status.as_deref(),
            legacy.transcript.is_some(),
            legacy.summary.is_some(),
            legacy.front,
            legacy.error.as_deref(),
        );
        if let Some(plan) = plan {
            let enqueued = apply_change(
                tx,
                &Change::JobEnqueued {
                    id: id.clone(),
                    goal: plan.goal,
                    priority: plan.priority,
                    at: legacy.created_at,
                },
            )?;
            let job =
                enqueued.ok_or_else(|| StoreError::Corrupt("enqueue returned no job".into()))?;
            for event in plan.events {
                apply_change(tx, &Change::Job { job, event })?;
            }
            job_id = Some(i64::try_from(job.0).unwrap_or(i64::MAX));
        }
    }
    tx.execute(
        "INSERT OR REPLACE INTO legacy_imports (recording_id, content_hash, job_id) VALUES (?1, ?2, ?3)",
        params![id.as_str(), hash, job_id],
    )
    .map_err(db)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dory_core::domain::{Job, RecordingId};

    fn lines(text: &str) -> Vec<(u16, String)> {
        parse_transcript(text)
            .into_iter()
            .map(|s| (s.speaker.0, s.text))
            .collect()
    }

    #[test]
    fn transcript_lines_parse_by_speaker_label() {
        let cases: &[(&str, Vec<(u16, &str)>)] = &[
            ("", vec![]),
            ("\n  \n", vec![]),
            ("[Speaker 1] hello", vec![(1, "hello")]),
            ("[Speaker 2] hi\n[Speaker 1] yo", vec![(2, "hi"), (1, "yo")]),
            ("[Speaker 12] far", vec![(12, "far")]),
            ("[Speaker 0] odd", vec![(1, "odd")]),
            ("no label", vec![(1, "no label")]),
            ("[Speaker 1]", vec![]),
            ("[Speaker 1]   ", vec![]),
            (
                "[Speaker x] not a label",
                vec![(1, "[Speaker x] not a label")],
            ),
            (
                "[Speaker 1 missing bracket",
                vec![(1, "[Speaker 1 missing bracket")],
            ),
            (
                "[Speaker 3] a\r\n[Speaker 3] b\r\n",
                vec![(3, "a"), (3, "b")],
            ),
            ("  [Speaker 2]   padded  ", vec![(2, "padded")]),
            (
                "[Speaker 1] has [brackets] inside",
                vec![(1, "has [brackets] inside")],
            ),
            (
                "[Speaker 70000] too big",
                vec![(1, "[Speaker 70000] too big")],
            ),
        ];
        for (input, want) in cases {
            let want: Vec<(u16, String)> =
                want.iter().map(|(n, t)| (*n, (*t).to_owned())).collect();
            assert_eq!(lines(input), want, "{input:?}");
        }
    }

    fn final_job(plan: &JobPlan) -> Job {
        let mut job = Job::new(
            JobId(1),
            RecordingId::parse("r").unwrap(),
            plan.goal,
            plan.priority,
            Timestamp(0),
        );
        for event in &plan.events {
            job = job.on(event).unwrap_or_else(|e| panic!("{event:?}: {e}"));
        }
        job
    }

    /// Status, has transcript, has summary, front flag, and the job it must become.
    type Case = (
        Option<&'static str>,
        bool,
        bool,
        bool,
        Option<(JobState, Vec<Stage>, Priority)>,
    );

    #[test]
    fn every_old_status_maps_to_a_legal_job() {
        let (t, s) = (Stage::Transcribe, Stage::Summarize);
        // (status, has transcript, has summary, front, expected state, done)
        let cases: Vec<Case> = vec![
            (None, false, false, false, None),
            (Some("idle"), true, false, false, None),
            (Some("weird"), false, false, true, None),
            (Some("done"), false, false, false, None),
            (
                Some("done"),
                true,
                true,
                false,
                Some((JobState::Succeeded, vec![t, s], Priority::Normal)),
            ),
            (
                Some("done"),
                true,
                false,
                false,
                Some((JobState::Succeeded, vec![t], Priority::Normal)),
            ),
            (
                Some("done"),
                false,
                true,
                false,
                Some((JobState::Succeeded, vec![s], Priority::Normal)),
            ),
            (
                Some("queued"),
                false,
                false,
                false,
                Some((JobState::Queued, vec![], Priority::Normal)),
            ),
            (
                Some("queued"),
                false,
                false,
                true,
                Some((JobState::Queued, vec![], Priority::Front)),
            ),
            (
                Some("queued"),
                true,
                false,
                true,
                Some((JobState::Queued, vec![t], Priority::Front)),
            ),
            (
                Some("transcribing"),
                false,
                false,
                false,
                Some((JobState::Queued, vec![], Priority::Normal)),
            ),
            (
                Some("summarizing"),
                true,
                false,
                false,
                Some((JobState::Queued, vec![t], Priority::Normal)),
            ),
            (
                Some("queued"),
                true,
                true,
                true,
                Some((JobState::Succeeded, vec![t, s], Priority::Normal)),
            ),
            (
                Some("failed"),
                true,
                true,
                false,
                Some((JobState::Succeeded, vec![t, s], Priority::Normal)),
            ),
        ];
        for (status, has_t, has_s, front, want) in cases {
            let plan = plan_job(status, has_t, has_s, front, Some("boom"));
            match (plan, want) {
                (None, None) => {}
                (Some(plan), Some((state, done, priority))) => {
                    let job = final_job(&plan);
                    assert_eq!(
                        (job.state, job.done, job.priority),
                        (state, done, priority),
                        "{status:?} t={has_t} s={has_s} front={front}"
                    );
                }
                (got, want) => panic!(
                    "{status:?}: plan {:?} but wanted {want:?}",
                    got.map(|p| p.events)
                ),
            }
        }
    }

    #[test]
    fn a_failed_status_fails_the_stage_after_the_last_one_that_produced_output() {
        let (t, s) = (Stage::Transcribe, Stage::Summarize);
        for (has_t, stage, done) in [(false, t, vec![]), (true, s, vec![t])] {
            let job = final_job(
                &plan_job(Some("failed"), has_t, false, true, Some("model gone")).unwrap(),
            );
            let JobState::Failed { stage: at, error } = &job.state else {
                panic!("{:?}", job.state)
            };
            assert_eq!((*at, &job.done), (stage, &done));
            assert_eq!(
                (error.class, error.message.as_str()),
                (FailureClass::Permanent, "model gone")
            );
            assert_eq!(
                job.priority,
                Priority::Normal,
                "a failed job does not jump the queue"
            );
        }
        let plain = final_job(&plan_job(Some("failed"), false, false, false, None).unwrap());
        let JobState::Failed { error, .. } = plain.state else {
            panic!()
        };
        assert!(!error.message.is_empty());
    }
}
