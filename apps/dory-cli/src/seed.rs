//! `seed`: writes deterministic synthetic recordings through `Store::apply`, for the search lanes
//! and the perf runs in `docs/plans/search.md`. Words come from `/usr/share/dict/words` with Zipf
//! frequencies, and the words that plan's Appendix A names are planted at known places and kept
//! out of the random words, so a lane knows exactly which recordings hold them.

use std::collections::BTreeMap;
use std::time::Instant;

use dory::Config;
use dory_core::domain::{
    Channel, ChannelLayout, LangTag, LanguagePref, Ms, Recording, RecordingId, Revision, Segment,
    SpeakerId, SpeakerNames, Summary, TimeRange, Timestamp, Transcript,
};
use dory_core::models::ModelRef;
use dory_core::ports::Change;

use crate::args::SeedArgs;

const DICTIONARY: &str = "/usr/share/dict/words";
const VOCABULARY: usize = 40_000;
const WORDS_PER_TURN: usize = 15;
const TURN_MS: u64 = 6_000;
const SUMMARY_WORDS: usize = 300;
/// 2026-01-05 09:00 UTC. Each recording is six hours after the one before it.
const FIRST_AT: i64 = 1_767_603_600_000;
const SPACING_MS: i64 = 6 * 3_600_000;

/// Words a lane looks for. None of them, nor a word starting with one, is drawn at random.
const RESERVED: &[&str] = &[
    "kilimanjaro",
    "muller",
    "review",
    "roadmap",
    "atlas",
    "antinational",
    "okavango",
    "priyanka",
    "zanzibar",
];

/// Where Appendix A's words go, by recording index.
const KILIMANJARO: usize = 1;
const MUELLER: usize = 2;
const MULLER: usize = 3;
const BUDGET_REVIEW: usize = 4;
const REVIEW_OF_THE_BUDGET: usize = 5;
const SUMMARY_ONLY: usize = 6;
const ROADMAP_TITLE: usize = 8;
const STRASSE: usize = 12;
const ORSTED: usize = 13;

pub fn run(config: &Config, args: &SeedArgs) -> Result<(), String> {
    crate::allow_fakes("seed")?;
    let dictionary = std::fs::read_to_string(DICTIONARY)
        .map_err(|e| format!("cannot read {DICTIONARY}: {e}"))?;
    let corpus = Corpus::new(&dictionary, args.seed);
    let started = Instant::now();
    let written = dory::with_store(config, |store| {
        for index in 0..args.recordings as usize {
            store.apply(corpus.recording(index, args.words as usize))?;
        }
        Ok::<_, dory_core::ports::StoreError>(())
    })
    .map_err(crate::chain)?;
    written.map_err(crate::chain)?;
    println!(
        "seeded {} recordings of {} words in {:.1} s",
        args.recordings,
        args.words,
        started.elapsed().as_secs_f64()
    );
    for (word, where_) in plants(args.recordings as usize) {
        println!("planted {word:?} in {where_}");
    }
    Ok(())
}

fn id(index: usize) -> RecordingId {
    RecordingId::generate(created_at(index), index as u16)
}

fn created_at(index: usize) -> Timestamp {
    Timestamp(FIRST_AT + index as i64 * SPACING_MS)
}

fn has_roadmap(index: usize) -> bool {
    index.is_multiple_of(3)
}

fn has_atlas_title(index: usize) -> bool {
    index % 25 == 9
}

fn has_budget_title(index: usize) -> bool {
    index % 20 == 14 && !has_atlas_title(index)
}

fn has_antinational(index: usize) -> bool {
    index % 40 == 11
}

