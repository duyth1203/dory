use dory_core::domain::{LangTag, LanguagePref, Ms};
use dory_core::models::{
    Availability, Catalog, DiarizationNeed, Fit, Mismatch, ModelId, ModelKind, Stt, SttNeeds,
};
use dory_core::ports::{CancelToken, EngineError, ModelProvider};
use dory_engines::Sherpa;

fn provider(dir: &std::path::Path) -> Sherpa {
    Sherpa::new(dir, "http://127.0.0.1:9")
}

fn id(name: &str) -> ModelId<Stt> {
    format!("sherpa/{name}").parse().unwrap()
}

fn tag(s: &str) -> LangTag {
    LangTag::parse(s).unwrap()
}

#[test]
fn the_catalog_is_valid_and_explains_every_model() {
    let dir = tempfile::tempdir().unwrap();
    let entries = provider(dir.path()).catalog();
    let catalog = Catalog::new(entries.clone()).unwrap();

    let ids: Vec<String> = catalog.entries().iter().map(|e| e.id.to_string()).collect();
    assert_eq!(
        ids,
        [
            "sherpa/parakeet-tdt-0.6b-v3",
            "sherpa/qwen3-asr-0.6b",
            "sherpa/whisper-turbo"
        ]
    );
    let recommended: Vec<_> = entries.iter().filter(|e| e.recommended).collect();
    assert_eq!(recommended.len(), 1);
    assert_eq!(recommended[0].id, id("parakeet-tdt-0.6b-v3"));
    for entry in &entries {
        assert!(!entry.about.summary.is_empty(), "{}", entry.id);
        assert!(!entry.about.pros.is_empty(), "{}", entry.id);
        assert!(!entry.about.cons.is_empty(), "{}", entry.id);
        assert!(
            entry.caps.window.is_some_and(|w| w >= Ms(30_000)),
            "{}",
            entry.id
        );
        assert!(entry.disk_bytes.is_some(), "{}", entry.id);
    }
}

#[test]
fn a_tagalog_and_english_meeting_fits_qwen3_and_whisper_but_not_parakeet() {
    let dir = tempfile::tempdir().unwrap();
    let entries = provider(dir.path()).catalog();
    let caps = |name: &str| &entries.iter().find(|e| e.id == id(name)).unwrap().caps;
    let needs = |language: LanguagePref, expected: &[&str]| SttNeeds {
        language,
        expected: expected.iter().map(|s| tag(s)).collect(),
        diarization: DiarizationNeed::NotNeeded,
        input: Ms(3_600_000),
    };
    let mixed = needs(LanguagePref::Auto, &["tl", "en"]);
    assert_eq!(Stt::fit(caps("qwen3-asr-0.6b"), &mixed), Fit::Yes);
    assert_eq!(Stt::fit(caps("whisper-turbo"), &mixed), Fit::Yes);
    assert_eq!(
        Stt::fit(caps("parakeet-tdt-0.6b-v3"), &mixed),
        Fit::No(Mismatch::LanguageUnsupported)
    );

    for pinned in ["tl", "fil"] {
        let need = needs(LanguagePref::Pinned(tag(pinned)), &[]);
        assert_eq!(
            Stt::fit(caps("qwen3-asr-0.6b"), &need),
            Fit::Yes,
            "{pinned}"
        );
        assert_eq!(Stt::fit(caps("whisper-turbo"), &need), Fit::Yes, "{pinned}");
    }
    let english = needs(LanguagePref::Auto, &["en"]);
    assert_eq!(Stt::fit(caps("parakeet-tdt-0.6b-v3"), &english), Fit::Yes);
}

#[test]
fn an_empty_models_folder_needs_each_whole_archive() {
    let dir = tempfile::tempdir().unwrap();
    let sherpa = provider(dir.path());
    let cases = [
        ("parakeet-tdt-0.6b-v3", 487_211_443),
        ("qwen3-asr-0.6b", 878_702_423),
        ("whisper-turbo", 563_790_207),
    ];
    for (name, bytes) in cases {
        assert_eq!(
            sherpa.availability(&id(name)),
            Availability::NeedsProvision { bytes: Some(bytes) },
            "{name}"
        );
    }
    assert!(matches!(
        sherpa.availability(&id("nope")),
        Availability::Unavailable(_)
    ));
}

#[test]
fn opening_a_model_that_is_not_downloaded_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let err = provider(dir.path())
        .open(&id("qwen3-asr-0.6b"), &CancelToken::new())
        .err()
        .unwrap();
    assert!(matches!(err, EngineError::ModelMissing(_)), "{err:?}");
}
