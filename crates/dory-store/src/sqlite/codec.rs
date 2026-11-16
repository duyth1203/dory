//! Private DTO layer: how domain values become columns and back. The domain types carry no serde,
//! so a domain change cannot silently change what is on disk. Every `match` over a domain enum
//! has no wildcard arm, so adding a variant stops compiling here until the format says what to do.

use dory_core::domain::{
    Blocker, Channel, ChannelLayout, FailureCause, FailureClass, Goal, JobError, LangTag,
    LanguagePref, Ms, Stage, TakeContainer, TimeRange,
};
use dory_core::models::{
    Choice, KindTag, Mismatch, ModelId, ModelKind, ModelRef, Unavailable, UnavailableReason,
};
use dory_core::ports::{RawSegment, StoreError};
use serde_json::{Value, json};

pub(crate) fn corrupt(detail: impl Into<String>) -> StoreError {
    StoreError::Corrupt(detail.into())
}

pub(crate) fn layout_to_db(layout: ChannelLayout) -> &'static str {
    match layout {
        ChannelLayout::MicSystem => "mic_system",
        ChannelLayout::MicOnly => "mic_only",
        ChannelLayout::SystemOnly => "system_only",
        ChannelLayout::Mixed => "mixed",
    }
}

pub(crate) fn layout_from_db(s: &str) -> Result<ChannelLayout, StoreError> {
    Ok(match s {
        "mic_system" => ChannelLayout::MicSystem,
        "mic_only" => ChannelLayout::MicOnly,
        "system_only" => ChannelLayout::SystemOnly,
        "mixed" => ChannelLayout::Mixed,
        other => return Err(corrupt(format!("unknown channel layout {other:?}"))),
    })
}

pub(crate) fn container_to_db(container: TakeContainer) -> &'static str {
    match container {
        TakeContainer::Wav => "wav",
        TakeContainer::Flac => "flac",
        TakeContainer::LegacyWebm => "legacy_webm",
    }
}

pub(crate) fn container_from_db(s: &str) -> Result<TakeContainer, StoreError> {
    Ok(match s {
        "wav" => TakeContainer::Wav,
        "flac" => TakeContainer::Flac,
        "legacy_webm" => TakeContainer::LegacyWebm,
        other => return Err(corrupt(format!("unknown take container {other:?}"))),
    })
}

pub(crate) fn channel_to_db(channel: Channel) -> &'static str {
    match channel {
        Channel::Mic => "mic",
        Channel::System => "system",
        Channel::Mixed => "mixed",
    }
}

pub(crate) fn channel_from_db(s: &str) -> Result<Channel, StoreError> {
    Ok(match s {
        "mic" => Channel::Mic,
        "system" => Channel::System,
        "mixed" => Channel::Mixed,
        other => return Err(corrupt(format!("unknown channel {other:?}"))),
    })
}

pub(crate) fn stage_to_db(stage: Stage) -> &'static str {
    match stage {
        Stage::Transcribe => "transcribe",
        Stage::Summarize => "summarize",
    }
}

pub(crate) fn stage_from_db(s: &str) -> Result<Stage, StoreError> {
    Ok(match s {
        "transcribe" => Stage::Transcribe,
        "summarize" => Stage::Summarize,
        other => return Err(corrupt(format!("unknown stage {other:?}"))),
    })
}

pub(crate) fn done_to_db(done: &[Stage]) -> String {
    done.iter()
        .map(|s| stage_to_db(*s))
        .collect::<Vec<_>>()
        .join(",")
}

pub(crate) fn done_from_db(s: &str) -> Result<Vec<Stage>, StoreError> {
    s.split(',')
        .filter(|part| !part.is_empty())
        .map(stage_from_db)
        .collect()
}

pub(crate) fn goal_to_db(goal: Goal) -> i64 {
    i64::from(goal.wants(Stage::Transcribe)) | i64::from(goal.wants(Stage::Summarize)) << 1
}

pub(crate) fn goal_from_db(bits: i64) -> Result<Goal, StoreError> {
    Goal::new(bits & 1 != 0, bits & 2 != 0).ok_or_else(|| corrupt(format!("empty job goal {bits}")))
}

