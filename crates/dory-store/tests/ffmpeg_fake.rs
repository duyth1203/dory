//! The ffmpeg plumbing, checked with a stand-in script so it runs on machines where the real
//! ffmpeg is missing or broken. The script copies a prepared WAV to its last argument, which is
//! where the real call puts the output. This file holds one test because it sets a process-wide
//! environment variable.

use std::os::unix::fs::PermissionsExt;

use dory_core::domain::{Channel, Ms, RecordingId, TakeContainer};
use dory_core::ports::{TakeError, Takes};
use dory_store::FsTakes;

#[test]
fn legacy_webm_goes_through_the_configured_converter_and_a_broken_one_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let canned = dir.path().join("canned.wav");
    let mut w = hound::WavWriter::create(
        &canned,
        hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for i in 0..96_000 {
        let t = i as f32 / 48_000.0;
        w.write_sample((0.5 * (std::f32::consts::TAU * 1_000.0 * t).sin() * 32767.0) as i16)
            .unwrap();
    }
    w.finalize().unwrap();

    let fake = dir.path().join("fake-ffmpeg");
    let args_log = dir.path().join("args.log");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"-version\" ]; then exit 0; fi\nprintf '%s\\n' \"$@\" > '{}'\nfor last; do :; done\ncp '{}' \"$last\"\n",
            args_log.display(),
            canned.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

    let old = dir.path().join("old.webm");
    std::fs::write(&old, b"pretend webm").unwrap();
    let id = RecordingId::parse("old").unwrap();
    let takes = FsTakes::new(dir.path().join("takes"));
    takes
        .link_legacy(&id, &old, TakeContainer::LegacyWebm, 48_000, Ms(2_000))
        .unwrap();

    // SAFETY: this is the only test in the binary, so no other thread reads the environment.
    unsafe { std::env::set_var("DORY_FFMPEG", &fake) };
    let clip = takes.clip(&id, Channel::Mixed, None).unwrap();
    assert_eq!(clip.duration, Ms(2_000));
    let logged = std::fs::read_to_string(&args_log).unwrap();
    assert!(
        logged.contains(old.to_str().unwrap()),
        "the input is the old file: {logged}"
    );
    assert!(logged.contains("-ac\n1"), "decoded to mono: {logged}");
    assert!(
        logged.contains("pcm_s16le"),
        "decoded to 16-bit PCM: {logged}"
    );
    drop(clip);
    assert_eq!(std::fs::read(&old).unwrap(), b"pretend webm");

    let broken = dir.path().join("broken-ffmpeg");
    std::fs::write(
        &broken,
        "#!/bin/sh\necho 'dyld: Library not loaded: libx265' >&2\nexit 134\n",
    )
    .unwrap();
    std::fs::set_permissions(&broken, std::fs::Permissions::from_mode(0o755)).unwrap();
    // SAFETY: as above.
    unsafe {
        std::env::set_var("DORY_FFMPEG", &broken);
        std::env::set_var("PATH", dir.path().join("nowhere"));
    }
    match takes.clip(&id, Channel::Mixed, None) {
        Err(TakeError::ConverterMissing(why)) => assert!(why.contains("does not run"), "{why}"),
        other if std::path::Path::new("/opt/homebrew/bin/ffmpeg").is_file() => {
            // The Homebrew fallback may be a working install on another machine.
            assert!(
                other.is_ok() || matches!(other, Err(TakeError::Decode(_))),
                "{other:?}"
            );
        }
        other => panic!("expected ConverterMissing, got {other:?}"),
    }
}
