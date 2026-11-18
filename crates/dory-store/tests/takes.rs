use std::f32::consts::TAU;
use std::path::Path;

use dory_core::domain::{
    BUCKETS, Channel, ChannelLayout, Ms, RecordingId, TakeContainer, TakeInfo, TimeRange, Waveform,
};
use dory_core::ports::{Activity, TakeError, Takes, Trash as _};
use dory_core::testing::FakeTrash;
use dory_store::FsTakes;

fn rid(s: &str) -> RecordingId {
    RecordingId::parse(s).unwrap()
}

fn tone(freq: f32, rate: u32, from_s: f32, to_s: f32, amp: f32, total_frames: usize) -> Vec<f32> {
    (0..total_frames)
        .map(|i| {
            // f64 phase: an f32 phase has errors of whole milliradians after a few seconds, which
            // is broadband noise that no resampler should be blamed for.
            let t = i as f64 / f64::from(rate);
            if t >= f64::from(from_s) && t < f64::from(to_s) {
                (f64::from(amp) * (std::f64::consts::TAU * f64::from(freq) * t).sin()) as f32
            } else {
                0.0
            }
        })
        .collect()
}

fn interleave(channels: &[&[f32]]) -> Vec<f32> {
    let frames = channels[0].len();
    (0..frames)
        .flat_map(|i| channels.iter().map(move |c| c[i]))
        .collect()
}

fn read_clip(path: &Path) -> (hound::WavSpec, Vec<f32>) {
    let mut reader = hound::WavReader::open(path).unwrap();
    let spec = reader.spec();
    let samples = reader
        .samples::<i16>()
        .map(|s| f32::from(s.unwrap()) / 32768.0)
        .collect();
    (spec, samples)
}

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

fn db(ratio: f32) -> f32 {
    20.0 * ratio.log10()
}

/// Amplitude of the `freq` component, by correlation with a sine and a cosine.
fn amplitude_at(samples: &[f32], freq: f32, rate: u32) -> f32 {
    let (mut i, mut q) = (0.0f64, 0.0f64);
    for (n, s) in samples.iter().enumerate() {
        let phase = f64::from(TAU) * f64::from(freq) * n as f64 / f64::from(rate);
        i += f64::from(*s) * phase.sin();
        q += f64::from(*s) * phase.cos();
    }
    (2.0 * (i * i + q * q).sqrt() / samples.len() as f64) as f32
}

fn write_in_chunks(
    writer: &mut dyn dory_core::ports::TakeWriter,
    interleaved: &[f32],
    channels: usize,
    rate: u32,
) {
    let chunk = (rate as usize / 100) * channels;
    for piece in interleaved.chunks(chunk) {
        writer.write(piece).unwrap();
    }
}

#[test]
fn a_crashed_take_is_recovered_sealed_and_clipped_per_channel_at_16_khz() {
    let dir = tempfile::tempdir().unwrap();
    let id = rid("2026-11-15T153012-7f3a");
    let rate = 48_000u32;
    let frames = rate as usize * 7;
    let mic = tone(1_000.0, rate, 0.0, 100.0, 0.5, frames);
    let system = tone(12_000.0, rate, 0.0, 100.0, 0.5, frames);
    let audio = interleave(&[&mic, &system]);

    let takes = FsTakes::new(dir.path().join("takes"));
    let mut writer = takes.begin(&id, ChannelLayout::MicSystem, rate).unwrap();
    let five_s = rate as usize * 5 * 2;
    write_in_chunks(&mut *writer, &audio[..five_s], 2, rate);
    writer.checkpoint().unwrap();
    write_in_chunks(&mut *writer, &audio[five_s..], 2, rate);
    assert_eq!(writer.frames_written(), frames as u64);
    // kill -9: no finish, no drop, no header update after the checkpoint.
    std::mem::forget(writer);

    let wav = dir.path().join("takes").join(format!("{id}.wav"));
    let header = std::fs::read(&wav).unwrap();
    let declared = u32::from_le_bytes(header[40..44].try_into().unwrap());
    assert_eq!(
        declared as usize,
        five_s * 2,
        "the header still describes the checkpoint"
    );
    assert_eq!(
        header.len(),
        44 + frames * 4,
        "but every written frame reached the file"
    );

    let after_restart = FsTakes::new(dir.path().join("takes"));
    assert_eq!(after_restart.orphans().unwrap(), vec![id.clone()]);
    assert_eq!(
        after_restart.inspect(&id).unwrap(),
        Some(TakeInfo {
            container: TakeContainer::Wav,
            rate_hz: rate,
            duration: Ms(7_000)
        }),
        "the duration comes from the file length"
    );

    let sealed = after_restart.seal(&id).unwrap();
    assert_eq!(
        sealed,
        TakeInfo {
            container: TakeContainer::Flac,
            rate_hz: rate,
            duration: Ms(7_000)
        }
    );
    assert!(
        !wav.exists(),
        "the WAV is deleted once the FLAC is verified"
    );
    assert!(after_restart.orphans().unwrap().is_empty());
    assert_eq!(
        after_restart.seal(&id).unwrap(),
        sealed,
        "sealing again is a no-op"
    );
    assert_eq!(after_restart.inspect(&id).unwrap(), Some(sealed));
    let flac_len = std::fs::metadata(after_restart.playback_path(&id).unwrap())
        .unwrap()
        .len();
    assert!(
        flac_len < (frames as u64 * 4),
        "FLAC is smaller than the 16-bit WAV: {flac_len}"
    );

    let mic_clip = after_restart.clip(&id, Channel::Mic, None).unwrap();
    let (spec, mic_out) = read_clip(&mic_clip.path);
    assert_eq!(
        (spec.channels, spec.sample_rate, spec.bits_per_sample),
        (1, 16_000, 16)
    );
    assert_eq!(mic_out.len(), 7 * 16_000, "7 s at 16 kHz, to the sample");
    assert_eq!((mic_clip.duration, mic_clip.origin), (Ms(7_000), Ms(0)));
    let middle = &mic_out[16_000..6 * 16_000];
    let kept = amplitude_at(middle, 1_000.0, 16_000);
    assert!(
        (kept - 0.5).abs() < 0.01,
        "1 kHz survives at its original level, got {kept}"
    );

    let system_clip = after_restart.clip(&id, Channel::System, None).unwrap();
    let (_, system_out) = read_clip(&system_clip.path);
    assert_eq!(system_out.len(), 7 * 16_000);
    let middle = &system_out[16_000..6 * 16_000];
    let input_rms = rms(&system[48_000..6 * 48_000]);
    let attenuation = db(rms(middle) / input_rms);
    assert!(
        attenuation <= -40.0,
        "a 12 kHz tone must not alias into the 8 kHz band, but it is only {attenuation:.1} dB down"
    );
    assert!(
        rms(middle) < rms(&mic_out[16_000..6 * 16_000]) / 50.0,
        "the system clip does not carry the mic's tone"
    );

    let path = mic_clip.path.clone();
    drop(mic_clip);
    assert!(!path.exists(), "dropping a clip deletes its file");
    assert!(system_clip.path.exists());
}

