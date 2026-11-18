//! Apple Speech (SpeechAnalyzer with SpeechTranscriber) as a speech model: one row per language
//! macOS can transcribe, each holding that language's regional locales.

use std::collections::BTreeMap;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::sync::Arc;

use dory_core::domain::{Fraction, LangSet, LangTag, LanguagePref, Ms, TimeRange, languages};
use dory_core::models::{
    Availability, DiarizationSupport, LangSupport, ModelAbout, ModelEntry, ModelId, ProviderId,
    Stt, SttCaps, Unavailable, UnavailableReason,
};
use dory_core::ports::{
    CancelToken, EngineError, ModelProvider, ProvisionError, ProvisionSink, RawSegment,
    SegmentSink, TranscribeOutcome, TranscribeRequest, Transcriber,
};

use super::ffi::{self, SpeechEvent};

/// Below Parakeet (30), which stays the pick when both fit, and above Whisper turbo (10), which
/// is far slower on a one-language meeting.
const RANK: i16 = 20;
/// Ten-minute windows give long meetings checkpoints to resume from.
const WINDOW: Ms = Ms(600_000);

/// The locale each language installs and runs when the meeting names only the language. A
/// language missing here defaults to its first locale.
const DEFAULT_REGIONS: [(&str, &str); 10] = [
    ("de", "DE"),
    ("en", "US"),
    ("es", "ES"),
    ("fr", "FR"),
    ("it", "IT"),
    ("ja", "JP"),
    ("ko", "KR"),
    ("pt", "BR"),
    ("yue", "CN"),
    ("zh", "CN"),
];

/// `ModelProvider<Stt>` over Apple's SpeechTranscriber.
pub struct AppleSpeech {
    rows: Vec<Row>,
}

/// One language and the regional locales Apple lists for it. Assets are per locale, so a row is
/// ready when any of its locales is installed.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    language: LangTag,
    name: String,
    /// The locale to install, and to run when the meeting names only the language.
    default: LangTag,
    /// Sorted, and includes `default`.
    locales: Vec<LangTag>,
}

