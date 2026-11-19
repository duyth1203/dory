//! Scores speech recognition on Tagalog and English audio: whole windows, 6-second chunks, and
//! whole windows with the speech they leave without text decoded again.
//!
//! `cargo run --release -p dory-engines --example bilingual-bench -- score <folders...>
//!  [--modes auto,tl,en,chunks,covered] [--model whisper-turbo] [--models DIR] [--threads 4]
//!  [--languages tl,en] [--dict /usr/share/dict/words] [--verbose]`
//!
//! A folder holds 16-bit mono WAVs, a `refs.tsv` of `file<TAB>reference text`, and for clips that
//! switch language between turns a `langs.tsv` of `file<TAB>part part ...`. A part is
//! `<tag>:<start ms>-<end ms>:<reference words>`, and the parts' word counts split the clip's
//! reference text in order. Every mode goes through the `Transcriber` port, one clip per call.
//!
//! `covered` is the app's recovery rule. The clip goes through the takes adapter, which measures
//! its speech the way the app does, then to the engine with sentence times. The stretches that
//! `windows::recover` finds go again on Auto, round after round, and `stages::settle_language`
//! redoes a window or stretch heard outside `--languages` pinned to the first of them. After the table it prints how
//! many clips needed a re-decode.
//!
//! The scoring rules port `docs/plans/bilingual-spike/score.py`. A reference and an output are
//! lowercased and split at anything that is not a letter, digit, or apostrophe, so hyphens split
//! words. Word errors are the edit distance over words. A word is English when it is in the
//! system dictionary, or a dictionary word with a suffix removed, and not in the Tagalog
//! function-word list. Every other word counts as Tagalog.
//!
//! - Kept: the share of the reference's Tagalog or English words found in the output, as bags.
//! - English-only: clips whose reference is at least 20% Tagalog and whose output holds under half
//!   that share.
//! - Dropped turns: parts of clips with two or more parts whose output keeps under half of the
//!   part's reference words.
//! - Words on silence: for clips with at most one part, the output words of any window or chunk
//!   none of whose words line up with a reference word.
//! - Wrong language: the share of windows or chunks, among those whose engine reported a
//!   language, heard as a language other than the one holding most of their time. A clip without
//!   parts is wrong only when the heard language is not a meeting language. Until an engine
//!   reports a language the column prints `-`.
//! - RTFx: audio seconds per second spent inside `transcribe`, after one untimed warm-up call.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dory_core::app::stages::windows::recover;
use dory_core::app::stages::{Settle, settle_language};
use dory_core::domain::{Channel, Fraction, LangTag, LanguagePref, Ms, RecordingId, TimeRange};
use dory_core::models::Availability;
use dory_core::ports::{
    Activity, CancelToken, Clip as EngineClip, EngineError, ModelProvider, RawSegment, SegmentSink,
    Takes, TranscribeRequest, Transcriber,
};
use dory_engines::Sherpa;
use dory_store::FsTakes;

const TAGALOG_FUNCTION_WORDS: &str = "
ang ng mga sa na ay at si ni kay ko mo niya namin natin nila ako ikaw siya kami tayo kayo sila
ito iyan iyon dito diyan doon hindi oo po opo din rin lang lamang pa naman nga kasi dahil kung
kapag para pero ngunit subalit bagaman kaya upang nang may mayroon wala walang isang isa dalawa
ano sino saan kailan bakit paano yung yun yan ba daw raw talaga sobrang mas pinaka kanya kaniyang
kanilang aming ating inyong kanila amin atin inyo akin iyo kanino ating nasa noon ngayon bago
mula hanggang tungkol laban gayon gayundin dapat kailangan maaari puwede pwede siguro baka
";

const ENGLISH_SUFFIXES: [&str; 6] = ["s", "es", "ed", "ing", "ly", "'s"];

/// Spike default: cut a window at the quietest 20 ms frame once it passes half of this.
const CHUNK_MS: u64 = 6_000;
const FRAME_MS: u64 = 20;

/// One stretch of a clip spoken in one language.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Part {
    language: LangTag,
    range: TimeRange,
    /// Whitespace-separated words of the clip's reference text that belong to this part.
    words: usize,
}

/// One WAV with its reference text and, for turn-switch clips, the language of each part.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Clip {
    path: PathBuf,
    name: String,
    reference: String,
    parts: Vec<Part>,
}

impl Clip {
    /// The reference text of each part, or the whole text when the clip has no parts.
    fn part_texts(&self) -> Vec<String> {
        if self.parts.is_empty() {
            return vec![self.reference.clone()];
        }
        let tokens: Vec<&str> = self.reference.split_whitespace().collect();
        let mut at = 0;
        self.parts
            .iter()
            .map(|p| {
                let text = tokens[at..at + p.words].join(" ");
                at += p.words;
                text
            })
            .collect()
    }
}

/// One recognition rule under test.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Mode {
    /// The whole clip in one call, the engine picking the language.
    Auto,
    /// The whole clip in one call, the language pinned.
    Pinned(LangTag),
    /// Cuts of at most this long at the quietest frame, each decoded on Auto.
    Chunks(Ms),
    /// The whole clip on Auto with sentence times, then each uncovered stretch of speech on Auto.
    Covered,
}

impl Mode {
    fn parse(text: &str) -> Result<Mode, String> {
        match text {
            "auto" => Ok(Mode::Auto),
            "covered" => Ok(Mode::Covered),
            "chunks" => Ok(Mode::Chunks(Ms(CHUNK_MS))),
            other => match other.strip_prefix("chunks=") {
                Some(secs) => secs
                    .parse::<f32>()
                    .ok()
                    .filter(|s| *s >= 1.0)
                    .map(|s| Mode::Chunks(Ms((s * 1000.0) as u64)))
                    .ok_or_else(|| format!("chunks=<seconds> wants at least 1, got {secs:?}")),
                None => LangTag::parse(other).map(Mode::Pinned).map_err(|_| {
                    format!("{other:?} is not auto, covered, chunks, or a language tag")
                }),
            },
        }
    }