#[test]
fn a_finished_take_seals_and_a_span_clip_starts_where_asked() {
    let dir = tempfile::tempdir().unwrap();
    let takes = FsTakes::new(dir.path());
    let id = rid("span");
    let rate = 44_100u32;
    let frames = rate as usize * 5;
    let burst = tone(1_000.0, rate, 3.0, 4.0, 0.4, frames);

    let mut writer = takes.begin(&id, ChannelLayout::MicOnly, rate).unwrap();
    write_in_chunks(&mut *writer, &burst, 1, rate);
    let finished = writer.finish().unwrap();
    assert_eq!(
        finished,
        TakeInfo {
            container: TakeContainer::Wav,
            rate_hz: rate,
            duration: Ms(5_000)
        }
    );
    takes.seal(&id).unwrap();

    let span = |a, b| Some(TimeRange::new(Ms(a), Ms(b)).unwrap());
    let quiet = takes.clip(&id, Channel::Mic, span(1_000, 2_000)).unwrap();
    let loud = takes.clip(&id, Channel::Mic, span(3_000, 4_000)).unwrap();
    let (_, quiet_out) = read_clip(&quiet.path);
    let (_, loud_out) = read_clip(&loud.path);
    assert_eq!((quiet_out.len(), loud_out.len()), (16_000, 16_000));
    assert_eq!((quiet.origin, quiet.duration), (Ms(1_000), Ms(1_000)));
    assert_eq!((loud.origin, loud.duration), (Ms(3_000), Ms(1_000)));
    assert!(
        rms(&quiet_out) < 0.001,
        "nothing happens between 1 s and 2 s"
    );
    assert!(
        (amplitude_at(&loud_out[1_000..15_000], 1_000.0, 16_000) - 0.4).abs() < 0.02,
        "the burst is where the span says"
    );

    let past_the_end = takes.clip(&id, Channel::Mic, span(4_500, 9_000)).unwrap();
    assert_eq!(
        past_the_end.duration,
        Ms(500),
        "a span past the end is cut at the end"
    );
    let beyond = takes.clip(&id, Channel::Mic, span(8_000, 9_000)).unwrap();
    assert_eq!(beyond.duration, Ms(0));
}

/// Every span covers `want` and reaches no more than half a second past either end of it.
fn assert_speech(activity: &Activity, want: &[(u64, u64)]) {
    let Activity::Speech(spans) = activity else {
        panic!("activity was not measured: {activity:?}");
    };
    assert_eq!(spans.len(), want.len(), "{spans:?}");
    for (span, (start, end)) in spans.iter().zip(want) {
        assert!(
            span.start().0 <= *start && span.start().0 + 500 >= *start,
            "{span:?} for {start}..{end}"
        );
        assert!(
            span.end().0 >= *end && span.end().0 <= end + 500,
            "{span:?} for {start}..{end}"
        );
    }
}

#[test]
fn clips_report_where_someone_talks_on_their_own_time_axis() {
    let dir = tempfile::tempdir().unwrap();
    let takes = FsTakes::new(dir.path());
    let id = rid("activity");
    let rate = 48_000u32;
    let frames = rate as usize * 10;
    let mic: Vec<f32> = tone(300.0, rate, 1.0, 3.0, 0.3, frames)
        .iter()
        .zip(tone(300.0, rate, 6.0, 7.5, 0.3, frames))
        .map(|(a, b)| a + b)
        .collect();
    let system = vec![0.0; frames];
    let mut writer = takes.begin(&id, ChannelLayout::MicSystem, rate).unwrap();
    write_in_chunks(&mut *writer, &interleave(&[&mic, &system]), 2, rate);
    writer.finish().unwrap();
    takes.seal(&id).unwrap();

    let whole = takes.clip(&id, Channel::Mic, None).unwrap();
    assert_speech(&whole.activity, &[(1_000, 3_000), (6_000, 7_500)]);
    let silent = takes.clip(&id, Channel::System, None).unwrap();
    assert_eq!(
        silent.activity,
        Activity::Speech(vec![]),
        "measured and silent"
    );

    let span = TimeRange::new(Ms(5_000), Ms(9_000)).unwrap();
    let late = takes.clip(&id, Channel::Mic, Some(span)).unwrap();
    assert_speech(&late.activity, &[(1_000, 2_500)]);
}