impl Row {
    /// The default first, so a probe usually stops after one call.
    fn probe_order(&self) -> impl Iterator<Item = &LangTag> {
        std::iter::once(&self.default).chain(self.locales.iter().filter(|l| **l != self.default))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AssetState {
    Installed,
    Supported,
    Downloading,
    Unsupported,
}

impl AppleSpeech {
    /// Asks Apple once which locales it supports, so `catalog` can stay free of IO. The registry
    /// reads the catalog once at startup, so reading here costs the same and happens at one known
    /// point. A language Apple adds or drops shows up on the next launch.
    pub fn load() -> AppleSpeech {
        AppleSpeech {
            rows: rows(&ffi::speech_locales()),
        }
    }

    fn row(&self, id: &ModelId<Stt>) -> Option<&Row> {
        (id.provider().as_str() == "apple")
            .then(|| self.rows.iter().find(|row| model_name(row) == id.name()))
            .flatten()
    }
}

fn provider_id() -> ProviderId {
    ProviderId::parse("apple").expect("a valid provider id")
}

fn model_name(row: &Row) -> String {
    format!("speech-{}", row.language)
}

fn display_name(row: &Row) -> String {
    format!("Apple Speech ({})", row.name)
}

fn rows(locales_csv: &str) -> Vec<Row> {
    let mut by_language: BTreeMap<String, Vec<LangTag>> = BTreeMap::new();
    for tag in locales_csv
        .split(',')
        .filter_map(|tag| LangTag::parse(tag.trim()).ok())
    {
        by_language
            .entry(tag.primary().to_owned())
            .or_default()
            .push(tag);
    }
    by_language
        .into_iter()
        .filter_map(|(code, mut locales)| {
            locales.sort();
            locales.dedup();
            let language = LangTag::parse(&code).ok()?;
            let default = DEFAULT_REGIONS
                .iter()
                .find(|(c, _)| *c == code)
                .and_then(|(_, region)| {
                    let wanted = format!("{code}-{region}");
                    locales.iter().find(|l| l.as_str() == wanted)
                })
                .or(locales.first())?
                .clone();
            let name = languages::name(&language);
            Some(Row {
                language,
                name,
                default,
                locales,
            })
        })
        .collect()
}

fn entry(row: &Row) -> ModelEntry<Stt> {
    ModelEntry {
        id: ModelId::new(provider_id(), &model_name(row)).expect("language tags are valid names"),
        display_name: display_name(row),
        about: ModelAbout {
            summary: format!("Apple's on-device speech recognizer for {}.", row.name),
            pros: vec!["No download for English, and fast".into()],
            cons: vec!["One language per meeting, and no Tagalog".into()],
        },
        caps: SttCaps {
            languages: LangSupport::FixedLocale {
                tags: LangSet::Only(row.locales.clone()),
            },
            diarization: DiarizationSupport::None,
            max_input: None,
            window: Some(WINDOW),
            sentence_times: false,
        },
        recommended: false,
        rank: RANK,
        disk_bytes: None,
        built_in: false,
    }
}

fn asset_state(code: i32) -> AssetState {
    match code {
        ffi::ASSET_INSTALLED => AssetState::Installed,
        ffi::ASSET_SUPPORTED => AssetState::Supported,
        ffi::ASSET_DOWNLOADING => AssetState::Downloading,
        _ => AssetState::Unsupported,
    }
}

fn probe(locale: &LangTag) -> AssetState {
    let locale = CString::new(locale.as_str()).expect("a language tag has no NUL");
    asset_state(ffi::speech_asset_state(&locale))
}

/// Stops at the first installed locale, so pass the states lazily.
fn availability(row: &Row, states: impl IntoIterator<Item = AssetState>) -> Availability {
    let mut provisionable = false;
    for state in states {
        match state {
            AssetState::Installed => return Availability::Ready,
            AssetState::Supported | AssetState::Downloading => provisionable = true,
            AssetState::Unsupported => {}
        }
    }
    if provisionable {
        Availability::NeedsProvision { bytes: None }
    } else {
        Availability::Unavailable(Unavailable {
            reason: UnavailableReason::NotEligible {
                detail: format!("macOS no longer offers Apple Speech in {}", row.name),
            },
            fix: None,
        })
    }
}

/// The installed locale that runs a meeting in `wanted`: the exact locale when it is installed,
/// else the row's default, else any installed locale of the language.
fn choose_locale(
    row: &Row,
    wanted: &LangTag,
    installed: &[LangTag],
) -> Result<LangTag, EngineError> {
    if wanted.primary() != row.language.as_str() {
        return Err(EngineError::UnsupportedLanguage(wanted.clone()));
    }
    installed
        .iter()
        .find(|l| *l == wanted)
        .or_else(|| installed.iter().find(|l| **l == row.default))
        .or_else(|| installed.first())
        .cloned()
        .ok_or_else(|| EngineError::ModelMissing(format!("{} is not installed", display_name(row))))
}

/// One Apple result as a segment inside the clip. Apple's times are relative to the clip's start,
/// and are clamped to its length.
fn segment(
    start_ms: u64,
    end_ms: u64,
    text: &str,
    clip_len: Ms,
    language: &LangTag,
) -> Option<RawSegment> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return None;
    }
    let start = start_ms.min(clip_len.0);
    let end = end_ms.clamp(start, clip_len.0);
    Some(RawSegment {
        range: TimeRange::new(Ms(start), Ms(end))?,
        text,
        speaker: None,
        language: Some(language.clone()),
    })
}