    fn label(&self) -> String {
        match self {
            Mode::Auto => "auto".into(),
            Mode::Pinned(tag) => format!("pinned {tag}"),
            Mode::Chunks(ms) => format!("chunks {} s", ms.0 as f32 / 1000.0),
            Mode::Covered => "covered".into(),
        }
    }
}

/// What one engine call produced.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Unit {
    span: TimeRange,
    text: String,
    heard: Option<LangTag>,
}

struct Output {
    units: Vec<Unit>,
    busy: Duration,
    /// Stretches decoded again, in `covered`.
    stretches: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Score {
    clips: usize,
    ref_words: usize,
    errors: usize,
    tagalog_hit: usize,
    tagalog_total: usize,
    english_hit: usize,
    english_total: usize,
    english_only: usize,
    mixed_parts: usize,
    dropped_turns: usize,
    extra_words_on_silence: usize,
    heard_windows: usize,
    wrong_language: usize,
    audio_ms: u64,
    busy_ms: u64,
    clips_redone: usize,
    stretches: usize,
}

impl Score {
    fn add(&mut self, other: &Score) {
        self.clips += other.clips;
        self.ref_words += other.ref_words;
        self.errors += other.errors;
        self.tagalog_hit += other.tagalog_hit;
        self.tagalog_total += other.tagalog_total;
        self.english_hit += other.english_hit;
        self.english_total += other.english_total;
        self.english_only += other.english_only;
        self.mixed_parts += other.mixed_parts;
        self.dropped_turns += other.dropped_turns;
        self.extra_words_on_silence += other.extra_words_on_silence;
        self.heard_windows += other.heard_windows;
        self.wrong_language += other.wrong_language;
        self.audio_ms += other.audio_ms;
        self.busy_ms += other.busy_ms;
        self.clips_redone += other.clips_redone;
        self.stretches += other.stretches;
    }
}

struct Lexicon {
    english: HashSet<String>,
    tagalog_function: HashSet<String>,
}

impl Lexicon {
    fn new(dictionary: impl IntoIterator<Item = String>) -> Lexicon {
        Lexicon {
            english: dictionary
                .into_iter()
                .map(|w| w.trim().to_lowercase())
                .collect(),
            tagalog_function: TAGALOG_FUNCTION_WORDS
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
        }
    }

    fn load(path: &Path) -> Result<Lexicon, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read the dictionary {}: {e}", path.display()))?;
        Ok(Lexicon::new(text.lines().map(str::to_owned)))
    }

    fn is_english(&self, word: &str) -> bool {
        if self.tagalog_function.contains(word) {
            return false;
        }
        if self.english.contains(word) {
            return true;
        }
        ENGLISH_SUFFIXES.iter().any(|suffix| {
            word.strip_suffix(suffix)
                .is_some_and(|stem| self.english.contains(stem))
        })
    }

    fn is_tagalog(&self, word: &str) -> bool {
        !self.is_english(word)
    }
}

fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '\'' {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

/// The word edit distance, and for each hypothesis word whether it lines up with a reference
/// word. A tie prefers a match, then an insertion, so a stray word at the end counts as extra
/// instead of as a substitution for the last reference word.
fn align(reference: &[String], hypothesis: &[String]) -> (usize, Vec<bool>) {
    let (n, m) = (reference.len(), hypothesis.len());
    let mut cost = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in cost.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, c) in cost[0].iter_mut().enumerate() {
        *c = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let sub = cost[i - 1][j - 1] + usize::from(reference[i - 1] != hypothesis[j - 1]);
            cost[i][j] = sub.min(cost[i][j - 1] + 1).min(cost[i - 1][j] + 1);
        }
    }
    let mut aligned = vec![false; m];
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        if i > 0
            && j > 0
            && reference[i - 1] == hypothesis[j - 1]
            && cost[i][j] == cost[i - 1][j - 1]
        {
            aligned[j - 1] = true;
            i -= 1;
            j -= 1;
        } else if j > 0 && cost[i][j] == cost[i][j - 1] + 1 {
            j -= 1;
        } else if i > 0 && cost[i][j] == cost[i - 1][j] + 1 {
            i -= 1;
        } else {
            aligned[j - 1] = true;
            i -= 1;
            j -= 1;
        }
    }
    (cost[n][m], aligned)
}

/// Reference words of `pick` found in `hypothesis`, as bags, and how many the reference has.
fn recall(
    reference: &[String],
    hypothesis: &[String],
    pick: impl Fn(&str) -> bool,
) -> (usize, usize) {
    let mut have: HashMap<&str, usize> = HashMap::new();
    for w in hypothesis {
        *have.entry(w).or_default() += 1;
    }
    let mut hit = 0;
    let mut total = 0;
    for w in reference.iter().filter(|w| pick(w)) {
        total += 1;
        if let Some(n) = have.get_mut(w.as_str()).filter(|n| **n > 0) {
            *n -= 1;
            hit += 1;
        }
    }
    (hit, total)
}

fn share(count: usize, of: usize) -> f64 {
    count as f64 / of.max(1) as f64
}

fn truth_language(parts: &[Part], span: TimeRange) -> Option<&LangTag> {
    parts
        .iter()
        .map(|p| (p.range.overlap(&span), &p.language))
        .filter(|(overlap, _)| *overlap > Ms(0))
        .max_by_key(|(overlap, _)| *overlap)
        .map(|(_, language)| language)
}

