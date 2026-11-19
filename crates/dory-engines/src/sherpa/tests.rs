use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use dory_core::domain::{Boost, LangTag, Ms, VocabularyTerm};
use dory_core::models::{Availability, ModelId, Stt};
use dory_core::ports::{CancelToken, EngineError, ModelProvider, ProvisionError, ProvisionSink};
use sha2::{Digest, Sha256};
use sherpa_onnx::OfflineRecognizerResult;

use super::Sherpa;
use super::catalog::{self, Archive, Layout, Model};
use super::engine::{Built, Recipe, detected_language, engine_code, recognizer_config, segments};
use super::fetch::hex;
use super::hotwords::{bpe_vocab_path, ensure_bpe_vocab, hotwords_string};

const ROOT: &str = "tiny-int8";

fn files() -> Vec<(&'static str, Vec<u8>)> {
    let mut x: u32 = 0x9e37_79b9;
    let noise = (0..300_000)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect();
    vec![
        ("encoder.int8.onnx", noise),
        ("decoder.int8.onnx", b"decoder".to_vec()),
        ("joiner.int8.onnx", b"joiner".to_vec()),
        ("tokens.txt", b"<unk> 0\n".to_vec()),
    ]
}

fn archive() -> Vec<u8> {
    let encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
    let mut tar = tar::Builder::new(encoder);
    let mut add = |path: &str, data: &[u8]| {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, path, data).unwrap();
    };
    for (name, data) in files() {
        add(&format!("{ROOT}/{name}"), &data);
    }
    add(&format!("{ROOT}/test_wavs/en.wav"), b"not needed");
    tar.into_inner().unwrap().finish().unwrap()
}

fn model(archive: &[u8], sha256: &str) -> Model {
    Model {
        name: "tiny",
        display_name: "Tiny",
        archive: Archive {
            file: "tiny.tar.bz2",
            bytes: archive.len() as u64,
            sha256: Box::leak(sha256.to_owned().into_boxed_str()),
            root: ROOT,
        },
        layout: Layout::Parakeet,
        disk_bytes: 300_100,
        languages: &[("en", "en")],
        window_secs: 30,
        sentence_times: false,
        vocabulary: false,
        recommended: false,
        rank: 0,
        summary: "",
        pros: &[],
        cons: &[],
    }
}

fn sha(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

#[derive(Clone, Copy)]
enum Behavior {
    Whole,
    /// The first response sends half its body, then closes.
    TruncateFirst,
    /// Every response sends this many bytes, then holds the connection open.
    Stall(usize),
}

struct Server {
    url: String,
    /// The range start of each request, in order.
    ranges: Arc<Mutex<Vec<u64>>>,
}

fn serve(body: Vec<u8>, behavior: Behavior) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/models", listener.local_addr().unwrap());
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&ranges);
    thread::spawn(move || {
        for (n, stream) in listener.incoming().enumerate() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut start = 0;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
                if let Some(range) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                    start = range.trim().trim_end_matches('-').parse().unwrap();
                }
            }
            log.lock().unwrap().push(start);
            let slice = &body[start as usize..];
            let status = if start > 0 {
                format!(
                    "206 Partial Content\r\nContent-Range: bytes {start}-{}/{}",
                    body.len() - 1,
                    body.len()
                )
            } else {
                "200 OK".to_owned()
            };
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                slice.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            match behavior {
                Behavior::Whole => {
                    let _ = stream.write_all(slice);
                }
                Behavior::TruncateFirst if n == 0 => {
                    let _ = stream.write_all(&slice[..slice.len() / 2]);
                }
                Behavior::TruncateFirst => {
                    let _ = stream.write_all(slice);
                }
                Behavior::Stall(sent) => {
                    let _ = stream.write_all(&slice[..sent]);
                    let _ = stream.flush();
                    thread::spawn(move || {
                        thread::sleep(Duration::from_secs(20));
                        drop(stream);
                    });
                }
            }
        }
    });
    Server { url, ranges }
}

#[derive(Default)]
struct Sink {
    steps: Vec<String>,
    last: Option<(u64, Option<u64>)>,
}

