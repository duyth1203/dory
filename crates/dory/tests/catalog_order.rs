//! The order Auto tries the registered models in, pinned to the decision log. P5's rule in
//! `docs/plans/platform-ai.md` may move an Apple row up; when the operator accepts that, a new row
//! in `docs/plans/decisions.tsv` records it and these constants change with it.
#![cfg(target_os = "macos")]

use dory::{Config, PretendApple};
use dory_core::app::AppKind;
use dory_core::domain::{LangSet, LangTag, LanguagePref, Ms};
use dory_core::models::{
    Availability, Choice, DiarizationNeed, Llm, LlmNeeds, ModelKind, ModelStatus, Resolution, Stt,
    SttNeeds, resolve,
};
use dory_core::testing::block_on;

/// Auto's speech order for an English meeting with every model ready, as of the 2026-11-17
/// platform AI rows: Parakeet, then Apple Speech, then Whisper.
const SPEECH_ORDER: [&str; 3] = [
    "sherpa/parakeet-tdt-0.6b-v3",
    "apple/speech-en",
    "sherpa/whisper-turbo",
];

/// Auto's summary order for an English meeting with every model ready, as of the same rows:
/// Gemma first, Apple Intelligence only when no Gemma is ready.
const SUMMARY_ORDER: [&str; 3] = [
    "ollama/gemma4:e4b",
    "ollama/gemma4:12b",
    "apple/foundation-model",
];

/// Every registered row of one kind, all Ready, on a throwaway library with Apple's state
/// pretended, so the order does not depend on what this Mac has installed.
fn all_ready<K: AppKind>() -> Vec<ModelStatus<K>> {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        ollama_url: "http://127.0.0.1:1".into(),
        apple: Some(PretendApple::Off),
        ..Config::new(dir.path())
    };
    drop(dory::open(&config).expect("creates the library"));
    let reader = dory::open_library(&config).unwrap();
    block_on(reader.models::<K>())
        .unwrap()
        .into_iter()
        .map(|row| ModelStatus {
            availability: Availability::Ready,
            ..row.status
        })
        .collect()
}

/// The models Auto picks one after another, each time with the previous pick gone.
fn auto_order<K: ModelKind>(mut rows: Vec<ModelStatus<K>>, needs: &K::Needs) -> Vec<String> {
    let mut order = Vec::new();
    while let Resolution::Use(id) = resolve(&Choice::Auto, &rows, needs) {
        order.push(id.to_string());
        rows.retain(|row| row.entry.id != id);
    }
    order
}

fn english() -> LangTag {
    LangTag::parse("en").unwrap()
}

#[test]
fn auto_order_matches_the_logged_rule() {
    let speech = SttNeeds {
        language: LanguagePref::Auto,
        expected: vec![english()],
        expected_guessed: false,
        diarization: DiarizationNeed::NotNeeded,
        input: Ms(42_600),
    };
    assert_eq!(auto_order(all_ready::<Stt>(), &speech), SPEECH_ORDER);

    // A pretended Apple reads no languages, so give it the English it reads on a real Mac.
    let mut summary_rows = all_ready::<Llm>();
    for row in &mut summary_rows {
        if row.entry.id.to_string() == "apple/foundation-model" {
            row.entry.caps.languages = LangSet::Only(vec![english()]);
        }
    }
    let summary = LlmNeeds {
        main: Some(english()),
        spoken: vec![english()],
    };
    assert_eq!(auto_order(summary_rows, &summary), SUMMARY_ORDER);
}