fn score_clip(
    clip: &Clip,
    output: &Output,
    lexicon: &Lexicon,
    meeting: &[LangTag],
    audio: Ms,
) -> Score {
    let reference = words(&clip.reference);
    let texts: Vec<Vec<String>> = output.units.iter().map(|u| words(&u.text)).collect();
    let hypothesis: Vec<String> = texts.iter().flatten().cloned().collect();
    let (errors, aligned) = align(&reference, &hypothesis);
    let (tagalog_hit, tagalog_total) = recall(&reference, &hypothesis, |w| lexicon.is_tagalog(w));
    let (english_hit, english_total) = recall(&reference, &hypothesis, |w| lexicon.is_english(w));

    let tagalog_share = |ws: &[String]| {
        share(
            ws.iter().filter(|w| lexicon.is_tagalog(w)).count(),
            ws.len(),
        )
    };
    let (want, got) = (tagalog_share(&reference), tagalog_share(&hypothesis));
    let english_only = usize::from(want >= 0.2 && got < want / 2.0);

    let (mut mixed_parts, mut dropped_turns) = (0, 0);
    if clip.parts.len() >= 2 {
        for text in clip.part_texts() {
            let part = words(&text);
            let (hit, total) = recall(&part, &hypothesis, |_| true);
            if total > 0 {
                mixed_parts += 1;
                dropped_turns += usize::from(hit * 2 < total);
            }
        }
    }

    let mut extra_words_on_silence = 0;
    if clip.parts.len() <= 1 {
        let mut at = 0;
        for unit in &texts {
            let line = &aligned[at..at + unit.len()];
            if !unit.is_empty() && line.iter().all(|a| !a) {
                extra_words_on_silence += unit.len();
            }
            at += unit.len();
        }
    }

    let (mut heard_windows, mut wrong_language) = (0, 0);
    for unit in &output.units {
        let Some(heard) = &unit.heard else { continue };
        let wrong = if clip.parts.is_empty() {
            !meeting.iter().any(|m| m.primary() == heard.primary())
        } else {
            match truth_language(&clip.parts, unit.span) {
                Some(truth) => truth.primary() != heard.primary(),
                None => continue,
            }
        };
        heard_windows += 1;
        wrong_language += usize::from(wrong);
    }

    Score {
        clips: 1,
        ref_words: reference.len(),
        errors,
        tagalog_hit,
        tagalog_total,
        english_hit,
        english_total,
        english_only,
        mixed_parts,
        dropped_turns,
        extra_words_on_silence,
        heard_windows,
        wrong_language,
        audio_ms: audio.0,
        busy_ms: output.busy.as_millis() as u64,
        clips_redone: usize::from(output.stretches > 0),
        stretches: output.stretches,
    }
}

fn pct(part: usize, whole: usize) -> String {
    if whole == 0 {
        "-".into()
    } else {
        format!("{:.1}%", 100.0 * part as f64 / whole as f64)
    }
}

fn row(mode: &Mode, s: &Score) -> String {
    let wrong = if s.heard_windows == 0 {
        "-".to_owned()
    } else {
        format!("{}/{}", s.wrong_language, s.heard_windows)
    };
    let rtfx = if s.busy_ms == 0 {
        "-".to_owned()
    } else {
        format!("{:.1}", s.audio_ms as f64 / s.busy_ms as f64)
    };
    format!(
        "{} | {} | {:.0}% | {:.0}% | {}/{} | {}/{} | {} | {} | {}",
        mode.label(),
        pct(s.errors, s.ref_words),
        100.0 * share(s.tagalog_hit, s.tagalog_total),
        100.0 * share(s.english_hit, s.english_total),
        s.english_only,
        s.clips,
        s.dropped_turns,
        s.mixed_parts,
        s.extra_words_on_silence,
        wrong,
        rtfx
    )
}

const HEADER: &str = "mode | WER | Tagalog words kept | English words kept | English-only clips | dropped turns | words on silence | wrong language | RTFx";

fn parse_refs(refs: &str) -> Result<Vec<(String, String)>, String> {
    refs.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, line)| {
            line.split_once('\t')
                .map(|(f, t)| (f.to_owned(), t.to_owned()))
                .ok_or_else(|| format!("refs.tsv line {}: expected file<TAB>text", i + 1))
        })
        .collect()
}

fn parse_part(text: &str) -> Result<Part, String> {
    let bad = || format!("part {text:?} is not <tag>:<start ms>-<end ms>:<words>");
    let mut fields = text.split(':');
    let (Some(tag), Some(range), Some(count), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return Err(bad());
    };
    let (start, end) = range.split_once('-').ok_or_else(bad)?;
    let ms = |s: &str| s.parse::<u64>().map(Ms).map_err(|_| bad());
    Ok(Part {
        language: LangTag::parse(tag).map_err(|_| bad())?,
        range: TimeRange::new(ms(start)?, ms(end)?).ok_or_else(bad)?,
        words: count.parse().map_err(|_| bad())?,
    })
}

fn parse_clips(dir: &Path, refs: &str, langs: Option<&str>) -> Result<Vec<Clip>, String> {
    let mut parts: HashMap<String, Vec<Part>> = HashMap::new();
    for (i, line) in langs.unwrap_or_default().lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let (file, rest) = line
            .split_once('\t')
            .ok_or_else(|| format!("langs.tsv line {}: expected file<TAB>parts", i + 1))?;
        let list = rest
            .split_whitespace()
            .map(parse_part)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("langs.tsv line {}: {e}", i + 1))?;
        parts.insert(file.to_owned(), list);
    }
    let mut clips = Vec::new();
    for (file, reference) in parse_refs(refs)? {
        let list = parts.remove(&file).unwrap_or_default();
        let counted: usize = list.iter().map(|p| p.words).sum();
        let tokens = reference.split_whitespace().count();
        if !list.is_empty() && counted != tokens {
            return Err(format!(
                "{file}: langs.tsv counts {counted} words but refs.tsv has {tokens}"
            ));
        }
        clips.push(Clip {
            path: dir.join(&file),
            name: file,
            reference,
            parts: list,
        });
    }
    if let Some(orphan) = parts.keys().next() {
        return Err(format!("langs.tsv names {orphan}, which refs.tsv does not"));
    }
    Ok(clips)
}