#[test]
fn a_slice_cuts_a_window_from_the_whole_clip_without_decoding_the_take_again() {
    let dir = tempfile::tempdir().unwrap();
    let takes = FsTakes::new(dir.path());
    let id = rid("slice");
    let rate = 48_000u32;
    let frames = rate as usize * 10;
    let mic = tone(300.0, rate, 6.0, 7.5, 0.3, frames);
    let mut writer = takes.begin(&id, ChannelLayout::MicOnly, rate).unwrap();
    write_in_chunks(&mut *writer, &mic, 1, rate);
    writer.finish().unwrap();
    takes.seal(&id).unwrap();

    let whole = takes.clip(&id, Channel::Mic, None).unwrap();
    let (_, whole_out) = read_clip(&whole.path);
    // The take is gone, so only the whole clip can feed the slice.
    takes.delete(&id).unwrap();

    let span = TimeRange::new(Ms(5_000), Ms(9_000)).unwrap();
    let window = takes.slice(&whole, span).unwrap();
    let (spec, out) = read_clip(&window.path);
    assert_eq!((spec.sample_rate, spec.channels), (16_000, 1));
    assert_eq!(
        out,
        whole_out[80_000..144_000],
        "the same samples as the whole clip"
    );
    assert_eq!((window.origin, window.duration), (Ms(5_000), Ms(4_000)));
    assert_eq!(window.channel, Channel::Mic);
    assert_speech(&window.activity, &[(1_000, 2_500)]);

    let tail = takes
        .slice(&whole, TimeRange::new(Ms(9_500), Ms(12_000)).unwrap())
        .unwrap();
    assert_eq!(
        tail.duration,
        Ms(500),
        "a span past the end is cut at the end"
    );

    let path = window.path.clone();
    drop(window);
    assert!(!path.exists(), "dropping the slice removes its file");
    assert!(whole.path.exists(), "the whole clip outlives its slices");
}

#[test]
fn channels_are_picked_by_layout() {
    let dir = tempfile::tempdir().unwrap();
    let takes = FsTakes::new(dir.path());
    let rate = 16_000u32;
    let left = tone(500.0, rate, 0.0, 9.0, 0.5, rate as usize);
    let right = tone(2_000.0, rate, 0.0, 9.0, 0.25, rate as usize);

    let id = rid("stereo");
    let mut writer = takes.begin(&id, ChannelLayout::MicSystem, rate).unwrap();
    writer.write(&interleave(&[&left, &right])).unwrap();
    writer.finish().unwrap();
    takes.seal(&id).unwrap();

    let amp = |channel, freq| {
        let clip = takes.clip(&id, channel, None).unwrap();
        let (spec, out) = read_clip(&clip.path);
        assert_eq!(spec.sample_rate, 16_000);
        assert_eq!(
            out.len(),
            16_000,
            "an already-16k take passes through at the same length"
        );
        amplitude_at(&out[2_000..14_000], freq, 16_000)
    };
    assert!((amp(Channel::Mic, 500.0) - 0.5).abs() < 0.01);
    assert!(amp(Channel::Mic, 2_000.0) < 0.01);
    assert!((amp(Channel::System, 2_000.0) - 0.25).abs() < 0.01);
    assert!(amp(Channel::System, 500.0) < 0.01);
    assert!(
        (amp(Channel::Mixed, 500.0) - 0.25).abs() < 0.01,
        "Mixed averages both channels"
    );
    assert!((amp(Channel::Mixed, 2_000.0) - 0.125).abs() < 0.01);

    let mono = rid("mic-only");
    let mut writer = takes.begin(&mono, ChannelLayout::MicOnly, rate).unwrap();
    writer.write(&left).unwrap();
    writer.finish().unwrap();
    assert!(
        takes.clip(&mono, Channel::System, None).is_ok(),
        "a mono file has one channel to give"
    );
}

#[test]
fn a_layout_without_the_channel_is_refused_on_a_stereo_take() {
    let dir = tempfile::tempdir().unwrap();
    let takes = FsTakes::new(dir.path());
    let id = rid("s");
    let mut writer = takes.begin(&id, ChannelLayout::MicSystem, 16_000).unwrap();
    writer.write(&[0.0; 320]).unwrap();
    writer.finish().unwrap();
    std::fs::write(dir.path().join("s.meta"), "layout=system_only\n").unwrap();
    assert!(matches!(
        takes.clip(&id, Channel::Mic, None),
        Err(TakeError::Decode(_))
    ));
}

#[test]
fn begin_refuses_an_existing_take_and_write_refuses_torn_frames() {
    let dir = tempfile::tempdir().unwrap();
    let takes = FsTakes::new(dir.path());
    let id = rid("dup");
    let mut writer = takes.begin(&id, ChannelLayout::MicSystem, 48_000).unwrap();
    assert!(matches!(
        takes.begin(&id, ChannelLayout::MicSystem, 48_000),
        Err(TakeError::Exists(_))
    ));
    assert!(matches!(writer.write(&[0.0; 3]), Err(TakeError::Io(_))));
    assert_eq!(writer.frames_written(), 0, "a refused write writes nothing");
    writer.write(&[0.0; 4]).unwrap();
    writer.finish().unwrap();
    takes.seal(&id).unwrap();
    assert!(
        matches!(
            takes.begin(&id, ChannelLayout::MicOnly, 48_000),
            Err(TakeError::Exists(_))
        ),
        "a sealed take also blocks a new begin"
    );
    assert!(matches!(
        takes.begin(&rid("zero"), ChannelLayout::MicOnly, 0),
        Err(TakeError::Io(_))
    ));
}