fn engine_error(code: i32, message: String, wanted: &LangTag) -> Option<EngineError> {
    Some(match code {
        ffi::OK => return None,
        ffi::CANCELLED => EngineError::Cancelled,
        ffi::SPEECH_ASSETS_MISSING => EngineError::ModelMissing(message),
        ffi::SPEECH_UNSUPPORTED_LOCALE => EngineError::UnsupportedLanguage(wanted.clone()),
        ffi::SPEECH_AUDIO_UNREADABLE => EngineError::InputRejected(message),
        ffi::SPEECH_BUSY => EngineError::ServerUnavailable(message),
        _ => EngineError::Other {
            message,
            source: None,
        },
    })
}

fn provision_error(code: i32, message: String) -> Option<ProvisionError> {
    Some(match code {
        ffi::OK => return None,
        ffi::CANCELLED => ProvisionError::Cancelled,
        ffi::SPEECH_RESERVATION_LIMIT => {
            let limit = message.trim().parse::<u32>().unwrap_or(5);
            ProvisionError::NotProvisionable(format!(
                "Dory can keep {limit} Apple Speech languages. Remove one to add another."
            ))
        }
        ffi::SPEECH_UNSUPPORTED_LOCALE => ProvisionError::NotProvisionable(message),
        ffi::SPEECH_NETWORK => ProvisionError::Network(message),
        _ => ProvisionError::Other(message),
    })
}

/// Apple reports a fraction, not bytes. The sink shows `done / total` as a percent, so report
/// thousandths.
fn thousandths(fraction: f64) -> u64 {
    (fraction.clamp(0.0, 1.0) * 1000.0).round() as u64
}

fn cancel_with(cancel: &CancelToken) -> Arc<ffi::Task> {
    let task = Arc::new(ffi::Task::new());
    let hook = Arc::clone(&task);
    cancel.on_cancel(move || hook.cancel());
    task
}

impl ModelProvider<Stt> for AppleSpeech {
    fn id(&self) -> ProviderId {
        provider_id()
    }

    fn catalog(&self) -> Vec<ModelEntry<Stt>> {
        self.rows.iter().map(entry).collect()
    }

    fn availability(&self, id: &ModelId<Stt>) -> Availability {
        match self.row(id) {
            Some(row) => availability(row, row.probe_order().map(probe)),
            None => Availability::Unavailable(Unavailable {
                reason: UnavailableReason::NotEligible {
                    detail: format!("{id} is not an Apple Speech model"),
                },
                fix: None,
            }),
        }
    }

    fn provision(
        &self,
        id: &ModelId<Stt>,
        progress: &mut dyn ProvisionSink,
        cancel: &CancelToken,
    ) -> Result<(), ProvisionError> {
        let row = self.row(id).ok_or_else(|| {
            ProvisionError::NotProvisionable(format!("{id} is not in the catalog"))
        })?;
        if cancel.is_cancelled() {
            return Err(ProvisionError::Cancelled);
        }
        progress.step("Downloading");
        let locale = CString::new(row.default.as_str()).expect("a language tag has no NUL");
        let task = cancel_with(cancel);
        let raw = ffi::speech_install(&task, &locale, &mut |event| {
            if let SpeechEvent::Progress(fraction) = event {
                progress.bytes(thousandths(fraction), Some(1000));
            }
            true
        });
        provision_error(raw.code, raw.error).map_or(Ok(()), Err)
    }

    fn open(
        &self,
        id: &ModelId<Stt>,
        cancel: &CancelToken,
    ) -> Result<Arc<dyn Transcriber>, EngineError> {
        let row = self
            .row(id)
            .ok_or_else(|| EngineError::ModelMissing(format!("{id} is not in the catalog")))?;
        let installed: Vec<LangTag> = row
            .locales
            .iter()
            .filter(|l| probe(l) == AssetState::Installed)
            .cloned()
            .collect();
        if installed.is_empty() {
            return Err(EngineError::ModelMissing(format!(
                "{} is not installed",
                display_name(row)
            )));
        }
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        Ok(Arc::new(AppleSpeechEngine {
            row: row.clone(),
            installed,
        }))
    }
}

/// One language, with the locales that were installed when it opened.
struct AppleSpeechEngine {
    row: Row,
    installed: Vec<LangTag>,
}