impl ProvisionSink for Sink {
    fn step(&mut self, label: &str) {
        self.steps.push(label.to_owned());
    }
    fn bytes(&mut self, done: u64, total: Option<u64>) {
        self.last = Some((done, total));
    }
}

fn sherpa(dir: &Path, url: &str, model: Model) -> Sherpa {
    Sherpa {
        dir: dir.to_owned(),
        base_url: url.to_owned(),
        models: vec![model],
    }
}

fn tiny() -> ModelId<Stt> {
    "sherpa/tiny".parse().unwrap()
}

fn assert_installed(dir: &Path) {
    for (name, data) in files() {
        assert_eq!(
            fs::read(dir.join("tiny").join(name)).unwrap(),
            data,
            "{name}"
        );
    }
    assert!(
        !dir.join("tiny/test_wavs").exists(),
        "only model files are extracted"
    );
}

fn leftovers(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".extract"))
        .collect();
    names.extend(
        fs::read_dir(dir.join(".partial"))
            .into_iter()
            .flatten()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| !n.ends_with(".lock")),
    );
    names
}

#[test]
fn provisions_a_model_and_reports_it_ready() {
    let data = archive();
    let server = serve(data.clone(), Behavior::Whole);
    let dir = tempfile::tempdir().unwrap();
    let provider = sherpa(dir.path(), &server.url, model(&data, &sha(&data)));
    assert_eq!(
        provider.availability(&tiny()),
        Availability::NeedsProvision {
            bytes: Some(data.len() as u64)
        }
    );

    let mut sink = Sink::default();
    provider
        .provision(&tiny(), &mut sink, &CancelToken::new())
        .unwrap();

    assert_eq!(provider.availability(&tiny()), Availability::Ready);
    assert_installed(dir.path());
    assert_eq!(sink.steps, ["Downloading", "Verifying", "Unpacking"]);
    assert_eq!(
        sink.last,
        Some((data.len() as u64, Some(data.len() as u64)))
    );
    assert_eq!(leftovers(dir.path()), Vec::<String>::new());
}

#[test]
fn resumes_with_a_range_request_after_a_truncated_response() {
    let data = archive();
    let server = serve(data.clone(), Behavior::TruncateFirst);
    let dir = tempfile::tempdir().unwrap();
    let provider = sherpa(dir.path(), &server.url, model(&data, &sha(&data)));

    provider
        .provision(&tiny(), &mut Sink::default(), &CancelToken::new())
        .unwrap();

    assert_installed(dir.path());
    assert_eq!(
        *server.ranges.lock().unwrap(),
        [0, data.len() as u64 / 2],
        "the second request starts where the first stopped"
    );
}

#[test]
fn a_bad_hash_deletes_the_download_and_fails() {
    let data = archive();
    let server = serve(data.clone(), Behavior::Whole);
    let dir = tempfile::tempdir().unwrap();
    let provider = sherpa(
        dir.path(),
        &server.url,
        model(&data, &sha(b"something else")),
    );

    let err = provider
        .provision(&tiny(), &mut Sink::default(), &CancelToken::new())
        .unwrap_err();

    assert!(matches!(err, ProvisionError::ChecksumMismatch), "{err:?}");
    assert_eq!(
        provider.availability(&tiny()),
        Availability::NeedsProvision {
            bytes: Some(data.len() as u64)
        }
    );
    assert_eq!(leftovers(dir.path()), Vec::<String>::new());
    assert!(!dir.path().join("tiny").exists());
}

#[test]
fn provisioning_a_ready_model_makes_no_request() {
    let data = archive();
    let server = serve(data.clone(), Behavior::Whole);
    let dir = tempfile::tempdir().unwrap();
    let provider = sherpa(dir.path(), &server.url, model(&data, &sha(&data)));
    let cancel = CancelToken::new();
    provider
        .provision(&tiny(), &mut Sink::default(), &cancel)
        .unwrap();

    let started = Instant::now();
    let mut sink = Sink::default();
    provider.provision(&tiny(), &mut sink, &cancel).unwrap();

    assert!(started.elapsed() < Duration::from_millis(100));
    assert_eq!(server.ranges.lock().unwrap().len(), 1);
    assert!(sink.steps.is_empty());
}

