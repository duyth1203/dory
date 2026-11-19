//! `eval` runs case folders through the real pipeline and scores what comes out, so a change to
//! the speech model, its settings, or the summary prompt can be compared with a measured baseline.
//!
//! Each case is imported once as a recording titled `eval: <case>` and reused on later runs, so
//! the data folder holds one recording per case however often it runs.

mod case;
mod report;
mod score;

use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use dory::Config;
use dory_core::app::{Dory, Rerun, Subscription};
use dory_core::domain::{LanguagePref, Processing, RecordingId, RecordingQuery, Stage, Timestamp};

use crate::args::EvalArgs;
use crate::process::{StageTimes, follow};
use crate::{chain, view};
use case::Case;
use report::Outcome;

pub fn run(config: &Config, args: &EvalArgs) -> Result<(), String> {
    crate::needs_data_dir("eval imports its audio into the library")?;
    let cases = case::discover(&args.cases)?
        .iter()
        .map(|dir| Case::load(dir))
        .collect::<Result<Vec<_>, _>>()?;
    // Pace changes threads, not words, so a fixed pace keeps stt_secs comparable across runs.
    let config = Config {
        processing: Some(Processing::Fast),
        ..config.clone()
    };
    let dory = crate::open(&config, args.fake_engines)?;
    let events = dory.events();

    let mut outcomes = Vec::new();
    let mut failed = 0;
    for case in &cases {
        println!("case {}", case.name);
        match run_case(&dory, &events, case, args) {
            Ok(outcome) => {
                println!("{}", report::case_line(&outcome));
                for term in &outcome.absent_terms {
                    eprintln!(
                        "  term {term:?} is not in {}'s reference.txt, so it was not scored",
                        case.name
                    );
                }
                if let Some(tsv) = &args.tsv {
                    append_row(tsv, &args.label, &outcome)?;
                }
                outcomes.push(outcome);
            }
            Err(message) => {
                failed += 1;
                eprintln!("case {} failed: {message}", case.name);
            }
        }
    }
    if !outcomes.is_empty() {
        for line in report::totals(&outcomes) {
            println!("{line}");
        }
    }
    match failed {
        0 => Ok(()),
        n => Err(format!("{n} of {} cases failed", cases.len())),
    }
}

fn run_case(
    dory: &Dory,
    events: &Subscription,
    case: &Case,
    args: &EvalArgs,
) -> Result<Outcome, String> {
    let library = dory.library();
    let title = format!("eval: {}", case.name);
    let existing = library
        .list(&RecordingQuery::default())
        .map_err(chain)?
        .into_iter()
        .find(|row| row.recording.title.as_deref() == Some(title.as_str()));
    let id = match existing {
        Some(row) => row.recording.id,
        None if args.summary_only => {
            return Err("it has no recording yet, so run eval without --summary-only first".into());
        }
        None => library
            .import_audio(&case.audio, Some(title))
            .map_err(chain)?,
    };
    let recording = library
        .recording(&id)
        .map_err(chain)?
        .ok_or("the recording disappeared")?;
    if args.summary_only && library.transcript(&id).map_err(chain)?.is_none() {
        return Err("it has no transcript to summarize, so run eval without --summary-only".into());
    }

    let (mut stt_secs, mut llm_secs) = (None, None);
    if !args.summary_only {
        let language = match &case.language {
            Some(tag) => LanguagePref::Pinned(tag.clone()),
            None => recording.language.clone(),
        };
        library
            .rerun(
                &id,
                Rerun::Transcribe {
                    language,
                    stt: args.stt.clone(),
                },
            )
            .map_err(chain)?;
        let times = follow_newest(dory, events, &id)?;
        stt_secs = times.secs(Stage::Transcribe);
        llm_secs = times.secs(Stage::Summarize);
    }
    // The transcribe run already wrote a summary with the Settings model. A picked model writes
    // a second one over it, and that is the one scored.
    if args.summary_only || args.llm.is_some() {
        library
            .rerun(
                &id,
                Rerun::Summarize {
                    llm: args.llm.clone(),
                },
            )
            .map_err(chain)?;
        llm_secs = follow_newest(dory, events, &id)?.secs(Stage::Summarize);
    }

    let transcript = library
        .transcript(&id)
        .map_err(chain)?
        .ok_or("the job finished without a transcript")?;
    let spoken = transcript
        .segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let reference = case.reference.as_deref().map(score::words);
    let hypothesis = score::words(&spoken);
    let (terms, absent_terms) = score::term_hits(&case.terms, reference.as_deref(), &hypothesis);

    let summary = library.summary(&id).map_err(chain)?;
    let scored = match &summary {
        Some(summary) if !case.expected.is_empty() => {
            Some(score::score_summary(&summary.markdown, &case.expected))
        }
        _ => None,
    };
    Ok(Outcome {
        case: case.name.clone(),
        stt: transcript.produced_by.to_string(),
        llm: summary.map(|s| s.produced_by.to_string()),
        wer: reference.map(|r| score::word_errors(&r, &hypothesis)),
        terms,
        absent_terms,
        summary: scored,
        stt_secs,
        llm_secs,
        audio_secs: recording
            .take
            .map_or(0.0, |take| take.duration.0 as f64 / 1000.0),
    })
}

fn follow_newest(
    dory: &Dory,
    events: &Subscription,
    id: &RecordingId,
) -> Result<StageTimes, String> {
    let job = view::latest_job(dory.library().jobs(id).map_err(chain)?)
        .ok_or("the job was not queued")?;
    follow(dory, events, job)
}

/// Appends one row, writing the header first when the file is new or empty.
fn append_row(path: &Path, label: &str, outcome: &Outcome) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("{}: {e}", path.display());
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(fail)?;
    if file.metadata().map_err(fail)?.len() == 0 {
        writeln!(file, "{}", report::TSV_HEADER).map_err(fail)?;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    let row = report::tsv_row(&view::utc(Timestamp(now)), label, outcome);
    writeln!(file, "{row}").map_err(fail)
}