pub(crate) fn class_to_db(class: FailureClass) -> &'static str {
    match class {
        FailureClass::Cancelled => "cancelled",
        FailureClass::Transient => "transient",
        FailureClass::NeedsUser => "needs_user",
        FailureClass::Permanent => "permanent",
    }
}

pub(crate) fn class_from_db(s: &str) -> Result<FailureClass, StoreError> {
    Ok(match s {
        "cancelled" => FailureClass::Cancelled,
        "transient" => FailureClass::Transient,
        "needs_user" => FailureClass::NeedsUser,
        "permanent" => FailureClass::Permanent,
        other => return Err(corrupt(format!("unknown failure class {other:?}"))),
    })
}

pub(crate) fn cause_to_db(cause: FailureCause) -> &'static str {
    match cause {
        FailureCause::DiskFull => "disk_full",
        FailureCause::AudioUnreadable => "audio_unreadable",
        FailureCause::Crashed => "crashed",
        FailureCause::Other => "other",
    }
}

/// Rows that failed before schema v4 have no cause, which reads as `Other`.
fn cause_from_db(s: Option<&str>) -> Result<FailureCause, StoreError> {
    Ok(match s {
        None | Some("other") => FailureCause::Other,
        Some("disk_full") => FailureCause::DiskFull,
        Some("audio_unreadable") => FailureCause::AudioUnreadable,
        Some("crashed") => FailureCause::Crashed,
        Some(other) => return Err(corrupt(format!("unknown failure cause {other:?}"))),
    })
}

pub(crate) fn error_from_db(
    class: Option<String>,
    message: Option<String>,
    cause: Option<&str>,
) -> Result<JobError, StoreError> {
    let class = class.ok_or_else(|| corrupt("failed job without an error class"))?;
    Ok(JobError {
        class: class_from_db(&class)?,
        message: message.unwrap_or_default(),
        cause: cause_from_db(cause)?,
    })
}

pub(crate) fn language_to_db(pref: &LanguagePref) -> Option<String> {
    match pref {
        LanguagePref::Auto => None,
        LanguagePref::Pinned(tag) => Some(tag.to_string()),
    }
}

pub(crate) fn language_from_db(s: Option<&str>) -> Result<LanguagePref, StoreError> {
    match s {
        None => Ok(LanguagePref::Auto),
        Some(tag) => Ok(LanguagePref::Pinned(parse_tag(tag)?)),
    }
}

pub(crate) fn parse_tag(s: &str) -> Result<LangTag, StoreError> {
    LangTag::parse(s).map_err(|e| corrupt(e.to_string()))
}

pub(crate) fn parse_model_ref(s: &str) -> Result<ModelRef, StoreError> {
    ModelRef::parse(s).map_err(|e| corrupt(e.to_string()))
}

pub(crate) fn tags_to_db(tags: &[LangTag]) -> String {
    tags.iter()
        .map(LangTag::as_str)
        .collect::<Vec<_>>()
        .join(",")
}

pub(crate) fn tags_from_db(s: &str) -> Result<Vec<LangTag>, StoreError> {
    s.split(',')
        .filter(|part| !part.is_empty())
        .map(parse_tag)
        .collect()
}

pub(crate) fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

pub(crate) fn ms_from_db(value: i64) -> Result<Ms, StoreError> {
    u64::try_from(value)
        .map(Ms)
        .map_err(|_| corrupt(format!("negative media time {value}")))
}

pub(crate) fn range_from_db(
    start: Option<i64>,
    end: Option<i64>,
) -> Result<Option<TimeRange>, StoreError> {
    match (start, end) {
        (None, None) => Ok(None),
        (Some(start), Some(end)) => TimeRange::new(ms_from_db(start)?, ms_from_db(end)?)
            .map(Some)
            .ok_or_else(|| {
                corrupt(format!(
                    "segment range {start}..{end} ends before it starts"
                ))
            }),
        _ => Err(corrupt("segment has only one end of its range")),
    }
}

pub(crate) fn choice_to_db<K: ModelKind>(choice: &Choice<K>) -> String {
    match choice {
        Choice::Auto => "auto".to_owned(),
        Choice::Off => "off".to_owned(),
        Choice::Pinned(id) => format!("pinned:{id}"),
    }
}