#[test]
fn finishes_whatever_a_crash_left_behind() {
    let data = archive();
    let server = serve(data.clone(), Behavior::Whole);
    let dir = tempfile::tempdir().unwrap();
    let half = data.len() / 2;
    fs::create_dir_all(dir.path().join(".partial")).unwrap();
    fs::write(dir.path().join(".partial/tiny.tar.bz2"), &data[..half]).unwrap();
    fs::create_dir_all(dir.path().join(".extract-tiny-x1/sub")).unwrap();
    fs::create_dir_all(dir.path().join("tiny")).unwrap();
    fs::write(dir.path().join("tiny/tokens.txt"), b"stale").unwrap();
    let provider = sherpa(dir.path(), &server.url, model(&data, &sha(&data)));
    assert_eq!(
        provider.availability(&tiny()),
        Availability::NeedsProvision {
            bytes: Some((data.len() - half) as u64)
        }
    );

    provider
        .provision(&tiny(), &mut Sink::default(), &CancelToken::new())
        .unwrap();

    assert_installed(dir.path());
    assert_eq!(*server.ranges.lock().unwrap(), [half as u64]);
    assert_eq!(leftovers(dir.path()), Vec::<String>::new());
}

#[test]
fn cancel_returns_within_a_second_and_a_later_call_resumes() {
    let data = archive();
    let stalled = serve(data.clone(), Behavior::Stall(1000));
    let dir = tempfile::tempdir().unwrap();
    let cancel = CancelToken::new();
    let remote = cancel.clone();
    let cancelled_at = thread::spawn(move || {
        thread::sleep(Duration::from_millis(300));
        remote.cancel();
        Instant::now()
    });

    let err = sherpa(dir.path(), &stalled.url, model(&data, &sha(&data)))
        .provision(&tiny(), &mut Sink::default(), &cancel)
        .unwrap_err();
    let returned = Instant::now();

    assert!(matches!(err, ProvisionError::Cancelled), "{err:?}");
    let waited = returned - cancelled_at.join().unwrap();
    assert!(
        waited < Duration::from_secs(1),
        "returned {waited:?} after cancel"
    );

    let healthy = serve(data.clone(), Behavior::Whole);
    let provider = sherpa(dir.path(), &healthy.url, model(&data, &sha(&data)));
    provider
        .provision(&tiny(), &mut Sink::default(), &CancelToken::new())
        .unwrap();
    assert_installed(dir.path());
    assert_eq!(*healthy.ranges.lock().unwrap(), [1000]);
}

fn recipe(whisper_language: Option<&'static str>, threads: u8) -> Recipe {
    Recipe {
        whisper_language,
        threads,
        sentence_times: false,
        beam: false,
    }
}

#[test]
fn sentence_times_turn_on_segment_timestamps_and_leave_token_timestamps_off() {
    let dir = PathBuf::from("/m");
    for sentence_times in [false, true] {
        let config = recognizer_config(
            Layout::WhisperTurbo,
            &dir,
            Recipe {
                sentence_times,
                ..recipe(None, 4)
            },
        );
        let whisper = &config.model_config.whisper;
        assert_eq!(whisper.enable_segment_timestamps, sentence_times);
        assert!(!whisper.enable_token_timestamps);
    }
}

#[test]
fn whisper_times_sentences_in_28_second_windows() {
    let sherpa = Sherpa::new("/m", Sherpa::RELEASES);
    let caps = |name: &str| {
        sherpa
            .catalog()
            .into_iter()
            .find(|e| e.id.name() == name)
            .unwrap()
            .caps
    };
    let whisper = caps("whisper-turbo");
    assert_eq!(whisper.window, Some(Ms(28_000)));
    assert!(whisper.sentence_times);
    assert!(!caps("parakeet-tdt-0.6b-v3").sentence_times);
}