/// What a lane needs to know: each planted word and the recordings that hold it.
fn plants(n: usize) -> Vec<(&'static str, String)> {
    let ids = |indexes: Vec<usize>| {
        indexes
            .into_iter()
            .map(|i| id(i).to_string())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let all = |pred: fn(usize) -> bool| (0..n).filter(|&i| pred(i)).collect::<Vec<_>>();
    vec![
        (
            "kilimanjaro",
            format!("the transcript of {}", ids(vec![KILIMANJARO])),
        ),
        (
            "Müller",
            format!("the transcript of {}", ids(vec![MUELLER])),
        ),
        ("Muller", format!("the transcript of {}", ids(vec![MULLER]))),
        (
            "budget review",
            format!("the transcript of {}", ids(vec![BUDGET_REVIEW])),
        ),
        (
            "review of the budget",
            format!("the transcript of {}", ids(vec![REVIEW_OF_THE_BUDGET])),
        ),
        (
            "okavango",
            format!("the summary of {} only", ids(vec![SUMMARY_ONLY])),
        ),
        (
            "Straße",
            format!("the transcript of {}", ids(vec![STRASSE])),
        ),
        ("Ørsted", format!("the transcript of {}", ids(vec![ORSTED]))),
        (
            "roadmap",
            format!(
                "the title of {} and {} transcripts, every third from the first",
                ids(vec![ROADMAP_TITLE]),
                all(has_roadmap).len()
            ),
        ),
        (
            "atlas",
            format!("the titles of {}", ids(all(has_atlas_title))),
        ),
        (
            "Budget",
            format!("the titles of {}", ids(all(has_budget_title))),
        ),
        (
            "antinational",
            format!("the transcripts of {}", ids(all(has_antinational))),
        ),
    ]
}

/// xorshift64, so a seed gives the same library on every machine with the same dictionary.
struct Rng(u64);

impl Rng {
    /// Splitmix64 of the seed and the stream, so neighbouring streams start far apart.
    fn new(seed: u64, stream: u64) -> Self {
        let mut z = seed
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
            .wrapping_add(stream)
            .wrapping_add(0x9e37_79b9_7f4a_7c15);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        Self((z ^ (z >> 31)).max(1))
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

struct Corpus<'a> {
    seed: u64,
    words: Vec<&'a str>,
    /// Running sums of 1/rank, for Zipf draws.
    cumulative: Vec<f64>,
}

impl<'a> Corpus<'a> {
    fn new(dictionary: &'a str, seed: u64) -> Self {
        let mut rng = Rng::new(seed, u64::MAX);
        let mut words: Vec<&str> = dictionary
            .lines()
            .filter(|w| (2..=12).contains(&w.len()))
            .filter(|w| {
                let lower = w.to_lowercase();
                !RESERVED.iter().any(|r| lower.starts_with(r))
            })
            .collect();
        for i in (1..words.len()).rev() {
            words.swap(i, rng.below(i + 1));
        }
        words.truncate(VOCABULARY);
        // A common word, so "budget" is a search with many hits.
        if let Some(slot) = words.get_mut(200) {
            *slot = "budget";
        }
        let cumulative = (1..=words.len())
            .scan(0.0, |sum, rank| {
                *sum += 1.0 / rank as f64;
                Some(*sum)
            })
            .collect();
        Self {
            seed,
            words,
            cumulative,
        }
    }

    fn word(&self, rng: &mut Rng) -> &'a str {
        let total = self.cumulative.last().copied().unwrap_or(1.0);
        let x = rng.unit() * total;
        let at = self.cumulative.partition_point(|&c| c < x);
        self.words[at.min(self.words.len() - 1)]
    }

    fn recording(&self, index: usize, words: usize) -> Vec<Change> {
        let mut rng = Rng::new(self.seed, index as u64);
        let id = id(index);
        let title = self.title(index, &mut rng);
        let mut text: Vec<&str> = (0..words).map(|_| self.word(&mut rng)).collect();
        let plants = transcript_plants(index);
        let region = words / plants.len().max(1);
        for (n, plant) in plants.iter().enumerate() {
            let at = n * region + rng.below(region - plant.len());
            text.splice(at..at + plant.len(), plant.iter().copied());
        }
        let segments = text
            .chunks(WORDS_PER_TURN)
            .enumerate()
            .map(|(turn, chunk)| {
                let speaker = (turn % 3) as u16;
                let start = turn as u64 * TURN_MS;
                Segment {
                    range: Some(TimeRange::new(Ms(start), Ms(start + TURN_MS - 200)).unwrap()),
                    speaker: SpeakerId(speaker),
                    channel: if speaker == 0 {
                        Channel::Mic
                    } else {
                        Channel::System
                    },
                    text: sentence(chunk),
                    language: None,
                }
            })
            .collect();
        let english = LangTag::parse("en").unwrap();
        let transcript = Transcript {
            revision: Revision(1),
            segments,
            detected_languages: vec![english.clone()],
            produced_by: ModelRef::parse("sherpa/seed").unwrap(),
        };
        let summary = Summary {
            markdown: self.summary(index, &mut rng),
            language: Some(english),
            produced_by: ModelRef::parse("ollama/seed").unwrap(),
            transcript_revision: Revision(1),
        };
        let mut changes = vec![
            Change::RecordingCreated(Recording {
                id: id.clone(),
                title: Some(title),
                created_at: created_at(index),
                language: LanguagePref::Auto,
                layout: ChannelLayout::MicSystem,
                take: None,
            }),
            Change::TranscriptReplaced {
                id: id.clone(),
                transcript,
            },
            Change::SummaryReplaced {
                id: id.clone(),
                summary,
            },
        ];
        if index.is_multiple_of(2) {
            changes.push(Change::SpeakerNamesReplaced {
                id,
                names: SpeakerNames {
                    for_revision: Revision(1),
                    names: BTreeMap::from([
                        (SpeakerId(1), "Sam".to_owned()),
                        (SpeakerId(2), "Alex".to_owned()),
                    ]),
                },
            });
        }
        changes
    }

    fn title(&self, index: usize, rng: &mut Rng) -> String {
        if index == ROADMAP_TITLE {
            return "Roadmap planning".into();
        }
        let lead = if has_atlas_title(index) {
            "Atlas"
        } else if has_budget_title(index) {
            "Budget"
        } else {
            self.word(rng)
        };
        let mut title = format!("{lead} {} sync {index}", self.word(rng));
        if let Some(first) = title.get(..1) {
            title.replace_range(..1, &first.to_uppercase());
        }
        title
    }

    fn summary(&self, index: usize, rng: &mut Rng) -> String {
        let mut words: Vec<&str> = (0..SUMMARY_WORDS).map(|_| self.word(rng)).collect();
        if index == SUMMARY_ONLY {
            let at = rng.below(words.len());
            words[at] = "okavango";
        }
        let mut markdown = String::from("## Summary\n\n");
        for line in words.chunks(20) {
            markdown.push_str("- ");
            markdown.push_str(&sentence(line));
            markdown.push('\n');
        }
        markdown
    }
}

fn transcript_plants(index: usize) -> Vec<&'static [&'static str]> {
    let mut plants: Vec<&'static [&'static str]> = Vec::new();
    match index {
        KILIMANJARO => plants.push(&["kilimanjaro"]),
        MUELLER => plants.push(&["Müller"]),
        MULLER => plants.push(&["Muller"]),
        BUDGET_REVIEW => plants.push(&["the", "budget", "review"]),
        REVIEW_OF_THE_BUDGET => plants.push(&["the", "review", "of", "the", "budget"]),
        STRASSE => plants.push(&["Straße"]),
        ORSTED => plants.push(&["Ørsted"]),
        _ => {}
    }
    if has_roadmap(index) {
        plants.push(&["roadmap"]);
    }
    if has_antinational(index) {
        plants.push(&["antinational"]);
    }
    plants
}

