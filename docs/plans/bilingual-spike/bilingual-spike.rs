//! Throwaway spike for docs/plans/bilingual.md. Never merges.
//!
//! `bilingual-spike <whisper dir> <clips dir> <out.jsonl> [chunk_secs] [max_clips]`
//!
//! For each WAV in `<clips dir>/refs.tsv`, writes one JSON line with Whisper turbo's text on auto
//! (with the language Whisper detected), pinned `tl`, and pinned `en`. It also cuts the clip at
//! pauses into chunks of at most `chunk_secs` (default 6) and decodes each chunk on auto and on
//! pinned `tl`.
//!
//! sherpa-onnx 2.0.1's standalone language identifier feeds 80 mel bins and fails on turbo, which
//! wants 128, so the detected language comes from the recognizer's own JSON result. The Rust
//! wrapper's `OfflineRecognizerResult` drops that `lang` field, hence the raw call below.

use std::ffi::CStr;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineStream, OfflineWhisperModelConfig, Wave};

const RATE: i32 = 16_000;

fn recognizer(dir: &Path, language: Option<&str>) -> OfflineRecognizer {
    let path = |f: &str| Some(dir.join(f).to_string_lossy().into_owned());
    let mut config = OfflineRecognizerConfig::default();
    config.model_config.whisper = OfflineWhisperModelConfig {
        encoder: path("turbo-encoder.int8.onnx"),
        decoder: path("turbo-decoder.int8.onnx"),
        language: language.map(str::to_owned),
        task: Some("transcribe".into()),
        tail_paddings: -1,
        enable_segment_timestamps: std::env::var_os("TS_PROBE").is_some() || std::env::var_os("COVER").is_some(),
        enable_token_timestamps: std::env::var_os("TS_PROBE").is_some(),
        ..Default::default()
    };
    config.model_config.tokens = path("turbo-tokens.txt");
    config.model_config.num_threads = 4;
    config.model_config.provider = Some("cpu".into());
    OfflineRecognizer::create(&config).expect("whisper loads")
}

fn raw_json(stream: &OfflineStream) -> serde_json::Value {
    // SAFETY: spike only. `OfflineStream` is a single raw-pointer field.
    let ptr: *const sherpa_onnx_sys::OfflineStream =
        unsafe { *(stream as *const OfflineStream as *const *const sherpa_onnx_sys::OfflineStream) };
    unsafe {
        let c = sherpa_onnx_sys::SherpaOnnxGetOfflineStreamResultAsJson(ptr);
        let s = CStr::from_ptr(c).to_string_lossy().into_owned();
        sherpa_onnx_sys::SherpaOnnxDestroyOfflineStreamResultJson(c);
        serde_json::from_str(&s).unwrap_or(serde_json::Value::Null)
    }
}

struct Decoded {
    text: String,
    lang: String,
    secs: f32,
}

fn decode(r: &OfflineRecognizer, samples: &[f32]) -> Decoded {
    let started = Instant::now();
    let stream = r.create_stream();
    stream.accept_waveform(RATE, samples);
    r.decode(&stream);
    let json = raw_json(&stream);
    Decoded {
        text: json["text"].as_str().unwrap_or("").trim().to_owned(),
        lang: json["lang"].as_str().unwrap_or("").to_owned(),
        secs: started.elapsed().as_secs_f32(),
    }
}

