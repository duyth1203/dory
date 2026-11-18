//! Word error rate of every Ready speech model the app registers, on a folder of labelled clips.
//!
//! `DORY_DATA_DIR=<library> cargo run --release -p dory --example wer-bench -- [--model ID]...
//! [--expect TAG]... <clips dir> [lang]`
//!
//! `<clips dir>` holds 16 kHz mono 16-bit WAVs and a `refs.tsv` of `file<TAB>reference`, such as a
//! FLEURS test split. Models come from `dory::speech_providers`, the list `dory::open` registers,
//! with sherpa models read from the library's `models/` folder. `--model` picks rows; without it
//! every Ready row runs. `lang` pins the language, for example `tl`; without it each model detects.
//! `--expect` names the meeting languages, as `dory-cli languages` does, so a model that runs one
//! locale takes `Auto` the way the app runs it. A row that does not fit is reported, not run.
//! Text is lowercased and stripped of punctuation on both sides before words are compared.

use std::path::{Path, PathBuf};
use std::time::Instant;

use dory::Config;
use dory_core::domain::{Channel, Fraction, LangTag, LanguagePref, Ms};
use dory_core::models::{
    Availability, DiarizationNeed, Fit, ModelKind, Stt, SttNeeds, run_language,
};
use dory_core::ports::{
    Activity, CancelToken, Clip, EngineError, RawSegment, SegmentSink, TranscribeRequest,
};

#[derive(Default)]
struct Collect(Vec<RawSegment>);

impl SegmentSink for Collect {
    fn progress(&mut self, _: Fraction) {}
    fn segment(&mut self, segment: RawSegment) {
        self.0.push(segment);
    }
}

struct Args {
    models: Vec<String>,
    needs: SttNeeds,
    clips: PathBuf,
}

const USAGE: &str = "usage: wer-bench [--model ID]... [--expect TAG]... <clips dir> [lang]";

fn tag(text: &str) -> LangTag {
    LangTag::parse(text).unwrap_or_else(|_| panic!("{text} is not a BCP 47 tag"))
}

fn parse(args: &[String]) -> Option<Args> {
    let (mut models, mut expected, mut positional) = (Vec::new(), Vec::new(), Vec::new());
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--model" => models.push(it.next()?.clone()),
            "--expect" => expected.push(tag(it.next()?)),
            _ => positional.push(arg),
        }
    }
    let (clips, language) = match positional.as_slice() {
        [clips] => (clips, LanguagePref::Auto),
        [clips, lang] => (clips, LanguagePref::Pinned(tag(lang))),
        _ => return None,
    };
    Some(Args {
        models,
        needs: SttNeeds {
            language,
            expected,
            expected_guessed: false,
            diarization: DiarizationNeed::NotNeeded,
            input: Ms(0),
        },
        clips: PathBuf::from(clips),
    })
}

fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '\'' {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

fn edit_distance(reference: &[String], hypothesis: &[String]) -> usize {
    let mut row: Vec<usize> = (0..=hypothesis.len()).collect();
    for (i, r) in reference.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, h) in hypothesis.iter().enumerate() {
            let substitution = diagonal + usize::from(r != h);
            diagonal = row[j + 1];
            row[j + 1] = substitution.min(row[j] + 1).min(row[j + 1] + 1);
        }
    }
    row[hypothesis.len()]
}

/// The length of a 16 kHz mono 16-bit WAV, from its `data` chunk, since ffmpeg writes a `LIST`
/// chunk before it.
fn wav_ms(path: &Path) -> Ms {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().expect("4 bytes")) as u64;
        if &bytes[at..at + 4] == b"data" {
            return Ms(size * 1000 / 32_000);
        }
        at += 8 + size as usize + (size as usize & 1);
    }
    panic!("{} has no data chunk", path.display())
}

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let Some(args) = parse(&raw) else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    let config = Config::from_env().expect("the data folder from DORY_DATA_DIR");
    let refs: Vec<(PathBuf, Vec<String>)> = std::fs::read_to_string(args.clips.join("refs.tsv"))
        .expect("refs.tsv")
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(file, text)| (args.clips.join(file), words(text)))
        .collect();
    let cancel = CancelToken::new();
    let verbose = std::env::var_os("WER_VERBOSE").is_some();

    println!("model\tclips\tref_words\terrors\twer\trtfx\tempty");
    for provider in dory::speech_providers(&config) {
        for entry in provider.catalog() {
            let id = entry.id.to_string();
            if !args.models.is_empty() && !args.models.contains(&id) {
                continue;
            }
            if provider.availability(&entry.id) != Availability::Ready {
                eprintln!("{id}: not ready");
                continue;
            }
            if let Fit::No(why) = Stt::fit(&entry.caps, &args.needs) {
                println!("{id}\t0\t-\t-\tdoes not fit: {why:?}\t-\t-");
                continue;
            }
            let language = run_language(&entry.caps, &args.needs);
            let engine = provider.open(&entry.id, &cancel).expect("model opens");
            let (mut errors, mut total, mut audio, mut busy) = (0, 0, 0u64, 0.0f64);
            let (mut done, mut empty) = (0, 0);
            for (path, reference) in &refs {
                let duration = wav_ms(path);
                let clip = Clip::new(
                    path.clone(),
                    Channel::Mixed,
                    duration,
                    Ms(0),
                    Activity::Unmeasured,
                    None,
                );
                let request = TranscribeRequest {
                    clip: &clip,
                    language: language.clone(),
                    diarize: false,
                    hint: None,
                    threads: 4,
                    sentence_times: false,
                    locale: None,
                };
                let mut sink = Collect::default();
                let started = Instant::now();
                match engine.transcribe(&request, &mut sink, &cancel) {
                    Ok(_) => {}
                    Err(EngineError::UnsupportedLanguage(_)) => break,
                    Err(other) => panic!("{id}: {other}"),
                }
                busy += started.elapsed().as_secs_f64();
                audio += duration.0;
                let hypothesis: Vec<String> = sink.0.iter().flat_map(|s| words(&s.text)).collect();
                errors += edit_distance(reference, &hypothesis);
                total += reference.len();
                done += 1;
                empty += usize::from(hypothesis.is_empty());
                if verbose {
                    eprintln!(
                        "{id}\t{}\n  ref: {}\n  hyp: {}",
                        path.display(),
                        reference.join(" "),
                        hypothesis.join(" ")
                    );
                }
            }
            if done == 0 {
                println!("{id}\t0\t-\t-\tunsupported\t-\t-");
                continue;
            }
            println!(
                "{id}\t{done}\t{total}\t{errors}\t{:.1}%\t{:.1}\t{empty}",
                100.0 * errors as f64 / total as f64,
                audio as f64 / 1000.0 / busy
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_word_edits_after_normalizing() {
        let r = words("Nagganap ang pagbagsak, sa mataas!");
        assert_eq!(
            edit_distance(&r, &words("nagganap ang pagbagsak sa mataas")),
            0
        );
        assert_eq!(
            edit_distance(&r, &words("nagganap pagbagsak sa mataas na")),
            2
        );
        assert_eq!(edit_distance(&r, &[]), 5);
    }
}