/// Words as a sentence: the first capitalized, a period at the end.
fn sentence(words: &[&str]) -> String {
    let mut text = words.join(" ");
    if let Some(first) = text.chars().next() {
        let upper: String = first.to_uppercase().collect();
        text.replace_range(..first.len_utf8(), &upper);
    }
    text.push('.');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const DICTIONARY: &str =
        "alpha\nbeta\ngamma\ndelta\nreview\nreviewer\nAtlas\nmullerian\nzeta\n";

    fn transcript_text(changes: &[Change]) -> String {
        changes
            .iter()
            .find_map(|c| match c {
                Change::TranscriptReplaced { transcript, .. } => Some(
                    transcript
                        .segments
                        .iter()
                        .map(|s| s.text.as_str())
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn a_seed_gives_the_same_recordings_every_time() {
        let corpus = Corpus::new(DICTIONARY, 3);
        assert_eq!(corpus.recording(7, 60), corpus.recording(7, 60));
        assert_ne!(
            transcript_text(&corpus.recording(47, 60)),
            transcript_text(&corpus.recording(48, 60)),
            "neighbours differ"
        );
        assert_ne!(
            transcript_text(&corpus.recording(7, 60)),
            transcript_text(&Corpus::new(DICTIONARY, 4).recording(7, 60))
        );
    }

    #[test]
    fn reserved_words_appear_only_where_planted() {
        let corpus = Corpus::new(DICTIONARY, 1);
        assert!(
            corpus
                .words
                .iter()
                .all(|w| !["review", "reviewer", "Atlas", "mullerian"].contains(w)),
            "{:?}",
            corpus.words
        );
        let text = |i| transcript_text(&corpus.recording(i, 300)).to_lowercase();
        assert!(text(KILIMANJARO).contains("kilimanjaro"));
        assert!(text(BUDGET_REVIEW).contains("the budget review"));
        assert!(text(REVIEW_OF_THE_BUDGET).contains("the review of the budget"));
        assert!(text(MUELLER).contains("müller"));
        assert!(!text(MULLER).contains("müller"));
        assert!(!text(7).contains("review"));
        assert!(text(9).contains("roadmap"));
        assert!(!text(10).contains("roadmap"));
        assert!(!text(SUMMARY_ONLY).contains("okavango"));
    }

    #[test]
    fn every_turn_has_a_time_and_the_ids_sort_by_age() {
        let corpus = Corpus::new(DICTIONARY, 1);
        let changes = corpus.recording(0, 18_000);
        let Change::TranscriptReplaced { transcript, .. } = &changes[1] else {
            panic!("{changes:?}");
        };
        assert_eq!(transcript.segments.len(), 1_200);
        assert_eq!(
            transcript.segments.last().unwrap().range.unwrap().end(),
            Ms(7_200_000 - 200),
            "18,000 words is the 2-hour cap"
        );
        assert!(id(1) < id(2));
    }
}