/// Cuts at the quietest 20 ms frame inside each stretch once it passes half of `max`, so a chunk
/// ends at a pause when one exists and never runs past `max`.
fn chunks(samples: &[f32], max_secs: f32) -> Vec<(usize, usize)> {
    let frame = (RATE / 50) as usize;
    let max = (max_secs * RATE as f32) as usize;
    let energy = |a: usize| {
        let b = (a + frame).min(samples.len());
        samples[a..b].iter().map(|x| x * x).sum::<f32>() / (b - a).max(1) as f32
    };
    let mut out = Vec::new();
    let mut start = 0;
    while samples.len() - start > max {
        let lo = start + max / 2;
        let hi = start + max - frame;
        let cut = (lo..hi)
            .step_by(frame)
            .min_by(|a, b| energy(*a).total_cmp(&energy(*b)))
            .unwrap_or(hi);
        out.push((start, cut));
        start = cut;
    }
    out.push((start, samples.len()));
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [dir, clips, out, rest @ ..] = args.as_slice() else {
        eprintln!("usage: bilingual-spike <whisper dir> <clips dir> <out.jsonl> [chunk_secs] [max_clips]");
        std::process::exit(2);
    };
    let chunk_secs: f32 = rest.first().map_or(6.0, |s| s.parse().expect("seconds"));
    let max_clips: usize = rest.get(1).map_or(usize::MAX, |s| s.parse().expect("count"));
    let dir = PathBuf::from(dir);
    let clips = PathBuf::from(clips);
    let files: Vec<String> = std::fs::read_to_string(clips.join("refs.tsv"))
        .expect("refs.tsv")
        .lines()
        .filter_map(|l| l.split_once('\t').map(|(f, _)| f.to_owned()))
        .take(max_clips)
        .collect();

    if std::env::var_os("COVER").is_some() {
        let r = recognizer(&dir, None);
        let mut out = std::fs::File::create(out).expect("out file");
        for file in files.iter() {
            let wave = Wave::read(clips.join(file).to_str().unwrap()).unwrap();
            let samples = wave.samples();
            let secs = samples.len() as f32 / RATE as f32;
            let started = Instant::now();
            let decode_ts = |piece: &[f32]| {
                let stream = r.create_stream();
                stream.accept_waveform(RATE, piece);
                r.decode(&stream);
                raw_json(&stream)
            };
            let first = decode_ts(samples);
            let mut parts: Vec<(f32, f32, String, String)> = Vec::new();
            let lang = first["lang"].as_str().unwrap_or("").to_owned();
            if let (Some(ts), Some(du), Some(tx)) = (first["segment_timestamps"].as_array(), first["segment_durations"].as_array(), first["segment_texts"].as_array()) {
                for ((t, d), x) in ts.iter().zip(du).zip(tx) {
                    let t = t.as_f64().unwrap_or(0.0) as f32;
                    let d = d.as_f64().unwrap_or(0.0) as f32;
                    parts.push((t, t + d, x.as_str().unwrap_or("").trim().to_owned(), lang.clone()));
                }
            }
            let mut gaps = Vec::new();
            let mut at = 0.0f32;
            for (a, b, _, _) in &parts {
                if *a - at >= 2.0 { gaps.push((at, *a)); }
                at = at.max(*b);
            }
            if secs - at >= 2.0 { gaps.push((at, secs)); }
            let mut redone = Vec::new();
            for (a, b) in &gaps {
                let s0 = ((a * RATE as f32) as usize).min(samples.len());
                let s1 = ((b * RATE as f32) as usize).min(samples.len());
                let j = decode_ts(&samples[s0..s1]);
                let text = j["text"].as_str().unwrap_or("").trim().to_owned();
                let l = j["lang"].as_str().unwrap_or("").to_owned();
                if !text.is_empty() { parts.push((*a, *b, text, l.clone())); }
                redone.push(serde_json::json!({"start": a, "end": b, "lang": l}));
            }
            parts.sort_by(|x, y| x.0.total_cmp(&y.0));
            let text = parts.iter().map(|p| p.2.as_str()).collect::<Vec<_>>().join(" ");
            let row = serde_json::json!({"file": file, "secs": secs, "lid": lang, "first": first["text"], "cover": text,
                "cover_s": started.elapsed().as_secs_f32(), "gaps": redone,
                "langs": parts.iter().map(|p| p.3.clone()).collect::<Vec<_>>()});
            writeln!(out, "{row}").unwrap();
            eprintln!("{file} gaps={}", gaps.len());
        }
        return;
    }
    if std::env::var_os("TS_PROBE").is_some() {
        let r = recognizer(&dir, None);
        for file in files.iter() {
            let wave = Wave::read(clips.join(file).to_str().unwrap()).unwrap();
            let stream = r.create_stream();
            stream.accept_waveform(RATE, wave.samples());
            r.decode(&stream);
            let mut j = raw_json(&stream);
            if let Some(o) = j.as_object_mut() { o.remove("tokens"); o.remove("ys_log_probs"); o.remove("words"); }
            println!("{file} {j}");
        }
        return;
    }
    let auto = recognizer(&dir, None);
    let tl = recognizer(&dir, Some("tl"));
    let en = recognizer(&dir, Some("en"));
    let mut out = std::fs::File::create(out).expect("out file");

    for file in files {
        let wave = Wave::read(clips.join(&file).to_str().expect("utf-8")).expect("a wav");
        assert_eq!(wave.sample_rate(), RATE);
        let samples = wave.samples();
        let secs = samples.len() as f32 / RATE as f32;
        let a = decode(&auto, samples);
        let t = decode(&tl, samples);
        let e = decode(&en, samples);
        let mut row = serde_json::json!({
            "file": file, "secs": secs,
            "lid": a.lang, "auto": a.text, "auto_s": a.secs,
            "tl": t.text, "tl_s": t.secs, "en": e.text, "en_s": e.secs,
        });
        let mut parts = Vec::new();
        for (s, f) in chunks(samples, chunk_secs) {
            let piece = &samples[s..f];
            let a = decode(&auto, piece);
            let t = decode(&tl, piece);
            parts.push(serde_json::json!({
                "start": s as f32 / RATE as f32, "end": f as f32 / RATE as f32,
                "lid": a.lang, "auto": a.text, "auto_s": a.secs, "tl": t.text, "tl_s": t.secs,
            }));
        }
        row["chunks"] = parts.into();
        writeln!(out, "{row}").expect("write");
        eprintln!("{file} {secs:.1}s lid={} auto={:.1}s tl={:.1}s", row["lid"], a.secs, t.secs);
    }
}
