//! `FsTakes`: audio bytes on disk.
//!
//! One folder, one set of files per recording id:
//!
//! - `<id>.wav` while recording. 16-bit PCM, written by `hound`, so a crash leaves a file that
//!   [`Takes::orphans`] finds and [`Takes::seal`] finishes.
//! - `<id>.flac` after sealing. Written next to the WAV as `<id>.flac.tmp`, decoded back and
//!   compared with the WAV, then renamed into place. Only after that is the WAV deleted, so every
//!   crash point leaves a state that a second `seal` completes.
//! - `<id>.meta` holds the channel layout, because `clip` is asked for "the mic" by id alone.
//! - `<id>.legacy` points at an audio file in the old app's folder. That file is read in place and
//!   is never moved or deleted. [`Takes::adopt`] copies it in as a FLAC take and drops the pointer.
//!
//! [`FILES`] lists every name this adapter writes, so [`Takes::stored`] and [`Takes::delete`]
//! agree on what belongs to an id.

mod ffmpeg;
mod flac;
mod meta;
mod pcm;
mod resample;
mod vad;
mod wavtake;

use std::fs::File;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use dory_core::domain::{
    Channel, ChannelLayout, Ms, RecordingId, TakeContainer, TakeInfo, TimeRange,
};
use dory_core::ports::{Activity, Clip, TakeError, TakeWriter, Takes};

use pcm::{FlacReader, FrameReader, HoundReader, OwnWavReader, Pick};
use resample::{Downsampler, TARGET_RATE_HZ};
use vad::Vad;
use wavtake::WavTake;

pub(crate) fn io_err(e: std::io::Error) -> TakeError {
    if e.kind() == std::io::ErrorKind::StorageFull {
        TakeError::DiskFull
    } else {
        TakeError::Io(e)
    }
}

fn nonzero(rate_hz: u32) -> Result<NonZeroU32, TakeError> {
    NonZeroU32::new(rate_hz).ok_or_else(|| TakeError::Decode("sample rate is zero".into()))
}

/// Every suffix this adapter writes after `<id>.`, two-part ones first so `a.flac.tmp` is never
/// read as an id `a.flac` with a suffix `tmp`.
const FILES: [&str; 8] = [
    "wav.partial",
    "flac.tmp",
    "meta.tmp",
    "legacy.tmp",
    "wav",
    "flac",
    "meta",
    "legacy",
];

fn remove_if_present(path: &Path) -> Result<(), TakeError> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(io_err(e)),
        _ => Ok(()),
    }
}

/// The pointer a legacy import leaves in the takes folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LegacyLink {
    pub path: PathBuf,
    pub container: TakeContainer,
    pub rate_hz: u32,
    pub duration: Ms,
}

impl LegacyLink {
    fn to_text(&self) -> String {
        meta::encode(&[
            ("path", self.path.to_string_lossy().into_owned()),
            (
                "container",
                match self.container {
                    TakeContainer::Wav => "wav",
                    TakeContainer::Flac => "flac",
                    TakeContainer::LegacyWebm => "webm",
                }
                .to_owned(),
            ),
            ("rate_hz", self.rate_hz.to_string()),
            ("duration_ms", self.duration.0.to_string()),
        ])
    }

    fn from_text(text: &str) -> Option<Self> {
        let fields = meta::decode(text);
        Some(Self {
            path: PathBuf::from(fields.get("path")?),
            container: match *fields.get("container")? {
                "wav" => TakeContainer::Wav,
                "flac" => TakeContainer::Flac,
                "webm" => TakeContainer::LegacyWebm,
                _ => return None,
            },
            rate_hz: fields.get("rate_hz")?.parse().ok()?,
            duration: Ms(fields.get("duration_ms")?.parse().ok()?),
        })
    }

    fn info(&self) -> TakeInfo {
        TakeInfo {
            container: self.container,
            rate_hz: self.rate_hz,
            duration: self.duration,
        }
    }
}

enum Located {
    Legacy(LegacyLink),
    Flac(PathBuf),
    Wav(PathBuf),
}

#[derive(Clone, Debug)]
pub struct FsTakes {
    dir: PathBuf,
}

