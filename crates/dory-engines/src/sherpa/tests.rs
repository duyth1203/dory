use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use dory_core::domain::{LangTag, Ms};
use dory_core::models::{Availability, ModelId, Stt};
use dory_core::ports::{CancelToken, EngineError, ModelProvider, ProvisionError, ProvisionSink};
use sha2::{Digest, Sha256};
use sherpa_onnx::OfflineRecognizerResult;

use super::Sherpa;
use super::catalog::{self, Archive, Layout, Model};
use super::engine::{engine_code, recognizer_config, segments};
use super::fetch::hex;

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

fn configured_paths(
    layout: Layout,
    dir: &Path,
    language: Option<&str>,
) -> HashMap<&'static str, Option<String>> {
    let config = recognizer_config(layout, dir, language);
    let m = &config.model_config;
    HashMap::from([
        ("transducer.encoder", m.transducer.encoder.clone()),
        ("transducer.decoder", m.transducer.decoder.clone()),
        ("transducer.joiner", m.transducer.joiner.clone()),
        ("qwen3.conv_frontend", m.qwen3_asr.conv_frontend.clone()),
        ("qwen3.encoder", m.qwen3_asr.encoder.clone()),
        ("qwen3.decoder", m.qwen3_asr.decoder.clone()),
        ("qwen3.tokenizer", m.qwen3_asr.tokenizer.clone()),
        ("whisper.encoder", m.whisper.encoder.clone()),
        ("whisper.decoder", m.whisper.decoder.clone()),
        ("tokens", m.tokens.clone().filter(|t| !t.is_empty())),
    ])
}

#[test]
fn each_layout_configures_the_files_its_archive_holds() {
    let dir = PathBuf::from("/m");
    let at = |f: &str| Some(format!("/m/{f}"));
    let cases: [(Layout, &[(&str, &str)]); 3] = [
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
            Layout::Qwen3,
            &[
                ("qwen3.conv_frontend", "conv_frontend.onnx"),
                ("qwen3.encoder", "encoder.int8.onnx"),
                ("qwen3.decoder", "decoder.int8.onnx"),
                ("qwen3.tokenizer", "tokenizer"),
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
        let got = configured_paths(layout, &dir, None);
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
        let config = recognizer_config(layout, &dir, None);
        assert_eq!(config.model_config.provider.as_deref(), Some("cpu"));
        assert!(config.model_config.num_threads >= 1);
    }
    let parakeet = recognizer_config(Layout::Parakeet, &dir, None);
    assert_eq!(
        parakeet.model_config.model_type.as_deref(),
        Some("nemo_transducer")
    );
    let whisper = recognizer_config(Layout::WhisperTurbo, &dir, Some("tl"));
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
    let tag = |s: &str| LangTag::parse(s).unwrap();
    let cases = [
        ("qwen3-asr-0.6b", "tl", Some("Filipino")),
        ("qwen3-asr-0.6b", "fil", Some("Filipino")),
        ("qwen3-asr-0.6b", "en-US", Some("English")),
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
    }
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

#[test]
fn opening_a_missing_model_fails_without_loading() {
    let dir = tempfile::tempdir().unwrap();
    let provider = sherpa(dir.path(), "http://127.0.0.1:9", model(b"", ""));
    let err = provider.open(&tiny(), &CancelToken::new()).err().unwrap();
    assert!(matches!(err, EngineError::ModelMissing(_)), "{err:?}");
}