#[test]
fn an_empty_take_seals_to_a_valid_empty_flac() {
    let dir = tempfile::tempdir().unwrap();
    let takes = FsTakes::new(dir.path());
    let id = rid("empty");
    takes
        .begin(&id, ChannelLayout::MicSystem, 48_000)
        .unwrap()
        .finish()
        .unwrap();
    let info = takes.seal(&id).unwrap();
    assert_eq!(
        (info.container, info.duration),
        (TakeContainer::Flac, Ms(0))
    );
    let clip = takes.clip(&id, Channel::Mic, None).unwrap();
    assert_eq!(clip.duration, Ms(0));
}

#[test]
fn seal_finishes_a_crash_between_the_flac_rename_and_the_wav_delete() {
    let dir = tempfile::tempdir().unwrap();
    let takes = FsTakes::new(dir.path());
    let id = rid("midway");
    let mut writer = takes.begin(&id, ChannelLayout::MicOnly, 16_000).unwrap();
    writer
        .write(&tone(300.0, 16_000, 0.0, 9.0, 0.5, 32_000))
        .unwrap();
    writer.finish().unwrap();
    let wav = dir.path().join("midway.wav");
    let copy = dir.path().join("saved.wav");
    std::fs::copy(&wav, &copy).unwrap();
    takes.seal(&id).unwrap();
    std::fs::rename(&copy, &wav).unwrap();
    assert_eq!(
        takes.orphans().unwrap(),
        vec![id.clone()],
        "a leftover WAV is listed so it gets cleaned"
    );

    let info = takes.seal(&id).unwrap();
    assert_eq!(
        (info.container, info.duration),
        (TakeContainer::Flac, Ms(2_000))
    );
    assert!(!wav.exists());
}

#[test]
fn seal_rebuilds_a_flac_that_does_not_match_its_wav() {
    let dir = tempfile::tempdir().unwrap();
    let takes = FsTakes::new(dir.path());
    let id = rid("stale-flac");
    let mut writer = takes.begin(&id, ChannelLayout::MicOnly, 16_000).unwrap();
    writer
        .write(&tone(300.0, 16_000, 0.0, 9.0, 0.5, 32_000))
        .unwrap();
    writer.finish().unwrap();
    std::fs::write(dir.path().join("stale-flac.flac"), b"not a flac").unwrap();
    let info = takes.seal(&id).unwrap();
    assert_eq!(info.duration, Ms(2_000));
    let clip = takes.clip(&id, Channel::Mic, None).unwrap();
    assert_eq!(clip.duration, Ms(2_000));
}

#[test]
fn a_leftover_tmp_flac_from_a_crash_during_encoding_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let takes = FsTakes::new(dir.path());
    let id = rid("tmp");
    let mut writer = takes.begin(&id, ChannelLayout::MicOnly, 16_000).unwrap();
    writer.write(&[0.25; 16_000]).unwrap();
    writer.finish().unwrap();
    std::fs::write(dir.path().join("tmp.flac.tmp"), b"half written").unwrap();
    takes.seal(&id).unwrap();
    assert!(!dir.path().join("tmp.flac.tmp").exists());
}

#[test]
fn delete_removes_everything_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let takes = FsTakes::new(dir.path());
    let id = rid("gone");
    takes
        .begin(&id, ChannelLayout::MicOnly, 16_000)
        .unwrap()
        .finish()
        .unwrap();
    takes.seal(&id).unwrap();
    takes.delete(&id).unwrap();
    takes.delete(&id).unwrap();
    assert_eq!(takes.inspect(&id).unwrap(), None);
    assert!(matches!(
        takes.playback_path(&id),
        Err(TakeError::Missing(_))
    ));
    assert!(matches!(
        takes.clip(&id, Channel::Mic, None),
        Err(TakeError::Missing(_))
    ));
    assert!(matches!(takes.seal(&id), Err(TakeError::Missing(_))));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn playback_path_follows_the_take_through_its_life() {
    let dir = tempfile::tempdir().unwrap();
    let takes = FsTakes::new(dir.path());
    let id = rid("life");
    let writer = takes.begin(&id, ChannelLayout::MicOnly, 16_000).unwrap();
    assert_eq!(
        takes.playback_path(&id).unwrap(),
        dir.path().join("life.wav")
    );
    writer.finish().unwrap();
    takes.seal(&id).unwrap();
    assert_eq!(
        takes.playback_path(&id).unwrap(),
        dir.path().join("life.flac")
    );
}

