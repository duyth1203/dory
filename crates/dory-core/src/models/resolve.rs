use std::cmp::Reverse;

use super::{Availability, Fit, KindTag, ModelEntry, ModelId, ModelKind};
use crate::domain::{Blocker, InvalidInput};

/// The user's setting for one kind of model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice<K: ModelKind> {
    /// Pick the best model that fits. Never downloads anything.
    Auto,
    Pinned(ModelId<K>),
    /// Run nothing. Invalid for kinds that are not optional.
    Off,
}

// Deriving would demand `K: Default`, and the kinds are uninhabited.
#[allow(clippy::derivable_impls)]
impl<K: ModelKind> Default for Choice<K> {
    fn default() -> Self {
        Choice::Auto
    }
}

impl<K: ModelKind> Choice<K> {
    pub fn off() -> Result<Self, InvalidInput> {
        let choice = Choice::Off;
        choice.validate().map(|()| choice)
    }

    pub fn validate(&self) -> Result<(), InvalidInput> {
        match self {
            Choice::Off if !K::OPTIONAL => Err(InvalidInput::new(
                "model choice",
                format!("{} cannot be turned off", K::TAG),
            )),
            _ => Ok(()),
        }
    }
}

/// A catalog entry with what a probe found.
#[derive(Clone, Debug)]
pub struct ModelStatus<K: ModelKind> {
    pub entry: ModelEntry<K>,
    pub availability: Availability,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution<K: ModelKind> {
    Use(ModelId<K>),
    /// Nothing to run, by choice.
    Skip,
    /// The job cannot proceed until the user acts.
    Blocked(Blocker),
}

/// Picks the model for a job. Pure: the probe results arrive as data.
///
/// A pinned choice gets that model or a precise blocker. `Auto` drops models that cannot do the
/// job, then prefers in order: Ready over provisionable over unavailable, a clean fit over a
/// degraded one, `recommended`, higher `rank`, and finally the id so the result is deterministic.
/// A Ready model that fits with a limitation beats a better model that needs a download, because
/// `Auto` never downloads and a blocked job helps nobody.
///
/// An optional kind (summary, speakers) on `Auto` skips when the catalog is empty, so a missing
/// diarizer leaves remote speakers as one "Remote" speaker instead of blocking the job. When
/// every model rules itself out, it blocks on the best of them, so a meeting no summary model
/// can read says why instead of quietly getting no summary. A model that fits but needs a
/// download or is unavailable also blocks, since the user can act on it. `Pinned` never skips:
/// the user asked for that model.
pub fn resolve<K: ModelKind>(
    choice: &Choice<K>,
    statuses: &[ModelStatus<K>],
    needs: &K::Needs,
) -> Resolution<K> {
    match choice {
        Choice::Off if K::OPTIONAL => Resolution::Skip,
        Choice::Off => Resolution::Blocked(no_model(K::TAG)),
        Choice::Pinned(id) => match statuses.iter().find(|s| &s.entry.id == id) {
            None => Resolution::Blocked(no_model(K::TAG)),
            Some(status) => match K::fit(&status.entry.caps, needs) {
                Fit::No(why) => Resolution::Blocked(Blocker::Incompatible {
                    model: id.erase(),
                    why,
                }),
                Fit::Yes | Fit::Degraded(_) => settle(status),
            },
        },
        Choice::Auto => {
            let ranked = statuses.iter().map(|s| (s, K::fit(&s.entry.caps, needs)));
            let best = ranked
                .clone()
                .filter(|(_, fit)| !matches!(fit, Fit::No(_)))
                .min_by_key(|(s, fit)| preference(s, *fit));
            match best {
                Some((status, _)) => settle(status),
                None => match ranked.min_by_key(|(s, fit)| preference(s, *fit)) {
                    Some((status, Fit::No(why))) => Resolution::Blocked(Blocker::Incompatible {
                        model: status.entry.id.erase(),
                        why,
                    }),
                    _ if K::OPTIONAL => Resolution::Skip,
                    _ => Resolution::Blocked(no_model(K::TAG)),
                },
            }
        }
    }
}

fn no_model(kind: KindTag) -> Blocker {
    Blocker::NoModel { kind }
}

fn preference<K: ModelKind>(
    status: &ModelStatus<K>,
    fit: Fit,
) -> (u8, u8, bool, Reverse<i16>, ModelId<K>) {
    (
        status.availability.rank(),
        fit.rank(),
        !status.entry.recommended,
        Reverse(status.entry.rank),
        status.entry.id.clone(),
    )
}

fn settle<K: ModelKind>(status: &ModelStatus<K>) -> Resolution<K> {
    match &status.availability {
        Availability::Ready => Resolution::Use(status.entry.id.clone()),
        Availability::NeedsProvision { bytes } => Resolution::Blocked(Blocker::NeedsProvision {
            model: status.entry.id.erase(),
            bytes: *bytes,
        }),
        Availability::Unavailable(why) => Resolution::Blocked(Blocker::Unavailable {
            model: status.entry.id.erase(),
            why: why.clone(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{LangSet, LangTag, LanguagePref, Ms};
    use crate::models::{
        DiarizationNeed, DiarizationSupport, LangSupport, Mismatch, ModelRef, Stt, SttCaps,
        SttNeeds, Unavailable, UnavailableReason,
    };

    fn tag(s: &str) -> LangTag {
        LangTag::parse(s).unwrap()
    }

    fn id(s: &str) -> ModelId<Stt> {
        s.parse().unwrap()
    }

    fn mref(s: &str) -> ModelRef {
        ModelRef::parse(s).unwrap()
    }

    #[derive(Clone)]
    struct Row {
        id: &'static str,
        availability: Availability,
        recommended: bool,
        rank: i16,
        fixed_locale: bool,
    }

    fn row(id: &'static str, availability: Availability) -> Row {
        Row {
            id,
            availability,
            recommended: false,
            rank: 0,
            fixed_locale: false,
        }
    }

    fn statuses(rows: &[Row]) -> Vec<ModelStatus<Stt>> {
        rows.iter()
            .map(|r| {
                let languages = if r.fixed_locale {
                    LangSupport::FixedLocale {
                        tags: LangSet::Only(vec![tag("en-US")]),
                    }
                } else {
                    LangSupport::Multilingual {
                        auto_detect: true,
                        tags: LangSet::Any,
                    }
                };
                ModelStatus {
                    entry: ModelEntry {
                        id: id(r.id),
                        display_name: r.id.into(),
                        about: Default::default(),
                        caps: SttCaps {
                            languages,
                            diarization: DiarizationSupport::None,
                            max_input: None,
                            window: None,
                        },
                        recommended: r.recommended,
                        rank: r.rank,
                        disk_bytes: None,
                        built_in: false,
                    },
                    availability: r.availability.clone(),
                }
            })
            .collect()
    }

    fn needs(language: LanguagePref) -> SttNeeds {
        SttNeeds {
            language,
            expected: Vec::new(),
            diarization: DiarizationNeed::NotNeeded,
            input: Ms(1),
        }
    }

    fn down() -> Availability {
        Availability::NeedsProvision { bytes: Some(100) }
    }

    fn gone() -> Availability {
        Availability::Unavailable(Unavailable {
            reason: UnavailableReason::ServerUnreachable {
                endpoint: "localhost".into(),
            },
            fix: None,
        })
    }

    fn run(choice: Choice<Stt>, rows: &[Row], language: LanguagePref) -> Resolution<Stt> {
        resolve(&choice, &statuses(rows), &needs(language))
    }

    fn use_(s: &str) -> Resolution<Stt> {
        Resolution::Use(id(s))
    }

    #[test]
    fn auto_ordering_table() {
        let ready = Availability::Ready;
        let rec = |mut r: Row| {
            r.recommended = true;
            r
        };
        let ranked = |mut r: Row, n| {
            r.rank = n;
            r
        };
        type Case = (&'static str, Vec<Row>, Resolution<Stt>);
        let cases: Vec<Case> = vec![
            (
                "ready beats absent",
                vec![row("a/x", down()), row("a/y", ready.clone())],
                use_("a/y"),
            ),
            (
                "recommended beats plain among ready",
                vec![row("a/x", ready.clone()), rec(row("a/y", ready.clone()))],
                use_("a/y"),
            ),
            (
                "ready beats recommended-but-absent",
                vec![rec(row("a/x", down())), row("a/y", ready.clone())],
                use_("a/y"),
            ),
            (
                "rank breaks the tie, higher wins",
                vec![
                    ranked(row("a/x", ready.clone()), 1),
                    ranked(row("a/y", ready.clone()), 5),
                ],
                use_("a/y"),
            ),
            (
                "id breaks the final tie",
                vec![row("b/m", ready.clone()), row("a/m", ready.clone())],
                use_("a/m"),
            ),
            (
                "absent provisionable beats unavailable",
                vec![row("a/x", gone()), row("a/y", down())],
                Resolution::Blocked(Blocker::NeedsProvision {
                    model: mref("a/y"),
                    bytes: Some(100),
                }),
            ),
            (
                "only unavailable models",
                vec![row("a/x", gone())],
                Resolution::Blocked(Blocker::Unavailable {
                    model: mref("a/x"),
                    why: Unavailable {
                        reason: UnavailableReason::ServerUnreachable {
                            endpoint: "localhost".into(),
                        },
                        fix: None,
                    },
                }),
            ),
            (
                "empty catalog",
                vec![],
                Resolution::Blocked(Blocker::NoModel { kind: KindTag::Stt }),
            ),
        ];
        for (name, rows, want) in cases {
            assert_eq!(run(Choice::Auto, &rows, LanguagePref::Auto), want, "{name}");
        }
    }

    #[test]
    fn auto_never_picks_a_model_that_cannot_do_the_job() {
        let mut apple = row("apple/speech", Availability::Ready);
        apple.fixed_locale = true;
        apple.recommended = true;
        let whisper = row("sona/whisper", Availability::Ready);
        let rows = [apple.clone(), whisper];
        assert_eq!(
            run(Choice::Auto, &rows, LanguagePref::Auto),
            use_("sona/whisper")
        );
        assert_eq!(
            run(Choice::Auto, &rows, LanguagePref::Pinned(tag("en-US"))),
            use_("apple/speech")
        );
        assert_eq!(
            run(Choice::Auto, &[apple], LanguagePref::Auto),
            Resolution::Blocked(Blocker::Incompatible {
                model: mref("apple/speech"),
                why: Mismatch::AutoLanguageUnsupported
            })
        );
    }

    #[test]
    fn a_ready_degraded_model_beats_a_clean_one_that_needs_download() {
        let mut clean = row("a/clean", down());
        clean.recommended = true;
        let mut rows = statuses(&[clean, row("a/limited", Availability::Ready)]);
        rows[1].entry.caps.max_input = Some(Ms(0));
        assert_eq!(
            resolve(&Choice::Auto, &rows, &needs(LanguagePref::Auto)),
            use_("a/limited")
        );
    }

    #[test]
    fn a_clean_fit_beats_a_degraded_one_when_both_are_ready() {
        let mut rows_statuses = statuses(&[
            row("a/limited", Availability::Ready),
            row("b/clean", Availability::Ready),
        ]);
        rows_statuses[0].entry.caps.max_input = Some(Ms(0));
        let got = resolve(&Choice::Auto, &rows_statuses, &needs(LanguagePref::Auto));
        assert_eq!(got, use_("b/clean"));
    }

    #[test]
    fn pinned_table() {
        let ready = Availability::Ready;
        let mut apple = row("apple/speech", ready.clone());
        apple.fixed_locale = true;
        type Case = (
            &'static str,
            &'static str,
            Vec<Row>,
            LanguagePref,
            Resolution<Stt>,
        );
        let cases: Vec<Case> = vec![
            (
                "ready",
                "a/x",
                vec![row("a/x", ready.clone()), row("a/y", ready.clone())],
                LanguagePref::Auto,
                use_("a/x"),
            ),
            (
                "pin wins over a better model",
                "a/x",
                vec![row("a/x", ready.clone()), {
                    let mut r = row("a/y", ready.clone());
                    r.recommended = true;
                    r
                }],
                LanguagePref::Auto,
                use_("a/x"),
            ),
            (
                "absent",
                "a/x",
                vec![row("a/x", down())],
                LanguagePref::Auto,
                Resolution::Blocked(Blocker::NeedsProvision {
                    model: mref("a/x"),
                    bytes: Some(100),
                }),
            ),
            (
                "unknown id",
                "a/gone",
                vec![row("a/x", ready.clone())],
                LanguagePref::Auto,
                Resolution::Blocked(Blocker::NoModel { kind: KindTag::Stt }),
            ),
            (
                "incompatible language",
                "apple/speech",
                vec![apple],
                LanguagePref::Auto,
                Resolution::Blocked(Blocker::Incompatible {
                    model: mref("apple/speech"),
                    why: Mismatch::AutoLanguageUnsupported,
                }),
            ),
        ];
        for (name, pin, rows, language, want) in cases {
            assert_eq!(
                run(Choice::Pinned(id(pin)), &rows, language),
                want,
                "{name}"
            );
        }
    }

    #[test]
    fn off_skips_optional_kinds_and_blocks_required_ones() {
        use crate::models::{Llm, LlmNeeds};
        assert_eq!(
            resolve::<Llm>(
                &Choice::Off,
                &[],
                &LlmNeeds {
                    main: None,
                    all: Vec::new()
                }
            ),
            Resolution::Skip
        );
        assert_eq!(
            run(Choice::Off, &[], LanguagePref::Auto),
            Resolution::Blocked(Blocker::NoModel { kind: KindTag::Stt })
        );
    }

    #[test]
    fn auto_on_an_optional_kind_skips_only_with_an_empty_catalog() {
        use crate::models::{Diar, DiarCaps, DiarNeeds, Llm, LlmCaps, LlmNeeds};

        fn llm(name: &str, languages: LangSet, availability: Availability) -> ModelStatus<Llm> {
            ModelStatus {
                entry: ModelEntry {
                    id: name.parse().unwrap(),
                    display_name: name.into(),
                    about: Default::default(),
                    caps: LlmCaps {
                        context_tokens: 4096,
                        languages,
                    },
                    recommended: false,
                    rank: 0,
                    disk_bytes: None,
                    built_in: false,
                },
                availability,
            }
        }
        let vi = LlmNeeds {
            main: Some(tag("vi")),
            all: vec![tag("vi")],
        };
        let en_only = || LangSet::Only(vec![tag("en")]);

        type Case = (
            &'static str,
            Choice<Llm>,
            Vec<ModelStatus<Llm>>,
            Resolution<Llm>,
        );
        let cases: Vec<Case> = vec![
            ("empty catalog", Choice::Auto, vec![], Resolution::Skip),
            (
                "every model rules itself out, so the job says why",
                Choice::Auto,
                vec![llm("a/en", en_only(), Availability::Ready)],
                Resolution::Blocked(Blocker::Incompatible {
                    model: mref("a/en"),
                    why: Mismatch::LanguageUnsupported,
                }),
            ),
            (
                "a ready fit is used",
                Choice::Auto,
                vec![
                    llm("a/en", en_only(), Availability::Ready),
                    llm("a/any", LangSet::Any, Availability::Ready),
                ],
                Resolution::Use("a/any".parse().unwrap()),
            ),
            (
                "a fitting model that needs a download still blocks, the user can act",
                Choice::Auto,
                vec![llm("a/any", LangSet::Any, down())],
                Resolution::Blocked(Blocker::NeedsProvision {
                    model: mref("a/any"),
                    bytes: Some(100),
                }),
            ),
            (
                "pinned to a model that is not in the catalog still blocks",
                Choice::Pinned("a/gone".parse().unwrap()),
                vec![],
                Resolution::Blocked(Blocker::NoModel { kind: KindTag::Llm }),
            ),
            (
                "pinned to an unavailable model still blocks",
                Choice::Pinned("a/any".parse().unwrap()),
                vec![llm("a/any", LangSet::Any, gone())],
                Resolution::Blocked(Blocker::Unavailable {
                    model: mref("a/any"),
                    why: Unavailable {
                        reason: UnavailableReason::ServerUnreachable {
                            endpoint: "localhost".into(),
                        },
                        fix: None,
                    },
                }),
            ),
            (
                "pinned to a model that cannot do the job still blocks",
                Choice::Pinned("a/en".parse().unwrap()),
                vec![llm("a/en", en_only(), Availability::Ready)],
                Resolution::Blocked(Blocker::Incompatible {
                    model: mref("a/en"),
                    why: Mismatch::LanguageUnsupported,
                }),
            ),
        ];
        for (name, choice, rows, want) in cases {
            assert_eq!(resolve(&choice, &rows, &vi), want, "{name}");
        }

        assert_eq!(
            resolve::<Diar>(&Choice::Auto, &[], &DiarNeeds),
            Resolution::Skip,
            "no diarizer leaves remote speakers as one"
        );
        let diarizer: ModelStatus<Diar> = ModelStatus {
            entry: ModelEntry {
                id: "sherpa/pyannote".parse().unwrap(),
                display_name: "pyannote".into(),
                about: Default::default(),
                caps: DiarCaps { max_speakers: None },
                recommended: false,
                rank: 0,
                disk_bytes: None,
                built_in: false,
            },
            availability: Availability::Ready,
        };
        assert_eq!(
            resolve(&Choice::Auto, &[diarizer], &DiarNeeds),
            Resolution::Use("sherpa/pyannote".parse().unwrap())
        );
    }

    #[test]
    fn auto_on_a_required_kind_still_blocks_when_nothing_fits() {
        assert_eq!(
            run(Choice::Auto, &[], LanguagePref::Auto),
            Resolution::Blocked(Blocker::NoModel { kind: KindTag::Stt })
        );
    }

    #[test]
    fn auto_prefers_a_ready_fallback_over_an_unreachable_server() {
        use crate::models::{Llm, LlmCaps, LlmNeeds};
        let row = |name: &str, languages, recommended, availability| ModelStatus::<Llm> {
            entry: ModelEntry {
                id: name.parse().unwrap(),
                display_name: name.into(),
                about: Default::default(),
                caps: LlmCaps {
                    context_tokens: 4096,
                    languages,
                },
                recommended,
                rank: 0,
                disk_bytes: None,
                built_in: false,
            },
            availability,
        };
        let gemma = row("ollama/gemma", LangSet::Any, true, gone());
        let apple = row(
            "apple/foundation-model",
            LangSet::Only(vec![tag("en"), tag("fr")]),
            false,
            Availability::Ready,
        );
        let needs = |all: &[&str]| LlmNeeds {
            main: None,
            all: all.iter().map(|t| tag(t)).collect(),
        };
        let statuses = [gemma, apple];
        assert_eq!(
            resolve(&Choice::Auto, &statuses, &needs(&["en"])),
            Resolution::Use("apple/foundation-model".parse().unwrap()),
            "a ready model that reads the meeting beats a recommended one that cannot run"
        );
        assert!(
            matches!(
                resolve(&Choice::Auto, &statuses, &needs(&[])),
                Resolution::Blocked(Blocker::Unavailable { model, .. }) if model == mref("ollama/gemma")
            ),
            "unknown languages never reach a model that reads only some, so Auto waits for Gemma"
        );
        assert!(
            matches!(
                resolve(&Choice::Auto, &statuses, &needs(&["en", "tl"])),
                Resolution::Blocked(Blocker::Unavailable { model, .. }) if model == mref("ollama/gemma")
            ),
            "a meeting the ready model cannot read waits for the one that can"
        );
    }

    #[test]
    fn choice_validation() {
        assert!(Choice::<Stt>::off().is_err());
        assert!(Choice::<crate::models::Llm>::off().is_ok());
        assert!(Choice::<Stt>::Auto.validate().is_ok());
        assert!(Choice::Pinned(id("a/x")).validate().is_ok());
    }
}