fn load_folder(dir: &Path) -> Result<Vec<Clip>, String> {
    let read = |name: &str| std::fs::read_to_string(dir.join(name));
    let refs = read("refs.tsv").map_err(|e| format!("{}: refs.tsv: {e}", dir.display()))?;
    let langs = read("langs.tsv").ok();
    let clips = parse_clips(dir, &refs, langs.as_deref())?;
    for clip in &clips {
        if !clip.path.is_file() {
            return Err(format!(
                "{} is in refs.tsv but missing",
                clip.path.display()
            ));
        }
    }
    Ok(clips)
}

#[derive(Debug)]
struct Wav {
    rate: u32,
    samples: Vec<i16>,
}

impl Wav {
    fn ms(&self, samples: usize) -> Ms {
        Ms(samples as u64 * 1000 / u64::from(self.rate))
    }

    fn parse(bytes: &[u8]) -> Result<Wav, String> {
        if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
            return Err("not a RIFF WAVE file".into());
        }
        let le32 = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"));
        let le16 = |at: usize| u16::from_le_bytes(bytes[at..at + 2].try_into().expect("2 bytes"));
        let mut at = 12;
        let mut rate = None;
        while at + 8 <= bytes.len() {
            let id = &bytes[at..at + 4];
            let len = le32(at + 4) as usize;
            let body = at + 8;
            let end = (body + len).min(bytes.len());
            match id {
                b"fmt " if end - body >= 16 => {
                    let (format, channels, bits) = (le16(body), le16(body + 2), le16(body + 14));
                    if format != 1 || channels != 1 || bits != 16 {
                        return Err(format!(
                            "want 16-bit mono PCM, got format {format}, {channels} channels, {bits} bits"
                        ));
                    }
                    rate = Some(le32(body + 4));
                }
                b"data" => {
                    let rate = rate.ok_or("data chunk before fmt chunk")?;
                    let samples = bytes[body..end]
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|b| i16::from_le_bytes(*b))
                        .collect();
                    return Ok(Wav { rate, samples });
                }
                _ => {}
            }
            at = body + len + (len & 1);
        }
        Err("no data chunk".into())
    }

    fn encode(rate: u32, samples: &[i16]) -> Vec<u8> {
        let data = samples.len() as u32 * 2;
        let mut out = Vec::with_capacity(44 + data as usize);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * 2).to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data.to_le_bytes());
        for s in samples {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }
}

/// Cuts at the quietest frame inside each stretch once it passes half of `max`, so a chunk ends
/// at a pause when one exists and never runs past `max`.
fn chunk_cuts(samples: &[i16], rate: u32, max: Ms) -> Vec<(usize, usize)> {
    let frame = (u64::from(rate) * FRAME_MS / 1000) as usize;
    let max = (u64::from(rate) * max.0 / 1000) as usize;
    let energy = |a: usize| {
        let b = (a + frame).min(samples.len());
        samples[a..b]
            .iter()
            .map(|x| {
                let x = f32::from(*x) / 32768.0;
                x * x
            })
            .sum::<f32>()
            / (b - a).max(1) as f32
    };
    let mut out = Vec::new();
    let mut start = 0;
    while samples.len() - start > max {
        let lo = start + max / 2;
        let hi = start + max - frame;
        let cut = (lo..hi)
            .step_by(frame)
            .min_by(|a, b| energy(*a).total_cmp(&energy(*b)))
            .unwrap_or(hi);
        out.push((start, cut));
        start = cut;
    }
    out.push((start, samples.len()));
    out
}

#[derive(Default)]
struct Collect(Vec<RawSegment>);

impl SegmentSink for Collect {
    fn progress(&mut self, _: Fraction) {}
    fn segment(&mut self, segment: RawSegment) {
        self.0.push(segment);
    }
}

struct Call {
    /// On the clip's time axis.
    segments: Vec<RawSegment>,
    heard: Option<LangTag>,
    busy: Duration,
}

impl Call {
    fn text(&self) -> String {
        joined(&self.segments)
    }
}

fn joined(segments: &[RawSegment]) -> String {
    segments
        .iter()
        .map(|s| s.text.trim())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn other(error: impl std::fmt::Display) -> EngineError {
    EngineError::Other {
        message: error.to_string(),
        source: None,
    }
}

/// A WAV on disk as one clip with no speech measured.
fn wav_clip(path: &Path, duration: Ms) -> EngineClip {
    EngineClip::new(
        path.to_owned(),
        Channel::Mixed,
        duration,
        Ms(0),
        Activity::Unmeasured,
        None,
    )
}

fn call(
    engine: &dyn Transcriber,
    clip: &EngineClip,
    language: &LanguagePref,
    threads: u8,
    sentence_times: bool,
    cancel: &CancelToken,
) -> Result<Call, EngineError> {
    let request = TranscribeRequest {
        clip,
        language: language.clone(),
        diarize: false,
        hint: None,
        threads,
        sentence_times,
        locale: None,
        vocabulary: &[],
    };
    let mut sink = Collect::default();
    let started = Instant::now();
    let outcome = engine.transcribe(&request, &mut sink, cancel)?;
    let busy = started.elapsed();
    let heard = outcome
        .detected_language
        .or_else(|| sink.0.iter().find_map(|s| s.language.clone()));
    Ok(Call {
        segments: sink.0,
        heard,
        busy,
    })
}

/// The segments of every call, each call starting at its offset into the clip, as units in time
/// order. Neighbouring segments of one call stay one unit, so a first pass that a recovered
/// stretch splits becomes two units around it.
fn in_time_order(calls: &[(Ms, Call)]) -> Vec<Unit> {
    let mut timed: Vec<(usize, TimeRange, &RawSegment)> = calls
        .iter()
        .enumerate()
        .flat_map(|(n, (offset, call))| {
            call.segments
                .iter()
                .map(move |s| (n, s.range.shifted(*offset), s))
        })
        .collect();
    timed.sort_by_key(|(_, range, _)| range.start());
    let mut units: Vec<(usize, Unit, Vec<RawSegment>)> = Vec::new();
    for (n, range, segment) in timed {
        match units.last_mut() {
            Some((last, unit, segments)) if *last == n => {
                unit.span = TimeRange::new(unit.span.start(), range.end().max(unit.span.end()))
                    .expect("sorted by start");
                segments.push(segment.clone());
            }
            _ => units.push((
                n,
                Unit {
                    span: range,
                    text: String::new(),
                    heard: calls[n].1.heard.clone(),
                },
                vec![segment.clone()],
            )),
        }
    }
    units
        .into_iter()
        .map(|(_, unit, segments)| Unit {
            text: joined(&segments),
            ..unit
        })
        .collect()
}

struct Run<'a> {
    engine: &'a dyn Transcriber,
    threads: u8,
    cancel: &'a CancelToken,
    scratch: &'a Path,
    meeting: &'a [LangTag],
    /// Names each clip `covered` imports into the takes adapter.
    covered_runs: std::cell::Cell<u32>,
}

