//! The read-side commands: import, list, show, models.

use dory::{Config, LibraryReader};
use dory_core::app::{AppKind, ModelRow};
use dory_core::domain::{Job, JobState, RecordingQuery, RecordingRow, TakeContainer};
use dory_core::models::{Diar, Llm, Stt};
use dory_core::search::{Correction, Found};

use crate::args::{ImportArgs, ListArgs, ModelsArgs, SearchArgs, ShowArgs};
use crate::wait::block_on;
use crate::{open_library, view};

pub fn import(config: &Config, args: &ImportArgs) -> Result<(), String> {
    println!(
        "importing {} into {}",
        args.path.display(),
        config.data_dir.display()
    );
    let report = dory::import_legacy(config, &args.path).map_err(crate::chain)?;
    println!("{report}");
    Ok(())
}

pub fn list(config: &Config, _: &ListArgs) -> Result<(), String> {
    let rows = open_library(config, false)?
        .list(&RecordingQuery::default())
        .map_err(crate::chain)?;
    for row in &rows {
        println!("{}", list_line(row));
    }
    if rows.is_empty() {
        eprintln!("no recordings");
    }
    Ok(())
}

/// Prints a line per correction, then each hit newest first. The library logs `search ms=` on
/// stderr.
pub fn search(config: &Config, args: &SearchArgs) -> Result<(), String> {
    let results = open_library(config, false)?
        .search(&args.text)
        .map_err(crate::chain)?;
    let (hits, corrections) = results.map_or_else(Default::default, |r| (r.hits, r.corrections));
    for correction in &corrections {
        println!("{}", correction_line(correction));
    }
    for found in &hits {
        println!("{}", search_line(found));
    }
    if hits.is_empty() {
        eprintln!("no recordings match {:?}", args.text);
    }
    Ok(())
}

fn correction_line(correction: &Correction) -> String {
    format!(
        "showing results for {} instead of {}",
        correction.used.join(", "),
        correction.typed
    )
}

fn search_line(found: &Found) -> String {
    let r = &found.row.recording;
    format!(
        "{}  {}  {}  {}",
        r.id,
        r.title.as_deref().unwrap_or("(untitled)"),
        view::utc(r.created_at),
        if found.in_title { "title" } else { "text" }
    )
}

fn list_line(row: &RecordingRow) -> String {
    let r = &row.recording;
    let duration = r
        .take
        .map_or_else(|| "-".to_owned(), |t| view::clock(t.duration));
    let status = row
        .job
        .as_ref()
        .map_or_else(|| "idle".to_owned(), view::job_state);
    format!(
        "{}  {}  {}  {}  {}",
        r.id,
        r.title.as_deref().unwrap_or("(untitled)"),
        view::utc(r.created_at),
        duration,
        status
    )
}

pub fn show(config: &Config, args: &ShowArgs) -> Result<(), String> {
    let lib = open_library(config, false)?;
    let recording = lib
        .recording(&args.id)
        .map_err(crate::chain)?
        .ok_or_else(|| format!("no recording {}", args.id))?;

    println!("{}", recording.title.as_deref().unwrap_or("(untitled)"));
    println!("id        {}", recording.id);
    println!("created   {} UTC", view::utc(recording.created_at));
    println!(
        "language  {}",
        match &recording.language {
            dory_core::domain::LanguagePref::Auto => "auto".to_owned(),
            dory_core::domain::LanguagePref::Pinned(tag) => tag.to_string(),
        }
    );
    println!("layout    {}", view::layout(recording.layout));
    match recording.take {
        Some(take) => println!(
            "audio     {} {} Hz {:.1} s",
            container(take.container),
            take.rate_hz,
            take.duration.0 as f64 / 1000.0
        ),
        None => println!("audio     none"),
    }
    let job = view::latest_job(lib.jobs(&args.id).map_err(crate::chain)?);
    println!("status    {}", job_line(job.as_ref()));

    println!();
    match lib.transcript(&args.id).map_err(crate::chain)? {
        Some(transcript) => {
            let names = lib.speaker_names(&args.id).map_err(crate::chain)?;
            println!(
                "Transcript (revision {}, {})",
                transcript.revision.0, transcript.produced_by
            );
            for line in view::transcript_lines(&transcript, &names) {
                println!("{line}");
            }
        }
        None => println!("Transcript: none"),
    }

    println!();
    match lib.summary(&args.id).map_err(crate::chain)? {
        Some(summary) => {
            println!("Summary ({})", summary.produced_by);
            println!("{}", summary.markdown);
        }
        None => println!("Summary: none"),
    }
    Ok(())
}