pub(crate) fn choice_from_db<K: ModelKind>(s: &str) -> Result<Choice<K>, StoreError> {
    match s {
        "auto" => Ok(Choice::Auto),
        "off" => Ok(Choice::Off),
        pinned => {
            let id = pinned
                .strip_prefix("pinned:")
                .ok_or_else(|| corrupt(format!("unknown model choice {pinned:?}")))?;
            id.parse::<ModelId<K>>()
                .map(Choice::Pinned)
                .map_err(|e| corrupt(e.to_string()))
        }
    }
}

pub(crate) fn pref_to_setting(pref: &LanguagePref) -> String {
    match pref {
        LanguagePref::Auto => "auto".to_owned(),
        LanguagePref::Pinned(tag) => format!("pinned:{tag}"),
    }
}

pub(crate) fn pref_from_setting(s: &str) -> Result<LanguagePref, StoreError> {
    match s {
        "auto" => Ok(LanguagePref::Auto),
        other => match other.strip_prefix("pinned:") {
            Some(tag) => Ok(LanguagePref::Pinned(parse_tag(tag)?)),
            None => Err(corrupt(format!("unknown language setting {other:?}"))),
        },
    }
}

fn kind_to_db(kind: KindTag) -> &'static str {
    match kind {
        KindTag::Stt => "stt",
        KindTag::Llm => "llm",
        KindTag::Diar => "diar",
    }
}

fn kind_from_db(s: &str) -> Result<KindTag, StoreError> {
    Ok(match s {
        "stt" => KindTag::Stt,
        "llm" => KindTag::Llm,
        "diar" => KindTag::Diar,
        other => return Err(corrupt(format!("unknown model kind {other:?}"))),
    })
}

fn mismatch_to_db(why: Mismatch) -> &'static str {
    match why {
        Mismatch::AutoLanguageUnsupported => "auto_language_unsupported",
        Mismatch::LanguageUnsupported => "language_unsupported",
        Mismatch::NoDiarization => "no_diarization",
        Mismatch::InputTooLong => "input_too_long",
    }
}

fn mismatch_from_db(s: &str) -> Result<Mismatch, StoreError> {
    Ok(match s {
        "auto_language_unsupported" => Mismatch::AutoLanguageUnsupported,
        "language_unsupported" => Mismatch::LanguageUnsupported,
        "no_diarization" => Mismatch::NoDiarization,
        "input_too_long" => Mismatch::InputTooLong,
        other => return Err(corrupt(format!("unknown mismatch {other:?}"))),
    })
}

fn reason_to_json(reason: &UnavailableReason) -> Value {
    match reason {
        UnavailableReason::UnsupportedPlatform => json!({ "k": "unsupported_platform" }),
        UnavailableReason::OsTooOld { required } => {
            json!({ "k": "os_too_old", "required": required })
        }
        UnavailableReason::NotEligible { detail } => {
            json!({ "k": "not_eligible", "detail": detail })
        }
        UnavailableReason::ServerUnreachable { endpoint } => {
            json!({ "k": "server_unreachable", "endpoint": endpoint })
        }
    }
}

fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str, StoreError> {
    v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| corrupt(format!("blocker is missing {key:?}")))
}

fn reason_from_json(v: &Value) -> Result<UnavailableReason, StoreError> {
    Ok(match text(v, "k")? {
        "unsupported_platform" => UnavailableReason::UnsupportedPlatform,
        "os_too_old" => UnavailableReason::OsTooOld {
            required: text(v, "required")?.to_owned(),
        },
        "not_eligible" => UnavailableReason::NotEligible {
            detail: text(v, "detail")?.to_owned(),
        },
        "server_unreachable" => UnavailableReason::ServerUnreachable {
            endpoint: text(v, "endpoint")?.to_owned(),
        },
        other => return Err(corrupt(format!("unknown unavailable reason {other:?}"))),
    })
}

