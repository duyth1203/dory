//! Measures one take's levels through `FsTakes`, for timing the pass on a real take.
//!
//! `cargo run --release -p dory-store --example levels -- <takes dir> <id>`. The first run on a
//! take with no `<id>.levels` reads the whole take and keeps the file; a second run reads it.

use std::time::Instant;

use dory_core::domain::{RecordingId, Waveform};
use dory_core::ports::Takes as _;
use dory_store::FsTakes;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [dir, id] = args.as_slice() else {
        eprintln!("usage: levels <takes dir> <id>");
        std::process::exit(2);
    };
    let takes = FsTakes::new(dir);
    let id = RecordingId::parse(id).expect("a recording id");
    let kept = takes.dir().join(format!("{id}.levels")).is_file();
    let started = Instant::now();
    let levels = takes.levels(&id).expect("levels");
    let elapsed = started.elapsed();
    let heights = Waveform::from_levels(&levels);
    let floor = heights
        .heights()
        .iter()
        .filter(|h| **h == Waveform::FLOOR)
        .count();
    let loudest = levels.as_slice().iter().copied().fold(0., f32::max);
    println!(
        "{} buckets in {:.3} s ({}), loudest RMS {loudest:.4}, {floor} at the floor",
        levels.as_slice().len(),
        elapsed.as_secs_f64(),
        if kept {
            "read the kept file"
        } else {
            "measured the take"
        },
    );
}