fn configured_paths(layout: Layout, dir: &Path) -> HashMap<&'static str, Option<String>> {
    let config = recognizer_config(layout, dir, recipe(None, 4));
    let m = &config.model_config;
    HashMap::from([
        ("transducer.encoder", m.transducer.encoder.clone()),
        ("transducer.decoder", m.transducer.decoder.clone()),
        ("transducer.joiner", m.transducer.joiner.clone()),
        ("whisper.encoder", m.whisper.encoder.clone()),
        ("whisper.decoder", m.whisper.decoder.clone()),
        ("tokens", m.tokens.clone().filter(|t| !t.is_empty())),
    ])
}

#[test]
fn each_layout_configures_the_files_its_archive_holds() {
    let dir = PathBuf::from("/m");
    let at = |f: &str| Some(format!("/m/{f}"));
    let cases: [(Layout, &[(&str, &str)]); 2] = [
        (
            Layout::Parakeet,
            &[
                ("transducer.encoder", "encoder.int8.onnx"),
                ("transducer.decoder", "decoder.int8.onnx"),
                ("transducer.joiner", "joiner.int8.onnx"),
                ("tokens", "tokens.txt"),
            ],
        ),
        (
            Layout::WhisperTurbo,
            &[
                ("whisper.encoder", "turbo-encoder.int8.onnx"),
                ("whisper.decoder", "turbo-decoder.int8.onnx"),
                ("tokens", "turbo-tokens.txt"),
            ],
        ),
    ];
    for (layout, want) in cases {
        let got = configured_paths(layout, &dir);
        for (field, path) in &got {
            let expected = want
                .iter()
                .find(|(f, _)| f == field)
                .and_then(|(_, p)| at(p));
            assert_eq!(*path, expected, "{layout:?} {field}");
        }
        for (_, file) in want {
            assert!(
                layout.files().iter().any(|f| f.starts_with(file)),
                "{layout:?} extracts {file}"
            );
        }
        let config = recognizer_config(layout, &dir, recipe(None, 2));
        assert_eq!(config.model_config.provider.as_deref(), Some("cpu"));
        assert_eq!(config.model_config.num_threads, 2, "{layout:?}");
    }
    let parakeet = recognizer_config(Layout::Parakeet, &dir, recipe(None, 4));
    assert_eq!(
        parakeet.model_config.model_type.as_deref(),
        Some("nemo_transducer")
    );
    let whisper = recognizer_config(Layout::WhisperTurbo, &dir, recipe(Some("tl"), 4));
    assert_eq!(whisper.model_config.whisper.language.as_deref(), Some("tl"));
    assert_eq!(
        whisper.model_config.whisper.task.as_deref(),
        Some("transcribe")
    );
}

#[test]
fn pinned_languages_map_to_each_engines_code() {
    let models = catalog::models();
    let by_name = |name: &str| models.iter().find(|m| m.name == name).unwrap();
    let cases = [
        ("whisper-turbo", "tl", Some("tl")),
        ("whisper-turbo", "fil", Some("tl")),
        ("whisper-turbo", "jv", Some("jw")),
        ("parakeet-tdt-0.6b-v3", "de", Some("de")),
        ("parakeet-tdt-0.6b-v3", "tl", None),
    ];
    for (model, wanted, want) in cases {
        assert_eq!(
            engine_code(by_name(model), &tag(wanted)),
            want,
            "{model} {wanted}"
        );
    }
}

fn result(tokens: &[(&str, f32, f32)]) -> OfflineRecognizerResult {
    OfflineRecognizerResult {
        text: tokens.iter().map(|t| t.0.replace('▁', " ")).collect(),
        tokens: tokens.iter().map(|t| t.0.to_owned()).collect(),
        timestamps: Some(tokens.iter().map(|t| t.1).collect()),
        durations: Some(tokens.iter().map(|t| t.2).collect()),
        ..no_language()
    }
}

fn tag(s: &str) -> LangTag {
    LangTag::parse(s).unwrap()
}