impl FsTakes {
    /// `dir` is created when the first take begins.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn file(&self, id: &RecordingId, ext: &str) -> PathBuf {
        self.dir.join(format!("{id}.{ext}"))
    }

    fn legacy_link(&self, id: &RecordingId) -> Result<Option<LegacyLink>, TakeError> {
        match std::fs::read_to_string(self.file(id, "legacy")) {
            Ok(text) => LegacyLink::from_text(&text).map(Some).ok_or_else(|| {
                TakeError::Decode(format!("the legacy pointer for {id} is unreadable"))
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io_err(e)),
        }
    }

    fn locate(&self, id: &RecordingId) -> Result<Option<Located>, TakeError> {
        if let Some(link) = self.legacy_link(id)? {
            return Ok(Some(Located::Legacy(link)));
        }
        let flac = self.file(id, "flac");
        if flac.is_file() {
            return Ok(Some(Located::Flac(flac)));
        }
        let wav = self.file(id, "wav");
        Ok(wav.is_file().then_some(Located::Wav(wav)))
    }

    fn layout(&self, id: &RecordingId) -> Option<ChannelLayout> {
        let text = std::fs::read_to_string(self.file(id, "meta")).ok()?;
        meta::layout_from_text(&text)
    }

    /// Points `id` at an audio file in the old app's folder. Nothing is copied. Writing the same
    /// pointer again changes nothing on disk.
    pub fn link_legacy(
        &self,
        id: &RecordingId,
        audio: &Path,
        container: TakeContainer,
        rate_hz: u32,
        duration: Ms,
    ) -> Result<TakeInfo, TakeError> {
        if !audio.is_file() {
            return Err(TakeError::Missing(id.clone()));
        }
        if self.file(id, "wav").exists() || self.file(id, "flac").exists() {
            return Err(TakeError::Exists(id.clone()));
        }
        let link = LegacyLink {
            path: audio.to_owned(),
            container,
            rate_hz,
            duration,
        };
        if self.legacy_link(id).ok().flatten().as_ref() != Some(&link) {
            std::fs::create_dir_all(&self.dir).map_err(io_err)?;
            meta::write_atomic(&self.file(id, "legacy"), &link.to_text()).map_err(io_err)?;
        }
        Ok(link.info())
    }

    fn wav_info(path: &Path) -> Result<TakeInfo, TakeError> {
        let mut file = File::open(path).map_err(io_err)?;
        let header = pcm::read_own_wav_header(&mut file)?;
        let frames = pcm::own_wav_frames(&file, &header)?;
        Ok(TakeInfo {
            container: TakeContainer::Wav,
            rate_hz: header.rate_hz,
            duration: Ms::from_frames(frames, nonzero(header.rate_hz)?),
        })
    }

    fn flac_info(path: &Path) -> Result<TakeInfo, TakeError> {
        let reader = FlacReader::open(path)?;
        Ok(TakeInfo {
            container: TakeContainer::Flac,
            rate_hz: reader.rate_hz(),
            duration: Ms::from_frames(
                reader.total_frames().unwrap_or(0),
                nonzero(reader.rate_hz())?,
            ),
        })
    }

    /// Opens whatever holds the samples. A legacy WebM is decoded by ffmpeg into `scratch` first.
    fn open_reader(
        &self,
        id: &RecordingId,
        scratch: &Path,
    ) -> Result<(Box<dyn FrameReader>, Option<ChannelLayout>), TakeError> {
        let located = self
            .locate(id)?
            .ok_or_else(|| TakeError::Missing(id.clone()))?;
        Ok(match located {
            Located::Flac(path) => (Box::new(FlacReader::open(&path)?), self.layout(id)),
            Located::Wav(path) => (Box::new(OwnWavReader::open(&path)?), self.layout(id)),
            Located::Legacy(link) => {
                if !link.path.is_file() {
                    return Err(TakeError::Missing(id.clone()));
                }
                let reader: Box<dyn FrameReader> = match link.container {
                    TakeContainer::Wav => Box::new(HoundReader::open(&link.path)?),
                    TakeContainer::Flac => Box::new(FlacReader::open(&link.path)?),
                    TakeContainer::LegacyWebm => {
                        let decoded = scratch.join("source.wav");
                        ffmpeg::decode_to_wav(&link.path, &decoded)?;
                        Box::new(HoundReader::open(&decoded)?)
                    }
                };
                (reader, Some(ChannelLayout::Mixed))
            }
        })
    }

    fn seal_wav(&self, id: &RecordingId, wav: &Path) -> Result<TakeInfo, TakeError> {
        let flac = self.file(id, "flac");
        let tmp = self.file(id, "flac.tmp");
        let mut reader = OwnWavReader::open(wav)?;
        let (channels, rate_hz, frames) =
            (reader.channels(), reader.rate_hz(), reader.total_frames());

        if flac.is_file() {
            let consistent = FlacReader::open(&flac)
                .map(|f| {
                    f.total_frames().unwrap_or(0) == frames
                        && f.rate_hz() == rate_hz
                        && f.channels() == channels
                })
                .unwrap_or(false);
            if consistent {
                remove_if_present(wav)?;
                return Self::flac_info(&flac);
            }
            remove_if_present(&flac)?;
        }

        remove_if_present(&tmp)?;
        let digest = flac::encode(&mut reader, &tmp)?;
        if let Err(e) = flac::verify(&tmp, digest, channels, rate_hz) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        std::fs::rename(&tmp, &flac).map_err(io_err)?;
        if let Ok(dir) = File::open(&self.dir) {
            let _ = dir.sync_all();
        }
        remove_if_present(wav)?;
        Self::flac_info(&flac)
    }

    /// Converts `source` into this id's FLAC, by its extension. The caller has claimed the id.
    fn convert(&self, id: &RecordingId, source: &Path) -> Result<TakeInfo, TakeError> {
        let ext = source
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        match ext.as_str() {
            "flac" => {
                let reader = FlacReader::open(source)?;
                let (rate_hz, frames) = (reader.rate_hz(), reader.total_frames());
                drop(reader);
                let tmp = self.file(id, "flac.tmp");
                std::fs::copy(source, &tmp).map_err(io_err)?;
                std::fs::rename(&tmp, self.file(id, "flac")).map_err(io_err)?;
                let duration = Ms::from_frames(frames.unwrap_or(0), nonzero(rate_hz)?);
                Ok(TakeInfo {
                    container: TakeContainer::Flac,
                    rate_hz,
                    duration,
                })
            }
            "wav" => self.import_wav_samples(id, Box::new(HoundReader::open(source)?)),
            _ => {
                let scratch = tempfile::Builder::new()
                    .prefix("dory-import-")
                    .tempdir()
                    .map_err(io_err)?;
                let decoded = scratch.path().join("source.wav");
                ffmpeg::decode_to_wav(source, &decoded)?;
                self.import_wav_samples(id, Box::new(HoundReader::open(&decoded)?))
            }
        }
    }

    fn import_wav_samples(
        &self,
        id: &RecordingId,
        mut source: Box<dyn FrameReader>,
    ) -> Result<TakeInfo, TakeError> {
        let partial = self.file(id, "wav.partial");
        let wav = self.file(id, "wav");
        remove_if_present(&partial)?;
        let channels = source.channels();
        let mut take = WavTake::create(&partial, 1, source.rate_hz())?;
        let mut pcm = Vec::new();
        let mut mono: Vec<i16> = Vec::new();
        while source.read_frames(&mut pcm, 16_384)? > 0 {
            mono.clear();
            mono.extend(pcm.chunks_exact(channels).map(|frame| {
                let sum: i32 = frame.iter().map(|s| i32::from(*s)).sum();
                (sum / channels as i32) as i16
            }));
            take.write_i16(&mono)?;
        }
        take.finish_info()?;
        std::fs::rename(&partial, &wav).map_err(io_err)?;
        self.seal_wav(id, &wav)
    }
}

