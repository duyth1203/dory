//! Prints how a processed recording's transcript groups into turns, tags, accessible labels, and
//! exports, and how long `session::turns` takes. `DORY_DATA_DIR` names the library. Usage:
//! `turns_probe ID [--runs N] [--export]`.

use std::time::Instant;

use dory_core::domain::{RecordingId, SpeakerNames};
use dory_gpui::app::{Detail, TranscriptText, translated_turns, turns};
use dory_gpui::export::{ExportFormat, render};
use dory_gpui::transcript_view::spoken_turn;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let id = RecordingId::parse(&args[0]).expect("recording id");
    let runs: usize = args
        .iter()
        .position(|a| a == "--runs")
        .map_or(1, |i| args[i + 1].parse().unwrap());
    let export = args.iter().any(|a| a == "--export");
    let dir = std::env::var_os("DORY_DATA_DIR").expect("DORY_DATA_DIR");
    let library = dory::open_library(&dory::Config::new(dir)).expect("open library");
    let recording = library.recording(&id).unwrap().expect("recording");
    let transcript = library.transcript(&id).unwrap().expect("transcript");
    let names: SpeakerNames = library.speaker_names(&id).unwrap();

    let mut times = Vec::new();
    let mut grouped = Vec::new();
    for _ in 0..runs {
        let started = Instant::now();
        grouped = turns(&transcript, &names);
        times.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    println!(
        "segments={} turns={} bilingual={} shares={:?} detected={:?}",
        transcript.segments.len(),
        grouped.len(),
        transcript.is_bilingual(),
        transcript
            .language_shares()
            .iter()
            .map(|(t, f)| format!("{t}:{:.3}", f.get()))
            .collect::<Vec<_>>(),
        transcript
            .detected_languages
            .iter()
            .map(|t| t.as_str())
            .collect::<Vec<_>>()
    );
    println!(
        "turns_ms runs={runs} min={:.4} median={:.4} max={:.4}",
        times[0],
        times[times.len() / 2],
        times[times.len() - 1]
    );
    if runs == 1 || args.iter().any(|a| a == "--list") {
        for (ix, turn) in grouped.iter().enumerate() {
            let text: String = turn.text.chars().take(60).collect();
            println!(
                "turn {ix}: lang={:?} tag={:?} | {} | {text}",
                turn.language.as_ref().map(|l| l.as_str()),
                turn.tag.as_deref(),
                spoken_turn(turn).chars().take(70).collect::<String>()
            );
        }
    }
    if let Some(translation) = library
        .translation(&id)
        .unwrap()
        .filter(|t| t.applies_to(&transcript) && t.has_text())
    {
        println!(
            "translation target={} texts={:?}",
            translation.target,
            translation
                .texts
                .iter()
                .map(Option::is_some)
                .collect::<Vec<_>>()
        );
        for (ix, turn) in translated_turns(&transcript, &names, &translation)
            .iter()
            .enumerate()
        {
            let text: String = turn.text.chars().take(60).collect();
            println!(
                "translated turn {ix}: tag={:?} from={:?} | {} | {text}",
                turn.tag.as_deref(),
                turn.translated_from
                    .as_ref()
                    .map(|f| f.iter().map(|l| l.as_str()).collect::<Vec<_>>()),
                spoken_turn(turn).chars().take(80).collect::<String>()
            );
        }
    }
    if export {
        let detail = Detail {
            recording,
            transcript: Some(transcript),
            summary: None,
            names,
            turns: grouped,
            translation: None,
            translated_turns: None,
        };
        for format in [ExportFormat::Markdown, ExportFormat::Srt] {
            println!("---- {format:?}");
            print!(
                "{}",
                render(format, &detail, TranscriptText::Spoken).unwrap()
            );
        }
    }
}
