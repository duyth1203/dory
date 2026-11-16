//! `process`: queues the pipeline for a recording and follows it to the end.

use std::time::Duration;

use dory::Config;
use dory_core::app::Event;
use dory_core::domain::{Goal, JobState};

use crate::args::ProcessArgs;
use crate::{open, view};

const TICK: Duration = Duration::from_millis(25);

pub fn run(config: &Config, args: &ProcessArgs) -> Result<(), String> {
    let config = Config {
        processing: args.pace,
        ..config.clone()
    };
    let dory = open(&config, args.fake_engines)?;
    if dory
        .library()
        .recording(&args.id)
        .map_err(crate::chain)?
        .is_none()
    {
        return Err(format!("no recording {}", args.id));
    }
    let events = dory.events();
    dory.pipeline()
        .enqueue(
            &args.id,
            if args.summary_only {
                Goal::SUMMARIZE
            } else {
                Goal::ALL
            },
        )
        .map_err(crate::chain)?;
    let mut job = view::latest_job(dory.library().jobs(&args.id).map_err(crate::chain)?)
        .ok_or("the job was not queued")?;
    if matches!(job.state, JobState::Blocked { .. }) {
        // An earlier run blocked it. This run's engines may be what it was waiting for.
        println!("job {} was blocked, trying it again", job.id.0);
        dory.pipeline().unblock(job.id).map_err(crate::chain)?;
    }

    let mut shown = None;
    let mut last_progress = None;
    let mut last_budget = None;
    loop {
        while let Some(event) = events.try_recv() {
            match event {
                Event::StageProgress {
                    stage,
                    fraction,
                    budget,
                    ..
                } => {
                    if last_budget != Some(budget) {
                        last_budget = Some(budget);
                        println!("  pace {}", view::budget(budget));
                    }
                    let tenth = (fraction.get() * 10.0) as u32 * 10;
                    if last_progress != Some((stage, tenth)) {
                        last_progress = Some((stage, tenth));
                        println!("  {} {tenth}%", view::stage(stage));
                    }
                }
                Event::Error { context, message } => eprintln!("  error {context}: {message}"),
                _ => {}
            }
        }
        job = dory
            .pipeline()
            .job(job.id)
            .map_err(crate::chain)?
            .ok_or("the job disappeared")?;
        if shown.as_ref() != Some(&job.state) {
            println!("job {} {}", job.id.0, view::job_state(&job.state));
            shown = Some(job.state.clone());
        }
        match &job.state {
            JobState::Succeeded => break,
            JobState::Blocked { stage, blocker } => {
                let (what, fix) = view::blocker(blocker);
                return Err(format!(
                    "blocked at {}: {what}\nfix: {fix}",
                    view::stage(*stage)
                ));
            }
            JobState::Failed { stage, error } => {
                return Err(format!(
                    "failed at {}: {}",
                    view::stage(*stage),
                    error.message
                ));
            }
            JobState::Cancelled => return Err("the job was cancelled".into()),
            JobState::Queued | JobState::Running { .. } => std::thread::sleep(TICK),
        }
    }

    let lib = dory.library();
    let segments = lib
        .transcript(&args.id)
        .map_err(crate::chain)?
        .map_or(0, |t| t.segments.len());
    let summary = lib.summary(&args.id).map_err(crate::chain)?;
    println!(
        "{} transcript segments, summary {}",
        segments,
        summary.map_or("none".into(), |s| format!("{} chars", s.markdown.len()))
    );
    Ok(())
}