fn no_language() -> OfflineRecognizerResult {
    parsed(r#"{"text": "", "tokens": []}"#)
}

fn parsed(json: &str) -> OfflineRecognizerResult {
    serde_json::from_str(json).expect("a recognizer result")
}

fn ms_segments(result: &OfflineRecognizerResult, clip: u64) -> Vec<(u64, u64, String)> {
    segments(result, Ms(clip))
        .into_iter()
        .map(|(r, t)| (r.start().0, r.end().0, t))
        .collect()
}

#[test]
fn token_times_split_at_sentence_ends_and_long_pauses() {
    let r = result(&[
        ("▁Hello", 0.20, 0.30),
        ("▁world", 0.56, 0.30),
        (".", 0.86, 0.04),
        ("▁It", 1.00, 0.10),
        ("▁costs", 1.12, 0.30),
        ("▁3", 1.50, 0.10),
        (".", 1.60, 0.04),
        ("5", 1.64, 0.10),
        ("▁now", 1.80, 0.20),
        ("▁later", 3.50, 0.30),
        ("▁ok", 3.90, 0.20),
        ("?", 4.10, 0.04),
    ]);
    assert_eq!(
        ms_segments(&r, 5_000),
        [
            (200, 900, "Hello world.".to_owned()),
            (1000, 2000, "It costs 3.5 now".to_owned()),
            (3500, 4140, "later ok?".to_owned()),
        ]
    );
}

#[test]
fn without_token_times_the_window_is_one_segment() {
    let r = OfflineRecognizerResult {
        text: " Kumusta ka? I'm fine. ".into(),
        tokens: vec![],
        timestamps: None,
        durations: None,
        ..no_language()
    };
    assert_eq!(
        ms_segments(&r, 30_000),
        [(0, 30_000, "Kumusta ka? I'm fine.".to_owned())]
    );
    let silent = OfflineRecognizerResult {
        text: "  ".into(),
        tokens: vec![],
        timestamps: None,
        durations: None,
        ..no_language()
    };
    assert!(ms_segments(&silent, 30_000).is_empty());
}

#[test]
fn segment_times_never_pass_the_clip_end() {
    let r = result(&[("▁tail", 9.90, 0.50)]);
    assert_eq!(
        ms_segments(&r, 10_000),
        [(9_900, 10_000, "tail".to_owned())]
    );
}

/// What sherpa-onnx's `AsJsonString` returns for a Whisper window on automatic language.
const WHISPER_JSON: &str = r#"{"lang": "tl", "emotion": "", "event": "", "text": " Kumusta ka? Mabuti naman.", "timestamps": [], "durations": [], "tokens": [" Kum", "usta", " ka", "?"], "words": []}"#;

#[test]
fn whisper_result_lang_becomes_the_detected_language() {
    let result = parsed(WHISPER_JSON);
    assert_eq!(result.lang, "tl");
    assert_eq!(
        detected_language(Layout::WhisperTurbo, &result),
        Some(tag("tl"))
    );
    let english = parsed(r#"{"lang": "en", "text": "Hello.", "tokens": []}"#);
    assert_eq!(
        detected_language(Layout::WhisperTurbo, &english),
        Some(tag("en"))
    );
}

#[test]
fn an_empty_lang_is_none() {
    let empty = parsed(r#"{"lang": "", "text": "Hello.", "tokens": []}"#);
    let missing = parsed(r#"{"text": "Hello.", "tokens": []}"#);
    let garbage = parsed(r#"{"lang": "<|nospeech|>", "text": "", "tokens": []}"#);
    for result in [empty, missing, garbage] {
        assert_eq!(detected_language(Layout::WhisperTurbo, &result), None);
    }
}

#[test]
fn parakeet_reports_no_language_even_when_the_result_carries_one() {
    let result = parsed(WHISPER_JSON);
    assert_eq!(detected_language(Layout::Parakeet, &result), None);
}

#[test]
fn a_result_without_sentence_times_parses_with_empty_ones() {
    let result = parsed(r#"{"text": "Hello.", "tokens": []}"#);
    assert!(result.segment_timestamps.is_empty());
    assert!(result.segment_durations.is_empty());
    assert!(result.segment_texts.is_empty());
}

#[test]
fn sentence_times_parse_when_the_recognizer_reports_them() {
    let result = parsed(
        r#"{"lang": "en", "text": "One. Two.", "tokens": [], "segment_timestamps": [0.0, 2.5], "segment_durations": [2.4, 1.0], "segment_texts": ["One.", "Two."]}"#,
    );
    assert_eq!(result.segment_timestamps, [0.0, 2.5]);
    assert_eq!(result.segment_durations, [2.4, 1.0]);
    assert_eq!(result.segment_texts, ["One.", "Two."]);
}

/// A Whisper turbo window with segment timestamps on, in `AsJsonString`'s shape: a Tagalog
/// sentence and an English one, the second running past the 20-second clip.
const SENTENCES_JSON: &str = r#"{"lang": "tl", "emotion": "", "event": "", "text": " Ang trapiko ay hindi madali. The beta build is ready.", "timestamps": [], "durations": [], "tokens": [" Ang", " tr", "ap", "iko"], "words": [], "segment_timestamps": [0.00, 9.40, 19.80], "segment_durations": [8.80, 11.00, 0.00], "segment_texts": [" Ang trapiko ay hindi madali.", " The beta build is ready.", " "]}"#;

#[test]
fn each_sentence_becomes_a_segment_with_its_own_time() {
    assert_eq!(
        ms_segments(&parsed(SENTENCES_JSON), 20_000),
        [
            (0, 8_800, "Ang trapiko ay hindi madali.".to_owned()),
            (9_400, 20_000, "The beta build is ready.".to_owned()),
        ],
        "a blank sentence is dropped, and none runs past the clip"
    );
}

#[test]
fn sentence_times_that_do_not_line_up_fall_back_to_one_segment() {
    let mut result = parsed(SENTENCES_JSON);
    result.segment_durations.pop();
    assert_eq!(
        ms_segments(&result, 20_000),
        [(
            0,
            20_000,
            "Ang trapiko ay hindi madali. The beta build is ready.".to_owned()
        )]
    );
}

#[test]
fn opening_a_missing_model_fails_without_loading() {
    let dir = tempfile::tempdir().unwrap();
    let provider = sherpa(dir.path(), "http://127.0.0.1:9", model(b"", ""));
    let err = provider.open(&tiny(), &CancelToken::new()).err().unwrap();
    assert!(matches!(err, EngineError::ModelMissing(_)), "{err:?}");
}

#[test]
fn the_recognizer_is_rebuilt_only_when_threads_the_whisper_language_or_sentence_times_change() {
    let mut built = Built::empty();
    let mut builds = Vec::new();
    let timed = Recipe {
        sentence_times: true,
        ..recipe(Some("vi"), 4)
    };
    let beam = Recipe {
        beam: true,
        ..recipe(None, 4)
    };
    let windows = [
        recipe(None, 4),
        recipe(None, 4),
        recipe(None, 2),
        recipe(None, 2),
        recipe(Some("vi"), 2),
        recipe(Some("vi"), 4),
        recipe(Some("vi"), 4),
        timed,
        timed,
        beam,
        beam,
    ];
    for want in windows {
        let got: &mut Recipe = built
            .get(want, |r| {
                builds.push(r);
                Ok::<_, ()>(r)
            })
            .unwrap();
        assert_eq!(*got, want, "each window runs on a recognizer built for it");
    }
    assert_eq!(
        builds,
        [
            recipe(None, 4),
            recipe(None, 2),
            recipe(Some("vi"), 2),
            recipe(Some("vi"), 4),
            timed,
            beam
        ]
    );
}

#[test]
fn a_failed_build_leaves_nothing_built_and_the_next_window_tries_again() {
    let mut built = Built::<Recipe>::empty();
    assert!(built.get(recipe(None, 4), |_| Err("corrupt")).is_err());
    let mut tries = 0;
    let got = built
        .get(recipe(None, 4), |r| {
            tries += 1;
            Ok::<_, &str>(r)
        })
        .copied();
    assert_eq!(got, Ok(recipe(None, 4)));
    assert_eq!(tries, 1);
}

fn term(text: &str, boost: Boost) -> VocabularyTerm {
    VocabularyTerm::new(text, boost)
}

#[test]
fn hotwords_join_with_slashes_and_only_a_strong_term_carries_a_score() {
    assert_eq!(hotwords_string(&[]), "");
    assert_eq!(hotwords_string(&[term("Lina", Boost::Normal)]), "Lina");
    assert_eq!(
        hotwords_string(&[
            term("Lina", Boost::Normal),
            term("Priya", Boost::Strong),
            term("Acme Corp", Boost::Normal),
            term("Kubernetes", Boost::Strong),
        ]),
        "Lina/Priya :6/Acme Corp/Kubernetes :6"
    );
    assert_eq!(
        hotwords_string(&[
            term("bad\0name", Boost::Normal),
            term("Lina", Boost::Normal)
        ]),
        "Lina"
    );
}

#[test]
fn beam_search_with_hotwords_configures_only_a_transducer_that_was_asked_for_it() {
    let dir = PathBuf::from("/models/parakeet");
    let beam = Recipe {
        beam: true,
        ..recipe(None, 4)
    };
    let config = recognizer_config(Layout::Parakeet, &dir, beam);
    assert_eq!(
        config.decoding_method.as_deref(),
        Some("modified_beam_search")
    );
    assert_eq!(config.model_config.modeling_unit.as_deref(), Some("bpe"));
    assert_eq!(
        config.model_config.bpe_vocab.as_deref(),
        Some("/models/parakeet.bpe.vocab"),
        "beside the model directory, which provisioning replaces"
    );
    assert_eq!(config.hotwords_score, 2.0);

    let greedy = recognizer_config(Layout::Parakeet, &dir, recipe(None, 4));
    assert_eq!(greedy.decoding_method, None);
    assert_eq!(greedy.model_config.modeling_unit, None);
    assert_eq!(greedy.model_config.bpe_vocab, None);
    assert_eq!(greedy.hotwords_score, 0.0);

    let whisper = recognizer_config(Layout::WhisperTurbo, &dir, beam);
    assert_eq!(whisper.decoding_method, None, "Whisper never beams");
    assert_eq!(whisper.model_config.bpe_vocab, None);
}

#[test]
fn only_parakeet_reports_vocabulary_support() {
    let sherpa = Sherpa::new("/m", Sherpa::RELEASES);
    for entry in sherpa.catalog() {
        assert_eq!(
            entry.caps.vocabulary,
            entry.id.name().starts_with("parakeet"),
            "{}",
            entry.id
        );
    }
}

#[test]
fn the_bpe_vocab_is_made_once_from_tokens_beside_the_model_directory() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("parakeet");
    fs::create_dir(&dir).unwrap();
    fs::write(
        dir.join("tokens.txt"),
        "<unk> 0\n\u{2581}the 1\n\u{2581} 2\nab 3\n",
    )
    .unwrap();

    let path = ensure_bpe_vocab(&dir).unwrap();
    assert_eq!(path, bpe_vocab_path(&dir));
    assert_eq!(path, root.path().join("parakeet.bpe.vocab"));
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "<unk>\t-1.0\n\u{2581}the\t-1.0\n\u{2581}\t-1.0\nab\t-1.0\n"
    );
    let mut left: Vec<_> = fs::read_dir(root.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(
        left,
        ["parakeet", "parakeet.bpe.vocab"],
        "no temporary file is left"
    );
    assert_eq!(
        fs::read_dir(&dir).unwrap().count(),
        1,
        "nothing is written inside the model"
    );

    fs::write(&path, "kept").unwrap();
    ensure_bpe_vocab(&dir).unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "kept",
        "an existing file is not rewritten"
    );
}

#[test]
fn a_missing_tokens_file_is_an_error_and_leaves_no_vocab() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("parakeet");
    fs::create_dir(&dir).unwrap();
    assert!(ensure_bpe_vocab(&dir).is_err());
    assert!(!bpe_vocab_path(&dir).exists());
}
