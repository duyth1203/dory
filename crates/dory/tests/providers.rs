//! The speech and summary providers the composition root registers, seen through the public
//! facade.
#![cfg(target_os = "macos")]

use dory::{Config, PretendApple};
use dory_core::app::AppKind;
use dory_core::domain::{Blocker, LangSet, LangTag, LanguagePref, Ms};
use dory_core::models::{
    Availability, Choice, DiarizationNeed, LangSupport, Llm, LlmNeeds, ModelKind, ModelStatus,
    Resolution, Stt, SttNeeds, Unavailable, UnavailableReason, resolve,
};
use dory_core::testing::block_on;

/// A library on a throwaway folder, with Ollama unreachable and Apple's state pretended, so no
/// row depends on what this Mac runs.
fn summary_rows(apple: PretendApple) -> Vec<ModelStatus<Llm>> {
    listed(apple)
}

/// The rows of one kind on a throwaway library.
fn listed<K: AppKind>(apple: PretendApple) -> Vec<ModelStatus<K>> {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        ollama_url: "http://127.0.0.1:1".into(),
        apple: Some(apple),
        ..Config::new(dir.path())
    };
    drop(dory::open(&config).expect("creates the library"));
    let reader = dory::open_library(&config).unwrap();
    block_on(reader.models::<K>())
        .unwrap()
        .into_iter()
        .map(|row| row.status)
        .collect()
}

fn with<K: ModelKind>(rows: &[ModelStatus<K>], ready: &[&str]) -> Vec<ModelStatus<K>> {
    rows.iter()
        .map(|row| ModelStatus {
            entry: row.entry.clone(),
            availability: if ready.contains(&row.entry.id.to_string().as_str()) {
                Availability::Ready
            } else {
                row.availability.clone()
            },
        })
        .collect()
}

/// An English meeting.
fn auto(rows: &[ModelStatus<Llm>]) -> Resolution<Llm> {
    let en = LangTag::parse("en").unwrap();
    let needs = LlmNeeds {
        main: Some(en.clone()),
        all: vec![en],
    };
    resolve(&Choice::Auto, rows, &needs)
}

/// The `app` feature registers Apple Intelligence on macOS; `dory-mcp` builds `dory` without
/// it, and lane 9's `otool -L` shows that binary links no Swift.
#[test]
fn apple_rows_appear_only_with_the_app_feature() {
    let rows = summary_rows(PretendApple::Off);
    let apple = rows
        .iter()
        .find(|row| row.entry.id.to_string() == "apple/foundation-model")
        .expect("the app feature registers Apple Intelligence");
    assert_eq!(apple.entry.display_name, "Apple Intelligence");
    assert_eq!(
        apple.availability,
        Availability::Unavailable(Unavailable {
            reason: UnavailableReason::TurnedOff {
                detail: "Apple Intelligence is turned off".into(),
                settings_url: "x-apple.systempreferences:com.apple.Siri-Settings.extension".into(),
            },
            fix: Some("Turn it on in System Settings, Apple Intelligence & Siri".into()),
        })
    );
    assert_eq!(
        apple.entry.caps.languages,
        LangSet::Only(Vec::new()),
        "with no languages read, the row claims none"
    );
}

#[test]
fn every_pretended_state_reaches_the_row() {
    let reason = |apple| {
        let rows = summary_rows(apple);
        let row = rows
            .iter()
            .find(|row| row.entry.id.to_string() == "apple/foundation-model")
            .unwrap();
        match &row.availability {
            Availability::Unavailable(why) => why.reason.clone(),
            other => panic!("{apple:?} reads as {other:?}"),
        }
    };
    assert!(matches!(
        reason(PretendApple::Ineligible),
        UnavailableReason::NotEligible { detail } if detail == "This Mac can't run Apple Intelligence"
    ));
    assert!(matches!(
        reason(PretendApple::Preparing),
        UnavailableReason::Preparing { .. }
    ));
    assert_eq!(
        reason(PretendApple::OldOs),
        UnavailableReason::OsTooOld {
            required: "macOS 26.4".into()
        }
    );
}