impl Run<'_> {
    fn language(mode: &Mode) -> LanguagePref {
        match mode {
            Mode::Pinned(tag) => LanguagePref::Pinned(tag.clone()),
            Mode::Auto | Mode::Chunks(_) | Mode::Covered => LanguagePref::Auto,
        }
    }

    fn call(
        &self,
        clip: &EngineClip,
        language: &LanguagePref,
        timed: bool,
    ) -> Result<Call, EngineError> {
        call(
            self.engine,
            clip,
            language,
            self.threads,
            timed,
            self.cancel,
        )
    }

    /// The same call again pinned to a meeting language when it was heard outside them, as the
    /// transcribe stage does.
    fn settled(&self, clip: &EngineClip, first: Call) -> Result<Call, EngineError> {
        match settle_language(first.heard.as_ref(), self.meeting) {
            Settle::Keep => Ok(first),
            Settle::Redo(tag) => {
                let again = self.call(clip, &LanguagePref::Pinned(tag.clone()), true)?;
                Ok(Call {
                    heard: Some(tag),
                    busy: first.busy + again.busy,
                    ..again
                })
            }
        }
    }

    fn covered(&self, clip: &Clip) -> Result<Output, EngineError> {
        let takes = FsTakes::new(self.scratch.join("takes"));
        let id =
            RecordingId::parse(&format!("bench-{}", self.covered_runs.get())).map_err(other)?;
        self.covered_runs.set(self.covered_runs.get() + 1);
        takes.import(&id, &clip.path).map_err(other)?;
        let whole = takes.clip(&id, Channel::Mixed, None).map_err(other)?;
        let range = TimeRange::new(Ms(0), whole.duration).expect("0 is first");
        let speech = match &whole.activity {
            Activity::Unmeasured => vec![range],
            Activity::Speech(spans) => spans.clone(),
        };
        let first = self.call(&whole, &LanguagePref::Auto, true)?;
        let first = self.settled(&whole, first)?;
        let said: Vec<TimeRange> = first.segments.iter().map(|s| s.range).collect();
        let mut busy = first.busy;
        let mut calls = vec![(Ms(0), first)];
        let stretches = recover(&speech, &said, |stretch| {
            let piece = takes.slice(&whole, stretch).map_err(other)?;
            let again = self.call(&piece, &LanguagePref::Auto, true)?;
            let again = self.settled(&piece, again)?;
            busy += again.busy;
            let said = again
                .segments
                .iter()
                .map(|s| s.range.shifted(stretch.start()))
                .collect();
            calls.push((stretch.start(), again));
            Ok(said)
        })?;
        takes.delete(&id).map_err(other)?;
        Ok(Output {
            units: in_time_order(&calls),
            busy,
            stretches,
        })
    }

    /// Builds the recognizer outside the timed calls.
    fn warm_up(&self, mode: &Mode) -> Result<(), EngineError> {
        let path = self.scratch.join("warm-up.wav");
        std::fs::write(&path, Wav::encode(16_000, &[0; 16_000])).expect("scratch is writable");
        self.call(
            &wav_clip(&path, Ms(1000)),
            &Self::language(mode),
            *mode == Mode::Covered,
        )
        .map(|_| ())
    }

    fn clip(&self, mode: &Mode, clip: &Clip, wav: &Wav) -> Result<Output, EngineError> {
        let language = Self::language(mode);
        let whole = TimeRange::new(Ms(0), wav.ms(wav.samples.len())).expect("0 is first");
        let mut units = Vec::new();
        let mut busy = Duration::ZERO;
        match mode {
            Mode::Covered => return self.covered(clip),
            Mode::Auto | Mode::Pinned(_) => {
                let c = self.call(&wav_clip(&clip.path, whole.end()), &language, false)?;
                busy += c.busy;
                units.push(Unit {
                    span: whole,
                    text: c.text(),
                    heard: c.heard,
                });
            }
            Mode::Chunks(max) => {
                for (n, (a, b)) in chunk_cuts(&wav.samples, wav.rate, *max)
                    .into_iter()
                    .enumerate()
                {
                    let path = self.scratch.join(format!("chunk-{n}.wav"));
                    std::fs::write(&path, Wav::encode(wav.rate, &wav.samples[a..b]))
                        .expect("scratch is writable");
                    let span = TimeRange::new(wav.ms(a), wav.ms(b)).expect("cuts run forward");
                    let c = self.call(&wav_clip(&path, span.len()), &language, false)?;
                    busy += c.busy;
                    units.push(Unit {
                        span,
                        text: c.text(),
                        heard: c.heard,
                    });
                }
            }
        }
        Ok(Output {
            units,
            busy,
            stretches: 0,
        })
    }
}