impl Takes for FsTakes {
    fn begin(
        &self,
        id: &RecordingId,
        layout: ChannelLayout,
        rate_hz: u32,
    ) -> Result<Box<dyn TakeWriter>, TakeError> {
        std::fs::create_dir_all(&self.dir).map_err(io_err)?;
        if self.locate(id)?.is_some() {
            return Err(TakeError::Exists(id.clone()));
        }
        meta::write_atomic(&self.file(id, "meta"), &meta::layout_text(layout)).map_err(io_err)?;
        match WavTake::create(&self.file(id, "wav"), layout.channel_count(), rate_hz) {
            Ok(take) => Ok(Box::new(take)),
            Err(TakeError::Io(e)) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(TakeError::Exists(id.clone()))
            }
            Err(e) => Err(e),
        }
    }

    fn seal(&self, id: &RecordingId) -> Result<TakeInfo, TakeError> {
        match self.locate(id)? {
            None => Err(TakeError::Missing(id.clone())),
            Some(Located::Legacy(link)) => Ok(link.info()),
            Some(Located::Flac(flac)) => {
                let wav = self.file(id, "wav");
                if wav.is_file() {
                    self.seal_wav(id, &wav)
                } else {
                    Self::flac_info(&flac)
                }
            }
            Some(Located::Wav(wav)) => self.seal_wav(id, &wav),
        }
    }

    fn clip(
        &self,
        id: &RecordingId,
        channel: Channel,
        span: Option<TimeRange>,
    ) -> Result<Clip, TakeError> {
        let scratch = tempfile::Builder::new()
            .prefix("dory-clip-")
            .tempdir()
            .map_err(io_err)?;
        let (mut reader, layout) = self.open_reader(id, scratch.path())?;
        let channels = reader.channels();
        let rate_hz = reader.rate_hz();
        let pick = Pick::new(layout, channels, channel)?;

        let to_frames = |ms: Ms| {
            u64::try_from(u128::from(ms.0) * u128::from(rate_hz) / 1000).unwrap_or(u64::MAX)
        };
        let (origin, mut remaining) = match span {
            Some(range) => {
                reader.skip_frames(to_frames(range.start()))?;
                (range.start(), Some(to_frames(range.len())))
            }
            None => (Ms(0), None),
        };

        let path = scratch
            .path()
            .join(format!("{id}-{channel:?}.wav").to_lowercase());
        let mut writer = hound::WavWriter::create(
            &path,
            hound::WavSpec {
                channels: 1,
                sample_rate: TARGET_RATE_HZ,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .map_err(pcm::hound_err)?;
        let mut down = Downsampler::new(rate_hz)?;
        let mut vad = Vad::new(TARGET_RATE_HZ);
        let (mut interleaved, mut mono, mut resampled) = (Vec::new(), Vec::new(), Vec::new());
        let mut out_frames = 0u64;
        let mut emit =
            |samples: &mut Vec<i16>, writer: &mut hound::WavWriter<_>| -> Result<(), TakeError> {
                for s in samples.drain(..) {
                    writer.write_sample(s).map_err(pcm::hound_err)?;
                    out_frames += 1;
                }
                Ok(())
            };
        loop {
            let want = remaining.map_or(8192, |r| usize::try_from(r.min(8192)).unwrap_or(8192));
            if want == 0 {
                break;
            }
            let got = reader.read_frames(&mut interleaved, want)?;
            if got == 0 {
                break;
            }
            if let Some(r) = &mut remaining {
                *r -= got as u64;
            }
            pick.mono(&interleaved, channels, &mut mono);
            down.feed(&mono, &mut resampled)?;
            vad.feed(&resampled);
            emit(&mut resampled, &mut writer)?;
        }
        down.finish(&mut resampled)?;
        vad.feed(&resampled);
        emit(&mut resampled, &mut writer)?;
        writer.finalize().map_err(pcm::hound_err)?;

        let duration = Ms::from_frames(out_frames, nonzero(TARGET_RATE_HZ)?);
        Ok(Clip::new(
            path,
            channel,
            duration,
            origin,
            Activity::Speech(vad.finish(duration)),
            Some(Box::new(scratch)),
        ))
    }

    fn slice(&self, whole: &Clip, span: TimeRange) -> Result<Clip, TakeError> {
        let end_of_whole = Ms(whole.origin.0 + whole.duration.0);
        let start = span.start().clamp(whole.origin, end_of_whole);
        let end = span.end().clamp(start, end_of_whole);
        let to_frames =
            |ms: Ms| u32::try_from(ms.0 * u64::from(TARGET_RATE_HZ) / 1000).unwrap_or(u32::MAX);
        let (first, count) = (
            to_frames(Ms(start.0 - whole.origin.0)),
            to_frames(Ms(end.0 - start.0)),
        );

        let mut reader = hound::WavReader::open(&whole.path).map_err(pcm::hound_err)?;
        let spec = reader.spec();
        if (spec.channels, spec.sample_rate, spec.bits_per_sample) != (1, TARGET_RATE_HZ, 16) {
            return Err(TakeError::Decode(format!(
                "{} is not a clip from this adapter",
                whole.path.display()
            )));
        }
        reader.seek(first).map_err(io_err)?;
        let scratch = tempfile::Builder::new()
            .prefix("dory-slice-")
            .tempdir()
            .map_err(io_err)?;
        let path = scratch.path().join(
            whole
                .path
                .file_name()
                .map_or_else(|| "slice.wav".into(), |n| n.to_owned()),
        );
        let mut writer = hound::WavWriter::create(&path, spec).map_err(pcm::hound_err)?;
        let mut written = 0u64;
        for sample in reader.samples::<i16>().take(count as usize) {
            writer
                .write_sample(sample.map_err(pcm::hound_err)?)
                .map_err(pcm::hound_err)?;
            written += 1;
        }
        writer.finalize().map_err(pcm::hound_err)?;
        let local = TimeRange::new(Ms(start.0 - whole.origin.0), Ms(end.0 - whole.origin.0))
            .ok_or_else(|| TakeError::Decode(format!("{span:?} is not a span")))?;
        Ok(Clip::new(
            path,
            whole.channel,
            Ms::from_frames(written, nonzero(TARGET_RATE_HZ)?),
            start,
            whole.activity.within(local),
            Some(Box::new(scratch)),
        ))
    }

    fn orphans(&self) -> Result<Vec<RecordingId>, TakeError> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io_err(e)),
        };
        let mut ids = Vec::new();
        for entry in entries {
            let path = entry.map_err(io_err)?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("wav") {
                continue;
            }
            if let Some(id) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| RecordingId::parse(s).ok())
            {
                ids.push(id);
            }
        }
        ids.sort();
        Ok(ids)
    }

    fn stored(&self) -> Result<Vec<RecordingId>, TakeError> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io_err(e)),
        };
        let mut ids = std::collections::BTreeSet::new();
        for entry in entries {
            let name = entry.map_err(io_err)?.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let id = FILES.iter().find_map(|suffix| {
                let stem = name.strip_suffix(suffix)?.strip_suffix('.')?;
                RecordingId::parse(stem).ok()
            });
            ids.extend(id);
        }
        Ok(ids.into_iter().collect())
    }

    fn inspect(&self, id: &RecordingId) -> Result<Option<TakeInfo>, TakeError> {
        match self.locate(id)? {
            None => Ok(None),
            Some(Located::Legacy(link)) => Ok(link.path.is_file().then(|| link.info())),
            Some(Located::Flac(path)) => Self::flac_info(&path).map(Some),
            Some(Located::Wav(path)) => Self::wav_info(&path).map(Some),
        }
    }

    fn import(&self, id: &RecordingId, source: &Path) -> Result<TakeInfo, TakeError> {
        std::fs::create_dir_all(&self.dir).map_err(io_err)?;
        if self.locate(id)?.is_some() {
            return Err(TakeError::Exists(id.clone()));
        }
        if !source.is_file() {
            return Err(TakeError::Missing(id.clone()));
        }
        meta::write_atomic(
            &self.file(id, "meta"),
            &meta::layout_text(ChannelLayout::Mixed),
        )
        .map_err(io_err)?;
        self.convert(id, source)
    }

    fn legacy_original(&self, id: &RecordingId) -> Result<Option<PathBuf>, TakeError> {
        Ok(self.legacy_link(id)?.map(|link| link.path))
    }

    /// Everything the pointer's removal waits on is a complete file: `flac` and `wav` only appear
    /// by rename, so a retry finishes what it finds instead of converting again.
    fn adopt(&self, id: &RecordingId) -> Result<TakeInfo, TakeError> {
        let Some(link) = self.legacy_link(id)? else {
            return self
                .inspect(id)?
                .ok_or_else(|| TakeError::Missing(id.clone()));
        };
        let (flac, wav) = (self.file(id, "flac"), self.file(id, "wav"));
        let info = if wav.is_file() {
            self.seal_wav(id, &wav)?
        } else if flac.is_file() {
            Self::flac_info(&flac)?
        } else {
            if !link.path.is_file() {
                return Err(TakeError::Missing(id.clone()));
            }
            meta::write_atomic(
                &self.file(id, "meta"),
                &meta::layout_text(ChannelLayout::Mixed),
            )
            .map_err(io_err)?;
            self.convert(id, &link.path)?
        };
        remove_if_present(&self.file(id, "legacy"))?;
        remove_if_present(&self.file(id, "flac.tmp"))?;
        remove_if_present(&self.file(id, "wav.partial"))?;
        Ok(info)
    }

    fn playback_path(&self, id: &RecordingId) -> Result<PathBuf, TakeError> {
        match self.locate(id)? {
            None => Err(TakeError::Missing(id.clone())),
            Some(Located::Legacy(link)) if link.path.is_file() => Ok(link.path),
            Some(Located::Legacy(_)) => Err(TakeError::Missing(id.clone())),
            Some(Located::Flac(path) | Located::Wav(path)) => Ok(path),
        }
    }

    /// Removes this crate's files for `id`. The audio a legacy pointer refers to is never touched.
    fn delete(&self, id: &RecordingId) -> Result<(), TakeError> {
        for suffix in FILES {
            remove_if_present(&self.file(id, suffix))?;
        }
        Ok(())
    }

    fn files(&self, id: &RecordingId) -> Result<Vec<PathBuf>, TakeError> {
        Ok(FILES
            .iter()
            .map(|suffix| self.file(id, suffix))
            .filter(|path| path.exists())
            .collect())
    }
}