#[test]
fn auto_prefers_a_ready_gemma_and_falls_back_to_apple() {
    let unread = summary_rows(PretendApple::Off);
    assert!(
        !matches!(
            auto(&with(&unread, &["apple/foundation-model"])),
            Resolution::Use(_)
        ),
        "Apple turned on after launch, with no languages read, takes no meeting"
    );
    let mut rows = unread;
    for row in &mut rows {
        if row.entry.id.to_string() == "apple/foundation-model" {
            row.entry.caps.languages = LangSet::Only(vec![LangTag::parse("en").unwrap()]);
        }
    }
    let pick = |ready: &[&str]| match auto(&with(&rows, ready)) {
        Resolution::Use(id) => id.to_string(),
        other => panic!("{ready:?} resolves to {other:?}"),
    };
    assert_eq!(
        pick(&["ollama/gemma4:e4b", "apple/foundation-model"]),
        "ollama/gemma4:e4b"
    );
    assert_eq!(
        pick(&["ollama/gemma4:12b", "apple/foundation-model"]),
        "ollama/gemma4:12b"
    );
    assert_eq!(pick(&["apple/foundation-model"]), "apple/foundation-model");
    assert!(matches!(
        auto(&rows),
        Resolution::Blocked(Blocker::Unavailable { model, .. }) if model.as_str() == "ollama/gemma4:e4b"
    ));
}

/// Every speech row with exactly `ready` installed and the rest still to download, so the pick
/// does not depend on what this Mac has.
fn speech_with(rows: &[ModelStatus<Stt>], ready: &[&str]) -> Vec<ModelStatus<Stt>> {
    rows.iter()
        .map(|row| ModelStatus {
            entry: row.entry.clone(),
            availability: if ready.contains(&row.entry.id.to_string().as_str()) {
                Availability::Ready
            } else {
                Availability::NeedsProvision { bytes: None }
            },
        })
        .collect()
}

/// What Auto transcribes a meeting in `languages` with.
fn speech_pick(rows: &[ModelStatus<Stt>], languages: &[&str]) -> Resolution<Stt> {
    let needs = SttNeeds {
        language: LanguagePref::Auto,
        expected: languages
            .iter()
            .map(|tag| LangTag::parse(tag).unwrap())
            .collect(),
        diarization: DiarizationNeed::NotNeeded,
        input: Ms(42_600),
    };
    resolve(&Choice::Auto, rows, &needs)
}

fn used(resolution: Resolution<Stt>) -> String {
    match resolution {
        Resolution::Use(id) => id.to_string(),
        other => panic!("resolves to {other:?}"),
    }
}

const PARAKEET: &str = "sherpa/parakeet-tdt-0.6b-v3";
const APPLE_EN: &str = "apple/speech-en";

#[test]
fn apple_speech_rows_appear_with_the_app_feature() {
    let rows: Vec<ModelStatus<Stt>> = listed(PretendApple::Off);
    let english = rows
        .iter()
        .find(|row| row.entry.id.to_string() == APPLE_EN)
        .expect("the app feature registers Apple Speech");
    assert_eq!(english.entry.display_name, "Apple Speech (English)");
    let LangSupport::FixedLocale {
        tags: LangSet::Only(tags),
    } = &english.entry.caps.languages
    else {
        panic!("{:?}", english.entry.caps.languages);
    };
    assert!(tags.iter().any(|t| t.as_str() == "en-GB"), "{tags:?}");
    assert!(
        !rows
            .iter()
            .any(|row| row.entry.id.to_string() == "apple/speech-tl"),
        "macOS has no Tagalog speech"
    );
}

#[test]
fn auto_prefers_a_ready_parakeet_over_apple_speech() {
    let rows = speech_with(&listed(PretendApple::Off), &[PARAKEET, APPLE_EN]);
    assert_eq!(used(speech_pick(&rows, &["en"])), PARAKEET);
}

#[test]
fn auto_uses_apple_speech_for_one_known_language_when_no_sherpa_model_is_ready() {
    let rows = speech_with(&listed(PretendApple::Off), &[APPLE_EN]);
    assert_eq!(used(speech_pick(&rows, &["en"])), APPLE_EN);
    assert_eq!(used(speech_pick(&rows, &["en-GB"])), APPLE_EN);
    assert_eq!(used(speech_pick(&rows, &["en-US", "en-GB"])), APPLE_EN);
}

#[test]
fn auto_never_uses_apple_speech_for_two_or_unknown_languages() {
    let listed: Vec<ModelStatus<Stt>> = listed(PretendApple::Off);
    let every_apple_row: Vec<String> = listed
        .iter()
        .map(|row| row.entry.id.to_string())
        .filter(|id| id.starts_with("apple/"))
        .collect();
    let ready: Vec<&str> = every_apple_row.iter().map(String::as_str).collect();
    let rows = speech_with(&listed, &ready);
    for languages in [&["en", "tl"][..], &["en", "de"], &["tl"], &["en-PH"], &[]] {
        match speech_pick(&rows, languages) {
            Resolution::Use(id) => panic!("{languages:?} picks {id}"),
            Resolution::Blocked(Blocker::NeedsProvision { model, .. }) => {
                assert!(
                    model.as_str().starts_with("sherpa/"),
                    "{languages:?}: {model}"
                );
            }
            other => panic!("{languages:?} resolves to {other:?}"),
        }
    }
}