struct Options {
    folders: Vec<PathBuf>,
    modes: Vec<Mode>,
    model: String,
    models_dir: PathBuf,
    threads: u8,
    meeting: Vec<LangTag>,
    dictionary: PathBuf,
    verbose: bool,
}

const USAGE: &str =
    "usage: bilingual-bench score <folders...> [--modes auto,tl,en,chunks,covered] \
[--model whisper-turbo] [--models DIR] [--threads 4] [--languages tl,en] [--dict FILE] [--verbose]
Each folder holds WAVs, refs.tsv, and optionally langs.tsv. --models defaults to \
$DORY_DATA_DIR/models/sherpa.";

fn parse_options(args: &[String]) -> Result<Options, String> {
    let mut folders = Vec::new();
    let mut modes = "auto,tl,en,chunks".to_owned();
    let mut model = "whisper-turbo".to_owned();
    let mut models_dir = None;
    let mut threads = 4u8;
    let mut languages = "tl,en".to_owned();
    let mut dictionary = PathBuf::from("/usr/share/dict/words");
    let mut verbose = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |flag: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match arg.as_str() {
            "--modes" => modes = value("--modes")?,
            "--model" => model = value("--model")?,
            "--models" => models_dir = Some(PathBuf::from(value("--models")?)),
            "--threads" => {
                threads = value("--threads")?
                    .parse()
                    .map_err(|_| "--threads wants a small number".to_owned())?;
            }
            "--languages" => languages = value("--languages")?,
            "--dict" => dictionary = PathBuf::from(value("--dict")?),
            "--verbose" => verbose = true,
            flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}")),
            folder => folders.push(PathBuf::from(folder)),
        }
    }
    if folders.is_empty() {
        return Err("score needs at least one clip folder".into());
    }
    let models_dir = models_dir
        .or_else(|| {
            std::env::var_os("DORY_DATA_DIR").map(|d| PathBuf::from(d).join("models/sherpa"))
        })
        .ok_or("pass --models DIR or set DORY_DATA_DIR")?;
    let modes = modes
        .split(',')
        .map(Mode::parse)
        .collect::<Result<Vec<_>, _>>()?;
    let meeting = languages
        .split(',')
        .map(|t| LangTag::parse(t).map_err(|_| format!("{t:?} is not a language tag")))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Options {
        folders,
        modes,
        model,
        models_dir,
        threads,
        meeting,
        dictionary,
        verbose,
    })
}

