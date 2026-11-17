//! Live Apple Intelligence checks. They need a Mac with Apple Intelligence on, so they are ignored
//! by default: `DORY_APPLE_LIVE=1 cargo test -p dory-macos --features apple-intelligence --test
//! foundation_models_live -- --ignored`.
#![cfg(target_os = "macos")]

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use dory_core::ports::CancelToken;
use dory_macos::ai::foundation_models::{self, Availability, FailureKind};

const SYSTEM: &str = "You summarize meeting transcripts. Reply with a short summary.";
const WEEKLY: &str = "[00:00] Speaker 1: Let's start the weekly sync.\n\
[00:03] Speaker 1: The beta build is ready, and we ship it to testers on Friday.\n\
[00:10] Speaker 2: I finished the login fix yesterday.\n\
[00:14] Speaker 1: Maria reviews the invoices on Monday.\n";

fn live() -> bool {
    let on = std::env::var("DORY_APPLE_LIVE").as_deref() == Ok("1");
    if !on {
        eprintln!("skipped: set DORY_APPLE_LIVE=1 to call Apple Intelligence");
    }
    on
}

#[test]
#[ignore = "calls Apple Intelligence; run with DORY_APPLE_LIVE=1 and --ignored"]
fn a_short_summary_round_trips() {
    if !live() {
        return;
    }
    let started = Instant::now();
    let info = foundation_models::info();
    let info_ms = started.elapsed().as_millis();
    let Availability::Available {
        context_size,
        languages,
    } = info
    else {
        panic!("Apple Intelligence is not available here: {info:?}");
    };
    assert!(context_size >= 4096, "context {context_size}");
    assert!(
        languages.iter().any(|l| l == "en"),
        "languages {languages:?}"
    );

    let mut streamed = String::new();
    let started = Instant::now();
    let text = foundation_models::complete(
        SYSTEM,
        WEEKLY,
        256,
        0.2,
        |piece| {
            streamed.push_str(piece);
            true
        },
        &CancelToken::new(),
    )
    .expect("completion");
    let complete_ms = started.elapsed().as_millis();
    eprintln!(
        "info_ms={info_ms} context={context_size} languages={} complete_ms={complete_ms} \
         chars={} text={text:?}",
        languages.len(),
        text.len()
    );
    assert!(!text.trim().is_empty());
    assert_eq!(streamed, text, "the pieces add up to the full text");
}

#[test]
#[ignore = "calls Apple Intelligence; run with DORY_APPLE_LIVE=1 and --ignored"]
fn cancel_stops_a_completion_within_two_seconds() {
    if !live() {
        return;
    }
    let long_input = WEEKLY.repeat(20);
    let token = CancelToken::new();
    let (first_piece, saw_piece) = mpsc::channel();
    let worker = {
        let token = token.clone();
        thread::spawn(move || {
            let mut first_piece = Some(first_piece);
            let result = foundation_models::complete(
                "Rewrite the transcript in full, line by line, in formal English.",
                &long_input,
                2048,
                0.2,
                |_| {
                    if let Some(tx) = first_piece.take() {
                        let _ = tx.send(());
                    }
                    true
                },
                &token,
            );
            (result, Instant::now())
        })
    };
    saw_piece
        .recv_timeout(Duration::from_secs(60))
        .expect("Apple started streaming");
    let cancelled_at = Instant::now();
    token.cancel();
    let (result, returned_at) = worker.join().expect("worker");
    let latency = returned_at.duration_since(cancelled_at);
    eprintln!("cancel_latency_ms={}", latency.as_millis());
    assert_eq!(result.expect_err("cancelled").kind, FailureKind::Cancelled);
    assert!(latency < Duration::from_secs(2), "took {latency:?}");
}