#[test]
fn importing_a_stereo_wav_makes_a_sealed_mono_mixed_take() {
    let dir = tempfile::tempdir().unwrap();
    let takes = FsTakes::new(dir.path().join("takes"));
    let source = dir.path().join("meeting.wav");
    let rate = 44_100u32;
    let left = tone(1_000.0, rate, 0.0, 9.0, 0.5, rate as usize * 2);
    let right = vec![0.0f32; left.len()];
    let mut external = hound::WavWriter::create(
        &source,
        hound::WavSpec {
            channels: 2,
            sample_rate: rate,
            bits_per_sample: 24,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for s in interleave(&[&left, &right]) {
        external.write_sample((s * 8_388_607.0) as i32).unwrap();
    }
    external.finalize().unwrap();

    let id = rid("imported");
    let info = takes.import(&id, &source).unwrap();
    assert_eq!(
        info,
        TakeInfo {
            container: TakeContainer::Flac,
            rate_hz: rate,
            duration: Ms(2_000)
        }
    );
    assert!(source.exists(), "the source file is left alone");
    assert!(matches!(
        takes.import(&id, &source),
        Err(TakeError::Exists(_))
    ));

    let clip = takes.clip(&id, Channel::Mixed, None).unwrap();
    let (_, out) = read_clip(&clip.path);
    assert_eq!(out.len(), 32_000);
    let amp = amplitude_at(&out[4_000..28_000], 1_000.0, 16_000);
    assert!(
        (amp - 0.25).abs() < 0.01,
        "stereo is averaged into mono, got {amp}"
    );
}

#[test]
fn importing_a_flac_copies_it_in_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let a = FsTakes::new(dir.path().join("a"));
    let b = FsTakes::new(dir.path().join("b"));
    let id = rid("src");
    let mut writer = a.begin(&id, ChannelLayout::MicOnly, 16_000).unwrap();
    writer.write(&[0.1; 8_000]).unwrap();
    writer.finish().unwrap();
    a.seal(&id).unwrap();
    let source = a.playback_path(&id).unwrap();

    let copied = rid("copy");
    let info = b.import(&copied, &source).unwrap();
    assert_eq!(
        (info.container, info.duration),
        (TakeContainer::Flac, Ms(500))
    );
    assert_eq!(
        std::fs::read(&source).unwrap(),
        std::fs::read(b.playback_path(&copied).unwrap()).unwrap()
    );
}

/// At 16 kHz the clip is the take's own samples: no resampling, and no gain from scaling a
/// sample into float and back with two different constants.
#[test]
fn a_16_khz_clip_holds_the_takes_samples_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("ramp.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let ramp: Vec<i16> = (i16::MIN..=i16::MAX).step_by(3).chain([i16::MAX]).collect();
    let mut writer = hound::WavWriter::create(&source, spec).unwrap();
    for s in &ramp {
        writer.write_sample(*s).unwrap();
    }
    writer.finalize().unwrap();

    let takes = FsTakes::new(dir.path().join("takes"));
    let id = rid("ramp");
    takes.import(&id, &source).unwrap();
    let clip = takes.clip(&id, Channel::Mixed, None).unwrap();
    let got: Vec<i16> = hound::WavReader::open(&clip.path)
        .unwrap()
        .samples::<i16>()
        .map(Result::unwrap)
        .collect();
    assert_eq!(got.len(), ramp.len());
    let changed = got.iter().zip(&ramp).filter(|(a, b)| a != b).count();
    assert_eq!(changed, 0, "{changed} of {} samples changed", ramp.len());
}

#[test]
fn importing_garbage_fails_without_leaving_a_take() {
    let dir = tempfile::tempdir().unwrap();
    let takes = FsTakes::new(dir.path().join("takes"));
    let source = dir.path().join("noise.wav");
    std::fs::write(&source, b"this is not audio").unwrap();
    assert!(takes.import(&rid("bad"), &source).is_err());
    assert_eq!(takes.inspect(&rid("bad")).unwrap(), None);
    assert!(takes.orphans().unwrap().is_empty());
    assert!(matches!(
        takes.import(&rid("nofile"), &dir.path().join("missing.flac")),
        Err(TakeError::Missing(_))
    ));
}

fn working_ffmpeg() -> Option<std::path::PathBuf> {
    let mut candidates: Vec<std::path::PathBuf> = std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p)
                .map(|d| d.join("ffmpeg"))
                .collect()
        })
        .unwrap_or_default();
    candidates.push("/opt/homebrew/bin/ffmpeg".into());
    candidates.into_iter().find(|p| {
        std::process::Command::new(p)
            .arg("-version")
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

#[test]
fn a_legacy_webm_is_decoded_through_ffmpeg() {
    let Some(ffmpeg) = working_ffmpeg() else {
        eprintln!(
            "skipped: no working ffmpeg on this machine (see tests/ffmpeg_fake.rs for the plumbing test)"
        );
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let webm = dir.path().join("old.webm");
    let status = std::process::Command::new(ffmpeg)
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=1000:sample_rate=48000:duration=2",
        ])
        .args(["-c:a", "libopus"])
        .arg(&webm)
        .status()
        .unwrap();
    assert!(status.success(), "could not make a WebM fixture");

    let takes = FsTakes::new(dir.path().join("takes"));
    let id = rid("old");
    let info = takes
        .link_legacy(&id, &webm, TakeContainer::LegacyWebm, 48_000, Ms(2_000))
        .unwrap();
    assert_eq!(info.container, TakeContainer::LegacyWebm);
    assert_eq!(takes.inspect(&id).unwrap(), Some(info));
    assert_eq!(takes.playback_path(&id).unwrap(), webm);

    let clip = takes.clip(&id, Channel::Mixed, None).unwrap();
    let (spec, out) = read_clip(&clip.path);
    assert_eq!(spec.sample_rate, 16_000);
    assert!(
        (out.len() as i64 - 32_000).abs() < 1_700,
        "about 2 s, got {} samples",
        out.len()
    );
    let amp = amplitude_at(&out[8_000..24_000], 1_000.0, 16_000);
    assert!(amp > 0.05, "the tone survived the decode, got {amp}");

    assert!(
        matches!(takes.seal(&id), Ok(i) if i.container == TakeContainer::LegacyWebm),
        "sealing a legacy take is a no-op"
    );
    takes.delete(&id).unwrap();
    assert!(
        webm.exists(),
        "deleting the take never deletes the old audio"
    );
    assert_eq!(takes.inspect(&id).unwrap(), None);
}

#[test]
fn a_legacy_wav_is_read_in_place_without_ffmpeg() {
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("old.wav");
    let rate = 22_050u32;
    let mut w = hound::WavWriter::create(
        &old,
        hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for s in tone(1_000.0, rate, 0.0, 9.0, 0.5, rate as usize * 3) {
        w.write_sample((s * 32767.0) as i16).unwrap();
    }
    w.finalize().unwrap();
    let before = std::fs::read(&old).unwrap();

    let takes = FsTakes::new(dir.path().join("takes"));
    let id = rid("old");
    let info = takes
        .link_legacy(&id, &old, TakeContainer::Wav, rate, Ms(3_000))
        .unwrap();
    assert_eq!(info.container, TakeContainer::Wav);
    assert!(
        takes.orphans().unwrap().is_empty(),
        "a legacy WAV is not an unsealed take of ours"
    );
    assert_eq!(takes.seal(&id).unwrap(), info);

    let clip = takes.clip(&id, Channel::Mixed, None).unwrap();
    let (_, out) = read_clip(&clip.path);
    assert_eq!(out.len(), 48_000);
    assert!((amplitude_at(&out[4_000..44_000], 1_000.0, 16_000) - 0.5).abs() < 0.01);
    takes.delete(&id).unwrap();
    assert_eq!(
        std::fs::read(&old).unwrap(),
        before,
        "the old file is untouched"
    );
}

#[test]
fn linking_the_same_legacy_file_twice_writes_nothing_the_second_time() {
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("old.webm");
    std::fs::write(&old, b"x").unwrap();
    let takes = FsTakes::new(dir.path().join("takes"));
    let id = rid("old");
    takes
        .link_legacy(&id, &old, TakeContainer::LegacyWebm, 48_000, Ms(5))
        .unwrap();
    let pointer = dir.path().join("takes").join("old.legacy");
    let first = std::fs::metadata(&pointer).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    takes
        .link_legacy(&id, &old, TakeContainer::LegacyWebm, 48_000, Ms(5))
        .unwrap();
    assert_eq!(
        std::fs::metadata(&pointer).unwrap().modified().unwrap(),
        first
    );
    assert!(matches!(
        takes.link_legacy(
            &rid("none"),
            &dir.path().join("absent.webm"),
            TakeContainer::LegacyWebm,
            48_000,
            Ms(5)
        ),
        Err(TakeError::Missing(_))
    ));
}

#[test]
fn stored_lists_every_id_with_files_in_any_state_and_delete_clears_each() {
    let dir = tempfile::tempdir().unwrap();
    let takes = FsTakes::new(dir.path());
    let old = dir.path().join("old.webm");
    std::fs::write(&old, b"legacy audio").unwrap();

    let open = rid("open");
    drop(takes.begin(&open, ChannelLayout::MicOnly, 16_000).unwrap());
    let sealed = rid("sealed");
    takes
        .begin(&sealed, ChannelLayout::MicOnly, 16_000)
        .unwrap()
        .finish()
        .unwrap();
    takes.seal(&sealed).unwrap();
    // A crash in the middle of encoding leaves a half FLAC beside the layout file.
    let half = rid("rec.2026.half");
    std::fs::write(dir.path().join("rec.2026.half.flac.tmp"), b"half").unwrap();
    std::fs::write(dir.path().join("rec.2026.half.meta"), b"layout=mic\n").unwrap();
    let linked = rid("linked");
    takes
        .link_legacy(&linked, &old, TakeContainer::LegacyWebm, 48_000, Ms(5))
        .unwrap();
    std::fs::write(dir.path().join("not an id!.wav"), b"").unwrap();

    assert_eq!(
        takes.stored().unwrap(),
        vec![linked.clone(), open.clone(), half.clone(), sealed.clone()]
    );
    assert_eq!(takes.orphans().unwrap(), vec![open.clone()]);

    for id in [&linked, &open, &half, &sealed] {
        takes.delete(id).unwrap();
    }
    assert!(takes.stored().unwrap().is_empty());
    assert_eq!(
        std::fs::read(&old).unwrap(),
        b"legacy audio",
        "deleting a legacy pointer never touches the old app's file"
    );
}

#[test]
fn files_lists_what_delete_removes_for_one_id_and_never_a_legacy_audio_file() {
    let dir = tempfile::tempdir().unwrap();
    let takes_dir = dir.path().join("takes");
    let takes = FsTakes::new(&takes_dir);
    let old = dir.path().join("old.webm");
    std::fs::write(&old, b"legacy audio").unwrap();

    let open = rid("open");
    drop(takes.begin(&open, ChannelLayout::MicOnly, 16_000).unwrap());
    let sealed = rid("sealed");
    takes
        .begin(&sealed, ChannelLayout::MicSystem, 16_000)
        .unwrap()
        .finish()
        .unwrap();
    takes.seal(&sealed).unwrap();
    let half = rid("half");
    std::fs::write(takes_dir.join("half.flac.tmp"), b"half").unwrap();
    std::fs::write(takes_dir.join("half.meta"), b"layout=mic\n").unwrap();
    let linked = rid("linked");
    takes
        .link_legacy(&linked, &old, TakeContainer::LegacyWebm, 48_000, Ms(5))
        .unwrap();

    let names = |id: &RecordingId| {
        let mut names: Vec<String> = takes
            .files(id)
            .unwrap()
            .iter()
            .map(|path| {
                assert_eq!(path.parent(), Some(takes_dir.as_path()));
                path.file_name().unwrap().to_string_lossy().into_owned()
            })
            .collect();
        names.sort();
        names
    };
    assert_eq!(names(&open), ["open.meta", "open.wav"]);
    assert_eq!(
        names(&sealed),
        ["sealed.flac", "sealed.levels", "sealed.meta"]
    );
    assert_eq!(names(&half), ["half.flac.tmp", "half.meta"]);
    assert_eq!(names(&linked), ["linked.legacy"]);
    assert_eq!(names(&rid("none")), Vec::<String>::new());

    for id in [&open, &sealed, &half, &linked] {
        let listed = takes.files(id).unwrap();
        assert!(listed.iter().all(|path| path.is_file()));
        takes.delete(id).unwrap();
        assert!(listed.iter().all(|path| !path.exists()), "{id}");
        assert!(takes.files(id).unwrap().is_empty());
    }
    assert_eq!(std::fs::read_dir(&takes_dir).unwrap().count(), 0);
    assert_eq!(std::fs::read(&old).unwrap(), b"legacy audio");
}

const OLD_RATE: u32 = 22_050;

fn write_old_wav(path: &Path) {
    let mut w = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: 1,
            sample_rate: OLD_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for s in tone(1_000.0, OLD_RATE, 0.0, 9.0, 0.5, OLD_RATE as usize * 3) {
        w.write_sample((s * 32767.0) as i16).unwrap();
    }
    w.finalize().unwrap();
}

fn names(takes: &FsTakes, id: &RecordingId) -> Vec<String> {
    let mut names: Vec<String> = takes
        .files(id)
        .unwrap()
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn linked_wav(dir: &Path, name: &str) -> (FsTakes, RecordingId, std::path::PathBuf) {
    let old = dir.join(format!("{name}.wav"));
    write_old_wav(&old);
    let takes = FsTakes::new(dir.join("takes"));
    let id = rid(name);
    takes
        .link_legacy(&id, &old, TakeContainer::Wav, OLD_RATE, Ms(3_000))
        .unwrap();
    (takes, id, old)
}

#[test]
fn adopting_a_legacy_wav_copies_it_in_as_a_flac_take_and_leaves_the_original_alone() {
    let dir = tempfile::tempdir().unwrap();
    let (takes, id, old) = linked_wav(dir.path(), "old");
    let before = std::fs::read(&old).unwrap();
    assert_eq!(
        takes
            .legacy_original(&id)
            .unwrap()
            .map(|f| f.path)
            .as_deref(),
        Some(&*old)
    );

    let info = takes.adopt(&id).unwrap();
    assert_eq!(
        info,
        TakeInfo {
            container: TakeContainer::Flac,
            rate_hz: OLD_RATE,
            duration: Ms(3_000)
        }
    );
    assert_eq!(takes.legacy_original(&id).unwrap().map(|f| f.path), None);
    assert_eq!(names(&takes, &id), ["old.flac", "old.levels", "old.meta"]);
    assert_eq!(takes.inspect(&id).unwrap(), Some(info));
    assert_eq!(
        std::fs::read(&old).unwrap(),
        before,
        "the original is untouched"
    );

    std::fs::remove_file(&old).unwrap();
    let clip = takes.clip(&id, Channel::Mixed, None).unwrap();
    let (_, out) = read_clip(&clip.path);
    assert_eq!(out.len(), 48_000);
    assert!((amplitude_at(&out[4_000..44_000], 1_000.0, 16_000) - 0.5).abs() < 0.01);
    assert!(takes.playback_path(&id).unwrap().ends_with("old.flac"));
}

#[test]
fn adopting_twice_changes_nothing_and_adopting_an_own_take_returns_it() {
    let dir = tempfile::tempdir().unwrap();
    let (takes, id, _) = linked_wav(dir.path(), "again");
    let first = takes.adopt(&id).unwrap();
    let flac = takes.dir().join("again.flac");
    let bytes = std::fs::read(&flac).unwrap();

    assert_eq!(takes.adopt(&id).unwrap(), first);
    assert_eq!(std::fs::read(&flac).unwrap(), bytes);

    let own = rid("own");
    let mut writer = takes.begin(&own, ChannelLayout::Mixed, 16_000).unwrap();
    writer.write(&[0.0; 1_600]).unwrap();
    writer.finish().unwrap();
    let sealed = takes.seal(&own).unwrap();
    assert_eq!(takes.adopt(&own).unwrap(), sealed);

    assert!(matches!(
        takes.adopt(&rid("nothing")),
        Err(TakeError::Missing(_))
    ));
}

#[test]
fn adopting_cleans_up_what_a_crashed_attempt_left() {
    let dir = tempfile::tempdir().unwrap();
    let (takes, id, _) = linked_wav(dir.path(), "crashed");
    let tmp = takes.dir().join("crashed.flac.tmp");
    let partial = takes.dir().join("crashed.wav.partial");
    std::fs::write(&tmp, b"half an encoder run").unwrap();
    std::fs::write(&partial, b"half a decode").unwrap();

    let info = takes.adopt(&id).unwrap();
    assert_eq!(info.container, TakeContainer::Flac);
    assert_eq!(info.duration, Ms(3_000));
    assert_eq!(
        names(&takes, &id),
        ["crashed.flac", "crashed.levels", "crashed.meta"]
    );
}

#[test]
fn a_crash_after_the_flac_landed_and_before_the_pointer_went_is_finished_by_a_second_adopt() {
    let dir = tempfile::tempdir().unwrap();
    let (takes, id, old) = linked_wav(dir.path(), "late");
    let pointer = takes.dir().join("late.legacy");
    let pointer_text = std::fs::read(&pointer).unwrap();
    let done = takes.adopt(&id).unwrap();
    std::fs::write(&pointer, pointer_text).unwrap();
    assert_eq!(
        takes.inspect(&id).unwrap().unwrap().container,
        TakeContainer::Wav,
        "while the pointer stands, the legacy take is what reads"
    );

    assert_eq!(takes.adopt(&id).unwrap(), done);
    assert_eq!(takes.legacy_original(&id).unwrap().map(|f| f.path), None);
    assert_eq!(
        names(&takes, &id),
        ["late.flac", "late.levels", "late.meta"]
    );
    assert!(old.is_file());
}

#[test]
fn adopting_without_the_original_fails_and_keeps_the_pointer() {
    let dir = tempfile::tempdir().unwrap();
    let (takes, id, old) = linked_wav(dir.path(), "lost");
    std::fs::remove_file(&old).unwrap();

    assert!(matches!(takes.adopt(&id), Err(TakeError::Missing(_))));
    assert_eq!(
        takes
            .legacy_original(&id)
            .unwrap()
            .map(|f| f.path)
            .as_deref(),
        Some(&*old)
    );
    assert_eq!(names(&takes, &id), ["lost.legacy"]);
}

/// Ten seconds of `[mic, system]`: a 0.5 tone on the mic for the first five, then silence.
fn loud_then_silent(takes: &FsTakes, id: &RecordingId) {
    let rate = 16_000;
    let frames = rate as usize * 10;
    let mic = tone(440.0, rate, 0.0, 5.0, 0.5, frames);
    let system = vec![0.0; frames];
    let mut writer = takes.begin(id, ChannelLayout::MicSystem, rate).unwrap();
    write_in_chunks(&mut *writer, &interleave(&[&mic, &system]), 2, rate);
    writer.finish().unwrap();
    takes.seal(id).unwrap();
}

#[test]
fn sealing_keeps_levels_that_are_high_where_the_take_is_loud_and_floor_where_it_is_silent() {
    let dir = tempfile::tempdir().unwrap();
    let takes = FsTakes::new(dir.path());
    let id = rid("halves");
    loud_then_silent(&takes, &id);
    assert!(
        dir.path().join("halves.levels").is_file(),
        "sealing measured the levels"
    );

    let levels = takes.levels(&id).unwrap();
    let (loud, silent) = levels.as_slice().split_at(BUCKETS / 2);
    assert_eq!(levels.as_slice().len(), BUCKETS);
    let tone_rms = 0.5 / 2f32.sqrt();
    assert!(loud.iter().all(|l| (l - tone_rms).abs() < 0.02), "{loud:?}");
    assert!(silent.iter().all(|l| *l == 0.), "{silent:?}");

    let wave = Waveform::from_levels(&levels);
    let (loud, silent) = wave.heights().split_at(BUCKETS / 2);
    assert!(loud.iter().all(|h| *h > 0.99), "{loud:?}");
    assert!(silent.iter().all(|h| *h == Waveform::FLOOR), "{silent:?}");
}

#[test]
fn a_take_without_kept_levels_measures_them_on_first_request_and_reuses_them() {
    let dir = tempfile::tempdir().unwrap();
    let (takes, id, old) = linked_wav(dir.path(), "lazy");
    let kept = takes.dir().join("lazy.levels");
    assert!(!kept.exists());

    let first = takes.levels(&id).unwrap();
    assert_eq!(first.as_slice().len(), BUCKETS);
    assert!(first.as_slice().iter().all(|l| *l > 0.3), "{first:?}");
    assert!(kept.is_file(), "the first request kept what it measured");
    assert_eq!(names(&takes, &id), ["lazy.legacy", "lazy.levels"]);

    std::fs::remove_file(&old).unwrap();
    assert_eq!(
        takes.levels(&id).unwrap(),
        first,
        "the second request reads the kept file, not the audio, which is gone"
    );

    std::fs::write(&kept, b"torn").unwrap();
    assert!(
        matches!(takes.levels(&id), Err(TakeError::Missing(_))),
        "a torn file is measured again, from the audio"
    );
}

#[test]
fn an_open_take_is_measured_but_not_kept() {
    let dir = tempfile::tempdir().unwrap();
    let takes = FsTakes::new(dir.path());
    let id = rid("open");
    let mut writer = takes.begin(&id, ChannelLayout::MicOnly, 16_000).unwrap();
    writer.write(&vec![0.5; 16_000]).unwrap();
    writer.checkpoint().unwrap();

    assert_eq!(takes.levels(&id).unwrap().as_slice().len(), BUCKETS);
    assert!(!dir.path().join("open.levels").exists());
}

#[test]
fn trash_and_put_back_move_the_levels_with_the_audio_and_delete_removes_them() {
    let dir = tempfile::tempdir().unwrap();
    let takes = FsTakes::new(dir.path().join("takes"));
    let trash_dir = dir.path().join("trash");
    std::fs::create_dir_all(&trash_dir).unwrap();
    let trash = FakeTrash::new(&trash_dir);
    let id = rid("undo");
    loud_then_silent(&takes, &id);
    let kept = takes.dir().join("undo.levels");
    let before = std::fs::read(&kept).unwrap();
    assert_eq!(
        names(&takes, &id),
        ["undo.flac", "undo.levels", "undo.meta"]
    );

    let receipt = trash.move_to_trash(&takes.files(&id).unwrap()).unwrap();
    assert!(takes.files(&id).unwrap().is_empty());
    assert!(!kept.exists());

    trash.put_back(&receipt).unwrap();
    assert_eq!(std::fs::read(&kept).unwrap(), before);
    assert_eq!(
        names(&takes, &id),
        ["undo.flac", "undo.levels", "undo.meta"]
    );

    takes.delete(&id).unwrap();
    assert!(!kept.exists());
    assert!(takes.stored().unwrap().is_empty());
}