fn score(options: &Options) -> Result<(), String> {
    let lexicon = Lexicon::load(&options.dictionary)?;
    let mut clips = Vec::new();
    for folder in &options.folders {
        clips.extend(load_folder(folder)?);
    }
    let mut wavs = Vec::new();
    for clip in &clips {
        let bytes =
            std::fs::read(&clip.path).map_err(|e| format!("{}: {e}", clip.path.display()))?;
        wavs.push(Wav::parse(&bytes).map_err(|e| format!("{}: {e}", clip.path.display()))?);
    }
    let audio: f64 = wavs
        .iter()
        .map(|w| w.samples.len() as f64 / f64::from(w.rate))
        .sum();

    let sherpa = Sherpa::new(&options.models_dir, Sherpa::RELEASES);
    let entry = sherpa
        .catalog()
        .into_iter()
        .find(|e| e.id.name() == options.model)
        .ok_or_else(|| format!("{} is not in the sherpa catalog", options.model))?;
    if sherpa.availability(&entry.id) != Availability::Ready {
        return Err(format!(
            "{} is not installed under {}",
            entry.id,
            options.models_dir.display()
        ));
    }
    let cancel = CancelToken::new();
    let engine = sherpa.open(&entry.id, &cancel).map_err(|e| e.to_string())?;
    let scratch = tempfile::tempdir().map_err(|e| e.to_string())?;
    let run = Run {
        engine: engine.as_ref(),
        threads: options.threads,
        cancel: &cancel,
        scratch: scratch.path(),
        meeting: &options.meeting,
        covered_runs: std::cell::Cell::new(0),
    };

    println!("{} clips, {audio:.0} s of audio, {}", clips.len(), entry.id);
    println!("{HEADER}");
    println!("--- | --- | --- | --- | --- | --- | --- | --- | ---");
    let mut redone = Vec::new();
    for mode in &options.modes {
        match run.warm_up(mode) {
            Ok(()) => {}
            Err(EngineError::UnsupportedLanguage(tag)) => {
                println!("{} | unsupported: {} has no {tag}", mode.label(), entry.id);
                continue;
            }
            Err(other) => return Err(format!("{}: {other}", mode.label())),
        }
        let mut total = Score::default();
        for (clip, wav) in clips.iter().zip(&wavs) {
            let output = run
                .clip(mode, clip, wav)
                .map_err(|e| format!("{} on {}: {e}", mode.label(), clip.name))?;
            let audio_ms = wav.ms(wav.samples.len());
            let one = score_clip(clip, &output, &lexicon, &options.meeting, audio_ms);
            if options.verbose {
                eprintln!(
                    "{} {}\n  ref: {}\n  hyp: {}\n  heard: {:?}",
                    mode.label(),
                    clip.name,
                    clip.reference,
                    output
                        .units
                        .iter()
                        .map(|u| u.text.as_str())
                        .collect::<Vec<_>>()
                        .join(" | "),
                    output
                        .units
                        .iter()
                        .map(|u| u.heard.as_ref().map_or("-", LangTag::as_str))
                        .collect::<Vec<_>>()
                );
            }
            total.add(&one);
        }
        println!("{}", row(mode, &total));
        if *mode == Mode::Covered {
            redone.push(format!(
                "covered decoded {} stretches again, in {} of {} clips",
                total.stretches, total.clips_redone, total.clips
            ));
        }
    }
    for line in redone {
        println!("{line}");
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.split_first() {
        Some((command, rest)) if command == "score" => parse_options(rest).and_then(|o| score(&o)),
        _ => Err(USAGE.to_owned()),
    };
    if let Err(message) = result {
        eprintln!("{message}");
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(s: &str) -> LangTag {
        LangTag::parse(s).unwrap()
    }

    fn span(a: u64, b: u64) -> TimeRange {
        TimeRange::new(Ms(a), Ms(b)).unwrap()
    }

    fn lexicon() -> Lexicon {
        Lexicon::new(
            [
                "the", "beta", "build", "is", "ready", "ship", "friday", "plan", "we", "to", "it",
            ]
            .map(str::to_owned),
        )
    }

    fn w(text: &str) -> Vec<String> {
        words(text)
    }

    fn unit(a: u64, b: u64, text: &str, heard: Option<&str>) -> Unit {
        Unit {
            span: span(a, b),
            text: text.into(),
            heard: heard.map(tag),
        }
    }

    fn output(units: Vec<Unit>) -> Output {
        Output {
            units,
            busy: Duration::from_millis(500),
        }
    }

    #[test]
    fn parses_refs_and_langs() {
        let refs = "a.wav\tAng dokumento ay\nb.wav\tgood morning all\n\n";
        let langs = "a.wav\ttl:0-1500:3\nb.wav\ten:0-900:1 tl:1300-2000:2\n";
        let clips = parse_clips(Path::new("/x"), refs, Some(langs)).unwrap();
        assert_eq!(clips.len(), 2);
        assert_eq!(clips[0].path, Path::new("/x/a.wav"));
        assert_eq!(
            clips[0].parts,
            vec![Part {
                language: tag("tl"),
                range: span(0, 1500),
                words: 3
            }]
        );
        assert_eq!(clips[1].reference, "good morning all");
        assert_eq!(clips[1].part_texts(), vec!["good", "morning all"]);
        assert_eq!(clips[1].parts[1].range, span(1300, 2000));
    }

    #[test]
    fn rejects_bad_langs() {
        let refs = "a.wav\tone two three\n";
        let err = |langs: &str| parse_clips(Path::new("/x"), refs, Some(langs)).unwrap_err();
        assert!(err("a.wav\ttl:0-10:2").contains("counts 2 words"));
        assert!(err("a.wav\ttl:10-0:3").contains("not <tag>"));
        assert!(err("a.wav\ttl:0:3").contains("not <tag>"));
        assert!(err("c.wav\ttl:0-10:3").contains("c.wav"));
        assert!(parse_clips(Path::new("/x"), "a.wav no tab", None).is_err());
        let clips = parse_clips(Path::new("/x"), refs, None).unwrap();
        assert!(clips[0].parts.is_empty());
        assert_eq!(clips[0].part_texts(), vec!["one two three"]);
    }

    #[test]
    fn normalizes_case_punctuation_and_hyphens() {
        assert_eq!(
            w("Sa kalunos-lunos, na  \"Don't\" 5,000!"),
            ["sa", "kalunos", "lunos", "na", "don't", "5", "000"]
        );
        assert!(w("... !!").is_empty());
    }

    #[test]
    fn counts_word_edits_like_the_spike() {
        let r = w("nagganap ang pagbagsak sa mataas");
        assert_eq!(align(&r, &w("Nagganap ang pagbagsak, sa mataas!")).0, 0);
        assert_eq!(align(&r, &w("nagganap pagbagsak sa mataas na")).0, 2);
        assert_eq!(align(&r, &[]).0, 5);
        assert_eq!(align(&[], &w("a b")).0, 2);
    }

    #[test]
    fn a_stray_last_word_is_an_insertion_not_a_substitution() {
        let r = w("sertifikasyon");
        let (errors, aligned) = align(&r, &w("sertifikasyong you"));
        assert_eq!(errors, 2);
        assert_eq!(aligned, [true, false]);
        assert_eq!(align(&r, &w("sertifikasyon you")).1, [true, false]);
    }

    #[test]
    fn splits_english_from_tagalog() {
        let lex = lexicon();
        assert!(lex.is_english("ready"));
        assert!(lex.is_english("ships"));
        assert!(lex.is_english("building"));
        assert!(!lex.is_english("ang"));
        assert!(lex.is_tagalog("ang"));
        assert!(lex.is_tagalog("dokumento"));
        assert!(lex.is_tagalog("1667"));
    }

    #[test]
    fn recall_counts_words_as_bags() {
        let (hit, total) = recall(&w("a a b"), &w("a b b"), |_| true);
        assert_eq!((hit, total), (2, 3));
        assert_eq!(recall(&w("a b"), &w("c"), |x| x == "a"), (0, 1));
    }

    #[test]
    fn parses_modes() {
        assert_eq!(Mode::parse("auto").unwrap(), Mode::Auto);
        assert_eq!(Mode::parse("tl").unwrap(), Mode::Pinned(tag("tl")));
        assert_eq!(Mode::parse("chunks").unwrap(), Mode::Chunks(Ms(6000)));
        assert_eq!(Mode::parse("chunks=4.5").unwrap(), Mode::Chunks(Ms(4500)));
        assert!(Mode::parse("chunks=0").is_err());
        assert!(Mode::parse("loud!").is_err());
    }

    #[test]
    fn cuts_chunks_at_the_quietest_frame() {
        let rate = 16_000;
        let mut samples = vec![8000i16; 2 * rate as usize];
        let quiet = 12_000 + 12 * 320;
        samples[quiet..quiet + 320].fill(0);
        let cuts = chunk_cuts(&samples, rate, Ms(1500));
        assert_eq!(cuts, vec![(0, quiet), (quiet, samples.len())]);
        assert_eq!(
            chunk_cuts(&samples, rate, Ms(6000)),
            vec![(0, samples.len())]
        );
    }

    #[test]
    fn wav_round_trips_and_skips_extra_chunks() {
        let samples = [1i16, -2, 300, i16::MIN];
        let mut bytes = Wav::encode(16_000, &samples);
        let junk = *b"FLLR\x03\0\0\0abc\0";
        bytes.splice(36..36, junk);
        let wav = Wav::parse(&bytes).unwrap();
        assert_eq!(
            (wav.rate, wav.samples.as_slice()),
            (16_000, samples.as_slice())
        );
        assert_eq!(wav.ms(32_000), Ms(2000));
        let mut stereo = Wav::encode(16_000, &samples);
        stereo[22] = 2;
        assert!(Wav::parse(&stereo).unwrap_err().contains("mono"));
        assert!(Wav::parse(b"nope").is_err());
    }

    /// Three clips: a perfect Taglish one, a Taglish one the engine turned into English, and a
    /// turn-switch clip that lost its English turn.
    #[test]
    fn scores_a_three_clip_case() {
        let lex = lexicon();
        let meeting = [tag("tl"), tag("en")];

        let perfect = Clip {
            path: "p.wav".into(),
            name: "p.wav".into(),
            reference: "ang beta build ay ready".into(),
            parts: vec![],
        };
        let s1 = score_clip(
            &perfect,
            &output(vec![unit(0, 4000, "Ang beta build ay ready.", Some("tl"))]),
            &lex,
            &meeting,
            Ms(4000),
        );
        assert_eq!((s1.ref_words, s1.errors), (5, 0));
        assert_eq!(
            (s1.english_only, s1.dropped_turns, s1.wrong_language),
            (0, 0, 0)
        );
        assert_eq!(s1.heard_windows, 1);

        let flipped = Clip {
            path: "f.wav".into(),
            name: "f.wav".into(),
            reference: "ang mga tao ay ready".into(),
            parts: vec![],
        };
        let s2 = score_clip(
            &flipped,
            &output(vec![unit(0, 4000, "the beta build is ready", Some("zh"))]),
            &lex,
            &meeting,
            Ms(4000),
        );
        assert_eq!(s2.english_only, 1);
        assert_eq!(s2.errors, 4);
        assert_eq!((s2.heard_windows, s2.wrong_language), (1, 1));
        assert_eq!(s2.extra_words_on_silence, 0);

        let mixed = Clip {
            path: "m.wav".into(),
            name: "m.wav".into(),
            reference: "ang dokumento ay tumutukoy we plan to ship it friday".into(),
            parts: vec![
                Part {
                    language: tag("tl"),
                    range: span(0, 2000),
                    words: 4,
                },
                Part {
                    language: tag("en"),
                    range: span(2400, 6400),
                    words: 6,
                },
            ],
        };
        let s3 = score_clip(
            &mixed,
            &output(vec![unit(
                0,
                6400,
                "Ang dokumento ay tumutukoy",
                Some("tl"),
            )]),
            &lex,
            &meeting,
            Ms(6400),
        );
        assert_eq!((s3.mixed_parts, s3.dropped_turns), (2, 1));
        assert_eq!((s3.errors, s3.ref_words), (6, 10));
        assert_eq!((s3.heard_windows, s3.wrong_language), (1, 1));
        assert_eq!(s3.english_only, 0);

        let mut total = Score::default();
        for s in [&s1, &s2, &s3] {
            total.add(s);
        }
        assert_eq!(total.clips, 3);
        assert_eq!(total.ref_words, 20);
        assert_eq!(total.errors, 10);
        assert_eq!(pct(total.errors, total.ref_words), "50.0%");
        assert_eq!(total.audio_ms, 14_400);
        assert_eq!(
            row(&Mode::Auto, &total),
            "auto | 50.0% | 60% | 40% | 1/3 | 1/2 | 0 | 2/3 | 9.6"
        );
    }

    #[test]
    fn without_a_reported_language_the_column_is_a_dash() {
        let lex = lexicon();
        let clip = Clip {
            path: "a.wav".into(),
            name: "a.wav".into(),
            reference: "ang beta".into(),
            parts: vec![],
        };
        let s = score_clip(
            &clip,
            &output(vec![unit(0, 1000, "ang beta", None)]),
            &lex,
            &[tag("tl")],
            Ms(1000),
        );
        assert_eq!(s.heard_windows, 0);
        assert!(row(&Mode::Auto, &s).contains(" | - | "));
    }

    #[test]
    fn counts_the_words_a_chunk_adds_on_silence() {
        let lex = lexicon();
        let clip = Clip {
            path: "a.wav".into(),
            name: "a.wav".into(),
            reference: "ang beta build ay ready".into(),
            parts: vec![Part {
                language: tag("tl"),
                range: span(0, 8000),
                words: 5,
            }],
        };
        let s = score_clip(
            &clip,
            &output(vec![
                unit(0, 3000, "ang beta build", Some("tl")),
                unit(3000, 6000, "ay ready", Some("tl")),
                unit(6000, 8000, "you", Some("en")),
            ]),
            &lex,
            &[tag("tl"), tag("en")],
            Ms(8000),
        );
        assert_eq!(s.extra_words_on_silence, 1);
        assert_eq!(s.errors, 1);
        assert_eq!((s.heard_windows, s.wrong_language), (3, 1));
    }

    #[test]
    fn truth_is_the_part_holding_most_of_the_span() {
        let parts = [
            Part {
                language: tag("tl"),
                range: span(0, 5000),
                words: 1,
            },
            Part {
                language: tag("en"),
                range: span(5400, 9000),
                words: 1,
            },
        ];
        assert_eq!(truth_language(&parts, span(0, 6000)), Some(&tag("tl")));
        assert_eq!(truth_language(&parts, span(4000, 9000)), Some(&tag("en")));
        assert_eq!(truth_language(&parts, span(5000, 5400)), None);
    }
}
