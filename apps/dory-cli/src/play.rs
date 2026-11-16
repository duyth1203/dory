//! `play`: plays a recording's audio to the default output and prints the position about once a
//! second. Enter or Ctrl-C stops, and so does a closed stdin.

use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dory::Config;
use dory_core::ports::PlayState;

use crate::args::PlayArgs;
use crate::view;

const TICK: Duration = Duration::from_millis(20);
const REPORT_EVERY: Duration = Duration::from_secs(1);

pub fn run(config: &Config, args: &PlayArgs) -> Result<(), String> {
    let dory = crate::open(config, false)?;
    let length = dory
        .library()
        .recording(&args.id)
        .map_err(crate::chain)?
        .and_then(|r| r.take)
        .map(|take| take.duration);
    if length.is_some_and(|length| args.seek >= length) {
        return Err(format!(
            "--seek is at or past the end of the recording ({})",
            view::clock(length.unwrap_or(args.seek))
        ));
    }
    let player = dory.library().player(&args.id).map_err(crate::chain)?;

    let (stop_tx, stop) = mpsc::channel::<()>();
    let on_interrupt = stop_tx.clone();
    ctrlc::set_handler(move || {
        let _ = on_interrupt.send(());
    })
    .map_err(|e| e.to_string())?;
    let _ = std::thread::Builder::new()
        .name("dory-cli-stdin".into())
        .spawn(move || {
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            let _ = stop_tx.send(());
        });

    if args.seek.0 > 0 {
        player.seek(args.seek).map_err(crate::chain)?;
    }
    player.play().map_err(crate::chain)?;
    println!(
        "playing {} from {}, Enter stops (play() returned at unix_ms {})",
        args.id,
        view::clock(player.status().position),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis())
    );

    let mut next_report = Instant::now() + REPORT_EVERY;
    loop {
        let status = player.status();
        if status.state == PlayState::Ended {
            println!("finished at {}", view::clock(status.position));
            return Ok(());
        }
        if stop.try_recv().is_ok() {
            player.pause();
            println!("stopped at {}", view::clock(player.status().position));
            return Ok(());
        }
        if Instant::now() >= next_report {
            next_report += REPORT_EVERY;
            println!("  {}", view::clock(status.position));
        }
        std::thread::sleep(TICK);
    }
}