/// A blocker is a small closed tree, so it is stored as one JSON text column.
pub(crate) fn blocker_to_db(blocker: &Blocker) -> String {
    let value = match blocker {
        Blocker::NeedsProvision { model, bytes } => {
            json!({ "k": "needs_provision", "model": model.as_str(), "bytes": bytes })
        }
        Blocker::Unavailable { model, why } => json!({
            "k": "unavailable",
            "model": model.as_str(),
            "reason": reason_to_json(&why.reason),
            "fix": why.fix,
        }),
        Blocker::Incompatible { model, why } => {
            json!({ "k": "incompatible", "model": model.as_str(), "why": mismatch_to_db(*why) })
        }
        Blocker::NoModel { kind } => json!({ "k": "no_model", "kind": kind_to_db(*kind) }),
    };
    value.to_string()
}

pub(crate) fn blocker_from_db(s: &str) -> Result<Blocker, StoreError> {
    let v: Value = serde_json::from_str(s).map_err(|e| corrupt(format!("blocker: {e}")))?;
    Ok(match text(&v, "k")? {
        "needs_provision" => Blocker::NeedsProvision {
            model: parse_model_ref(text(&v, "model")?)?,
            bytes: match v.get("bytes") {
                None | Some(Value::Null) => None,
                Some(n) => Some(n.as_u64().ok_or_else(|| corrupt("blocker bytes"))?),
            },
        },
        "unavailable" => Blocker::Unavailable {
            model: parse_model_ref(text(&v, "model")?)?,
            why: Unavailable {
                reason: reason_from_json(
                    v.get("reason").ok_or_else(|| corrupt("blocker reason"))?,
                )?,
                fix: match v.get("fix") {
                    None | Some(Value::Null) => None,
                    Some(f) => Some(f.as_str().ok_or_else(|| corrupt("blocker fix"))?.to_owned()),
                },
            },
        },
        "incompatible" => Blocker::Incompatible {
            model: parse_model_ref(text(&v, "model")?)?,
            why: mismatch_from_db(text(&v, "why")?)?,
        },
        "no_model" => Blocker::NoModel {
            kind: kind_from_db(text(&v, "kind")?)?,
        },
        other => return Err(corrupt(format!("unknown blocker {other:?}"))),
    })
}

/// A checkpoint's segments are only ever read back whole, so they are one JSON text column.
pub(crate) fn raw_segments_to_db(segments: &[RawSegment]) -> String {
    Value::Array(
        segments
            .iter()
            .map(|s| {
                json!({
                    "start": s.range.start().0,
                    "end": s.range.end().0,
                    "text": s.text,
                    "speaker": s.speaker,
                    "language": s.language.as_ref().map(LangTag::as_str),
                })
            })
            .collect(),
    )
    .to_string()
}

pub(crate) fn raw_segments_from_db(s: &str) -> Result<Vec<RawSegment>, StoreError> {
    let v: Value = serde_json::from_str(s).map_err(|e| corrupt(format!("checkpoint: {e}")))?;
    let items = v
        .as_array()
        .ok_or_else(|| corrupt("checkpoint segments are not a list"))?;
    items
        .iter()
        .map(|item| {
            let ms = |key: &str| {
                item.get(key)
                    .and_then(Value::as_u64)
                    .map(Ms)
                    .ok_or_else(|| corrupt(format!("checkpoint segment is missing {key:?}")))
            };
            Ok(RawSegment {
                range: TimeRange::new(ms("start")?, ms("end")?)
                    .ok_or_else(|| corrupt("checkpoint segment ends before it starts"))?,
                text: item
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| corrupt("checkpoint segment is missing its text"))?
                    .to_owned(),
                speaker: match item.get("speaker") {
                    None | Some(Value::Null) => None,
                    Some(n) => Some(
                        n.as_u64()
                            .and_then(|n| u16::try_from(n).ok())
                            .ok_or_else(|| corrupt("checkpoint speaker out of range"))?,
                    ),
                },
                language: match item.get("language") {
                    None | Some(Value::Null) => None,
                    Some(tag) => {
                        Some(parse_tag(tag.as_str().ok_or_else(|| {
                            corrupt("checkpoint language is not text")
                        })?)?)
                    }
                },
            })
        })
        .collect()
}