impl Transcriber for AppleSpeechEngine {
    fn transcribe(
        &self,
        req: &TranscribeRequest<'_>,
        sink: &mut dyn SegmentSink,
        cancel: &CancelToken,
    ) -> Result<TranscribeOutcome, EngineError> {
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        let wanted = match &req.language {
            LanguagePref::Pinned(tag) => tag.clone(),
            LanguagePref::Auto => self.row.language.clone(),
        };
        let locale = choose_locale(&self.row, &wanted, &self.installed)?;
        let locale = CString::new(locale.as_str()).expect("a language tag has no NUL");
        let path = CString::new(req.clip.path.as_os_str().as_bytes()).map_err(|_| {
            EngineError::InputRejected(format!("{} has a NUL byte", req.clip.path.display()))
        })?;
        let clip_len = req.clip.duration;
        let task = cancel_with(cancel);

        sink.progress(Fraction::new(0.0));
        let raw = ffi::speech_transcribe(&task, &path, (0, clip_len.0), &locale, &mut |event| {
            match event {
                SpeechEvent::Segment {
                    start_ms,
                    end_ms,
                    text,
                } => {
                    if let Some(segment) = segment(start_ms, end_ms, text, clip_len, &wanted) {
                        sink.segment(segment);
                    }
                }
                SpeechEvent::Progress(fraction) => sink.progress(Fraction::new(fraction as f32)),
            }
            true
        });
        if let Some(error) = engine_error(raw.code, raw.error, &wanted) {
            return Err(error);
        }
        sink.progress(Fraction::new(1.0));
        Ok(TranscribeOutcome::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC_26_5: &str = "de-AT,de-CH,de-DE,en-AU,en-CA,en-GB,en-IE,en-IN,en-NZ,en-SG,en-US,\
        en-ZA,es-CL,es-ES,es-MX,es-US,fr-BE,fr-CA,fr-CH,fr-FR,it-CH,it-IT,ja-JP,ko-KR,pt-BR,pt-PT,\
        yue-CN,zh-CN,zh-HK,zh-TW";

    fn tag(s: &str) -> LangTag {
        LangTag::parse(s).unwrap()
    }

    fn tags(list: &[&str]) -> Vec<LangTag> {
        list.iter().map(|s| tag(s)).collect()
    }

    fn english() -> Row {
        rows(MAC_26_5)
            .into_iter()
            .find(|r| r.language.as_str() == "en")
            .unwrap()
    }

    #[test]
    fn rows_group_locales_by_language() {
        let rows = rows(MAC_26_5);
        let summary: Vec<_> = rows
            .iter()
            .map(|r| (r.language.as_str(), r.locales.len(), r.default.as_str()))
            .collect();
        assert_eq!(
            summary,
            [
                ("de", 3, "de-DE"),
                ("en", 9, "en-US"),
                ("es", 4, "es-ES"),
                ("fr", 4, "fr-FR"),
                ("it", 2, "it-IT"),
                ("ja", 1, "ja-JP"),
                ("ko", 1, "ko-KR"),
                ("pt", 2, "pt-BR"),
                ("yue", 1, "yue-CN"),
                ("zh", 3, "zh-CN"),
            ]
        );
        let entries: Vec<_> = rows.iter().map(entry).collect();
        assert_eq!(entries[1].id.to_string(), "apple/speech-en");
        assert_eq!(entries[1].display_name, "Apple Speech (English)");
        assert_eq!(
            entries[1].caps.languages,
            LangSupport::FixedLocale {
                tags: LangSet::Only(english().locales)
            }
        );
        assert_eq!(entries[1].caps.window, Some(Ms(600_000)));
        assert!(!entries[1].recommended);
        assert!(entries.iter().all(|e| e.rank < 30), "below Parakeet");
    }

    #[test]
    fn a_language_without_a_known_name_still_gets_a_row() {
        let rows = rows("nl-NL,nl-BE,ain-JP, bogus!,,");
        let named: Vec<_> = rows
            .iter()
            .map(|r| (display_name(r), r.default.as_str()))
            .collect();
        assert_eq!(
            named,
            [
                ("Apple Speech (ain)".to_owned(), "ain-JP"),
                ("Apple Speech (Dutch)".to_owned(), "nl-BE"),
            ]
        );
        assert!(super::rows("").is_empty());
    }

    #[test]
    fn asset_states_map_to_availability() {
        use AssetState::*;
        let row = english();
        assert_eq!(
            availability(&row, [Supported, Installed]),
            Availability::Ready
        );
        assert_eq!(
            availability(&row, [Supported, Downloading, Unsupported]),
            Availability::NeedsProvision { bytes: None }
        );
        assert_eq!(
            availability(&row, [Downloading]),
            Availability::NeedsProvision { bytes: None }
        );
        assert!(matches!(
            availability(&row, [Unsupported, Unsupported]),
            Availability::Unavailable(_)
        ));
        assert_eq!(asset_state(ffi::ASSET_INSTALLED), Installed);
        assert_eq!(asset_state(ffi::ASSET_SUPPORTED), Supported);
        assert_eq!(asset_state(ffi::ASSET_DOWNLOADING), Downloading);
        assert_eq!(asset_state(3), Unsupported);
        assert_eq!(asset_state(-1), Unsupported);
    }

    #[test]
    fn the_probe_stops_at_the_first_installed_locale() {
        let row = english();
        let mut probed = 0;
        let ready = availability(
            &row,
            row.probe_order().map(|_| {
                probed += 1;
                AssetState::Installed
            }),
        );
        assert_eq!((ready, probed), (Availability::Ready, 1));
        assert_eq!(row.probe_order().next(), Some(&tag("en-US")));
        assert_eq!(row.probe_order().count(), row.locales.len());
    }

    #[test]
    fn the_locale_follows_the_pinned_language() {
        let row = english();
        let all = row.locales.clone();
        let cases = [
            ("en-GB", &all[..], "en-GB"),
            ("en", &all[..], "en-US"),
            ("en-PH", &all[..], "en-US"),
            ("en-GB", &tags(&["en-US", "en-AU"])[..], "en-US"),
            ("en", &tags(&["en-AU", "en-IN"])[..], "en-AU"),
        ];
        for (wanted, installed, want) in cases {
            let got = choose_locale(&row, &tag(wanted), installed).unwrap();
            assert_eq!(got.as_str(), want, "{wanted}");
        }
        assert!(matches!(
            choose_locale(&row, &tag("de"), &all),
            Err(EngineError::UnsupportedLanguage(t)) if t == tag("de")
        ));
        assert!(matches!(
            choose_locale(&row, &tag("en"), &[]),
            Err(EngineError::ModelMissing(_))
        ));
    }

    #[test]
    fn segments_are_clip_relative() {
        let en = tag("en");
        let clip = Ms(42_600);
        let seg = segment(1_200, 4_800, " Let's start\nthe  weekly sync. ", clip, &en).unwrap();
        assert_eq!(seg.range, TimeRange::new(Ms(1_200), Ms(4_800)).unwrap());
        assert_eq!(seg.text, "Let's start the weekly sync.");
        assert_eq!(seg.language, Some(en.clone()));
        assert_eq!(seg.speaker, None);

        let past_end = segment(42_000, 43_100, "Bye.", clip, &en).unwrap();
        assert_eq!(past_end.range, TimeRange::new(Ms(42_000), clip).unwrap());
        let backwards = segment(5_000, 4_000, "Hm.", clip, &en).unwrap();
        assert_eq!(
            backwards.range,
            TimeRange::new(Ms(5_000), Ms(5_000)).unwrap()
        );
        assert_eq!(segment(0, 1_000, " \n ", clip, &en), None);
    }

    #[test]
    fn swift_codes_map_to_engine_and_provision_errors() {
        let en = tag("en");
        let engine = |code| engine_error(code, "m".into(), &en);
        assert!(engine(ffi::OK).is_none());
        assert!(matches!(
            engine(ffi::CANCELLED),
            Some(EngineError::Cancelled)
        ));
        assert!(matches!(
            engine(ffi::SPEECH_ASSETS_MISSING),
            Some(EngineError::ModelMissing(_))
        ));
        assert!(matches!(
            engine(ffi::SPEECH_UNSUPPORTED_LOCALE),
            Some(EngineError::UnsupportedLanguage(t)) if t == en
        ));
        assert!(matches!(
            engine(ffi::SPEECH_AUDIO_UNREADABLE),
            Some(EngineError::InputRejected(_))
        ));
        assert!(matches!(
            engine(ffi::SPEECH_BUSY),
            Some(EngineError::ServerUnavailable(_))
        ));
        assert!(matches!(engine(20), Some(EngineError::Other { .. })));

        assert!(provision_error(ffi::OK, String::new()).is_none());
        assert!(matches!(
            provision_error(ffi::CANCELLED, String::new()),
            Some(ProvisionError::Cancelled)
        ));
        let limit = provision_error(ffi::SPEECH_RESERVATION_LIMIT, "5".into()).unwrap();
        assert_eq!(
            limit.to_string(),
            "cannot be provisioned here: Dory can keep 5 Apple Speech languages. Remove one to \
             add another."
        );
        assert!(matches!(
            provision_error(ffi::SPEECH_NETWORK, "offline".into()),
            Some(ProvisionError::Network(_))
        ));
        assert!(matches!(
            provision_error(20, "x".into()),
            Some(ProvisionError::Other(_))
        ));
    }

    #[test]
    fn install_progress_reads_as_a_percent() {
        assert_eq!(thousandths(0.0), 0);
        assert_eq!(thousandths(0.4567), 457);
        assert_eq!(thousandths(1.5), 1000);
        assert_eq!(thousandths(f64::NAN), 0);
    }

    #[test]
    fn ids_outside_the_catalog_are_refused() {
        let speech = AppleSpeech {
            rows: rows(MAC_26_5),
        };
        let other: ModelId<Stt> = "sherpa/speech-en".parse().unwrap();
        let unknown: ModelId<Stt> = "apple/speech-tl".parse().unwrap();
        assert!(speech.row(&other).is_none());
        assert!(speech.row(&unknown).is_none());
        assert!(matches!(
            speech.open(&unknown, &CancelToken::new()),
            Err(EngineError::ModelMissing(_))
        ));
        assert_eq!(
            speech.row(&"apple/speech-en".parse().unwrap()),
            Some(&english())
        );
    }

    #[test]
    #[ignore = "calls Apple Speech; run with DORY_APPLE_LIVE=1 and --ignored"]
    fn a_range_reads_only_its_part() {
        if std::env::var("DORY_APPLE_LIVE").as_deref() != Ok("1") {
            return;
        }
        let path = CString::new("/tmp/dory-ai-p3-live/takes/2026-11-16T041159-0000.flac").unwrap();
        let mut seen = Vec::new();
        let raw = ffi::speech_transcribe(
            &ffi::Task::new(),
            &path,
            (20_000, 30_000),
            c"en-US",
            &mut |event| {
                if let SpeechEvent::Segment {
                    start_ms,
                    end_ms,
                    text,
                } = event
                {
                    seen.push((start_ms, end_ms, text.to_owned()));
                }
                true
            },
        );
        assert_eq!(raw.code, ffi::OK, "{}", raw.error);
        eprintln!("{seen:?}");
        let text: String = seen.iter().map(|(_, _, t)| t.as_str()).collect();
        assert!(text.contains("budget"), "{text}");
        assert!(!text.contains("weekly sync"), "{text}");
        assert!(seen.iter().all(|(_, end, _)| *end <= 10_000), "{seen:?}");
    }
}