fn container(container: TakeContainer) -> &'static str {
    match container {
        TakeContainer::Wav => "wav",
        TakeContainer::Flac => "flac",
        TakeContainer::LegacyWebm => "webm (legacy)",
    }
}

fn job_line(job: Option<&Job>) -> String {
    match job {
        None => "idle".into(),
        Some(job) => match &job.state {
            JobState::Failed { error, .. } => {
                format!("{}: {}", view::job_state(&job.state), error.message)
            }
            JobState::Blocked { blocker, .. } => {
                format!(
                    "{}: {}",
                    view::job_state(&job.state),
                    view::blocker(blocker).0
                )
            }
            state => view::job_state(state),
        },
    }
}

pub fn models(config: &Config, args: &ModelsArgs) -> Result<(), String> {
    let lib = open_library(config, args.fake_engines)?;
    print_kind::<Stt>(&lib, "speech to text", args.about)?;
    print_kind::<Llm>(&lib, "summaries", args.about)?;
    print_kind::<Diar>(&lib, "speakers", args.about)?;
    Ok(())
}

fn print_kind<K: AppKind>(lib: &LibraryReader, title: &str, about: bool) -> Result<(), String> {
    println!("{title}");
    let rows: Vec<ModelRow<K>> = block_on(lib.models::<K>()).map_err(crate::chain)?;
    if rows.is_empty() {
        println!("  (none registered)");
    }
    for row in &rows {
        let entry = &row.status.entry;
        println!(
            "{} {}  {}  {}  {}",
            if row.chosen { "*" } else { " " },
            entry.id,
            entry.display_name,
            view::availability(&row.status.availability),
            view::fit(&row.fit),
        );
        if about {
            print!("{}", view::about(&entry.about, entry.disk_bytes));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use dory_core::domain::{
        ChannelLayout, LanguagePref, Ms, Recording, RecordingId, SummaryFlag, TakeInfo, Timestamp,
    };

    use super::*;

    #[test]
    fn list_line_has_id_title_created_duration_status() {
        let row = RecordingRow {
            recording: Recording {
                id: RecordingId::parse("rec_1").unwrap(),
                title: Some("Weekly sync".into()),
                created_at: Timestamp(0),
                language: LanguagePref::Auto,
                layout: ChannelLayout::Mixed,
                take: Some(TakeInfo {
                    container: TakeContainer::Flac,
                    rate_hz: 48_000,
                    duration: Ms(8_100),
                }),
            },
            job: Some(JobState::Succeeded),
            has_transcript: true,
            summary: SummaryFlag::Current,
        };
        assert_eq!(
            list_line(&row),
            "rec_1  Weekly sync  1970-01-01 00:00:00  0:08  done"
        );
        let bare = RecordingRow {
            recording: Recording {
                title: None,
                take: None,
                ..row.recording
            },
            job: None,
            has_transcript: false,
            summary: SummaryFlag::None,
        };
        assert_eq!(
            list_line(&bare),
            "rec_1  (untitled)  1970-01-01 00:00:00  -  idle"
        );
        let correction = Correction {
            typed: "roadmpa".into(),
            used: vec!["roadmap".into(), "roadmaps".into()],
        };
        assert_eq!(
            correction_line(&correction),
            "showing results for roadmap, roadmaps instead of roadmpa"
        );
        let found = Found {
            row: bare,
            in_title: false,
        };
        assert_eq!(
            search_line(&found),
            "rec_1  (untitled)  1970-01-01 00:00:00  text"
        );
    }
}
