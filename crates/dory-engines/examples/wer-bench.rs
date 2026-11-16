//! Word error rate of every installed sherpa-onnx model on a folder of labelled clips.
//!
//! `cargo run --release -p dory-engines --example wer-bench -- <models dir> <clips dir> [lang]`
//!
//! `<clips dir>` holds 16 kHz mono 16-bit WAVs and a `refs.tsv` of `file<TAB>reference`, such as a
//! FLEURS test split. `lang` pins the language, for example `tl`; without it each model detects.
//! Text is lowercased and stripped of punctuation on both sides before words are compared.

use std::path::{Path, PathBuf};
use std::time::Instant;

use dory_core::domain::{Channel, Fraction, LangTag, LanguagePref, Ms};
use dory_core::models::Availability;
use dory_core::ports::{
    Activity, CancelToken, Clip, EngineError, ModelProvider, RawSegment, SegmentSink,
    TranscribeRequest,
};
use dory_engines::Sherpa;

#[derive(Default)]
struct Collect(Vec<RawSegment>);

impl SegmentSink for Collect {
    fn progress(&mut self, _: Fraction) {}
    fn segment(&mut self, segment: RawSegment) {
        self.0.push(segment);
    }
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

fn wav_secs(path: &Path) -> f32 {
    let bytes = std::fs::metadata(path).map_or(0, |m| m.len());
    bytes.saturating_sub(44) as f32 / 32_000.0
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [models, clips, rest @ ..] = args.as_slice() else {
        eprintln!("usage: wer-bench <models dir> <clips dir> [lang]");
        std::process::exit(2);
    };
    let language = rest.first().map_or(LanguagePref::Auto, |tag| {
        LanguagePref::Pinned(LangTag::parse(tag).expect("a BCP 47 tag"))
    });
    let clips = PathBuf::from(clips);
    let refs: Vec<(PathBuf, Vec<String>)> = std::fs::read_to_string(clips.join("refs.tsv"))
        .expect("refs.tsv")
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(file, text)| (clips.join(file), words(text)))
        .collect();
    let sherpa = Sherpa::new(models, Sherpa::RELEASES);
    let cancel = CancelToken::new();

    println!("model\tclips\tref_words\terrors\twer\trtfx");
    for entry in sherpa.catalog() {
        if sherpa.availability(&entry.id) != Availability::Ready {
            eprintln!("{}: not installed", entry.id);
            continue;
        }
        let engine = sherpa.open(&entry.id, &cancel).expect("model opens");
        let (mut errors, mut total, mut audio, mut busy, mut done) = (0, 0, 0.0f32, 0.0f32, 0);
        for (path, reference) in &refs {
            let clip = Clip::new(
                path.clone(),
                Channel::Mixed,
                Ms(0),
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
            };
            let mut sink = Collect::default();
            let started = Instant::now();
            match engine.transcribe(&request, &mut sink, &cancel) {
                Ok(_) => {}
                Err(EngineError::UnsupportedLanguage(_)) => break,
                Err(other) => panic!("{}: {other}", entry.id),
            }
            busy += started.elapsed().as_secs_f32();
            audio += wav_secs(path);
            let hypothesis: Vec<String> = sink.0.iter().flat_map(|s| words(&s.text)).collect();
            errors += edit_distance(reference, &hypothesis);
            total += reference.len();
            done += 1;
            if std::env::var_os("WER_VERBOSE").is_some() {
                eprintln!(
                    "{}\t{}\n  ref: {}\n  hyp: {}",
                    entry.id,
                    path.display(),
                    reference.join(" "),
                    hypothesis.join(" ")
                );
            }
        }
        if done == 0 {
            println!("{}\t0\t-\t-\tunsupported\t-", entry.id);
            continue;
        }
        println!(
            "{}\t{done}\t{total}\t{errors}\t{:.1}%\t{:.1}",
            entry.id,
            100.0 * errors as f32 / total as f32,
            audio / busy
        );
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
