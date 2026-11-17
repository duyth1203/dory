//! Where a search matches inside the open recording, and the step from one match to the next
//! (UX-32). Pure: [`matches`] lists every match in reading order from the same
//! `search::highlights` the sidebar marks with, and [`Matches`] holds the list with the current
//! one. The session owns a `Matches` while a search is active, and the views only draw it.
//!
//! Reading order is the title, then the transcript turns, then the summary. The summary is one
//! text, the one the Summary tab draws, so a match counted here is a mark there. A hit opens at
//! the first match of the turn `search::locate` picks (the one the sidebar snippet quotes), else
//! of the summary, else the title.

use std::ops::Range;

use dory_core::search::{self, Query};
use gpui_kit::{Bounds, Pixels, px};

use crate::a11y;
use crate::app::{DetailTab, Turn};

/// Where in the recording a match sits. Variants sort in reading order, then by index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Place {
    Title,
    /// An index into `Detail.turns`.
    Turn(usize),
    /// The summary as the Summary tab draws it. Spans are bytes of that text.
    Summary,
}

impl Place {
    /// The tab that shows this place, `None` for the title, which both tabs share.
    pub fn tab(self) -> Option<DetailTab> {
        match self {
            Place::Title => None,
            Place::Turn(_) => Some(DetailTab::Transcript),
            Place::Summary => Some(DetailTab::Summary),
        }
    }
}

/// One match: bytes of the text at `place`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchAt {
    pub place: Place,
    pub span: Range<usize>,
}

/// Every match in `items`, as (item, span) in text order. When no item holds a match of its own,
/// a phrase may still be split across two of them, and `search::locate` finds the words.
fn in_items(query: &Query, items: &[&str]) -> Vec<(usize, Range<usize>)> {
    let found: Vec<(usize, Range<usize>)> = items
        .iter()
        .enumerate()
        .flat_map(|(ix, item)| {
            search::highlights(query, item)
                .into_iter()
                .map(move |span| (ix, span))
        })
        .collect();
    if !found.is_empty() {
        return found;
    }
    search::locate(query, items)
        .map(|located| {
            let item = located.item;
            located.spans.into_iter().map(|span| (item, span)).collect()
        })
        .unwrap_or_default()
}

/// Every match of `query` in the title, the `turns`' text, and the `summary` as drawn, in reading
/// order.
pub fn matches(query: &Query, title: &str, turns: &[&str], summary: &str) -> Vec<MatchAt> {
    let title = in_items(query, &[title])
        .into_iter()
        .map(|(_, span)| MatchAt {
            place: Place::Title,
            span,
        });
    let turns = in_items(query, turns)
        .into_iter()
        .map(|(ix, span)| MatchAt {
            place: Place::Turn(ix),
            span,
        });
    let summary = search::highlights(query, summary)
        .into_iter()
        .map(|span| MatchAt {
            place: Place::Summary,
            span,
        });
    title.chain(turns).chain(summary).collect()
}

/// The matches of one search in one recording, and the one the user is on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Matches {
    query: Query,
    at: Vec<MatchAt>,
    current: usize,
}

impl Matches {
    /// The matches of `query`, positioned where a hit opens. `None` when nothing matches, as for
    /// a hit found only through a speaker's name.
    pub fn open(query: &Query, title: &str, turns: &[&str], summary: &str) -> Option<Matches> {
        let at = matches(query, title, turns, summary);
        if at.is_empty() {
            return None;
        }
        let current = opening(query, &at, turns, summary);
        Some(Matches {
            query: query.clone(),
            at,
            current,
        })
    }

    /// This list on match `current`, kept inside the list.
    pub fn on(mut self, current: usize) -> Matches {
        self.current = current.min(self.at.len() - 1);
        self
    }

    pub fn query(&self) -> &Query {
        &self.query
    }

    pub fn len(&self) -> usize {
        self.at.len()
    }

    pub fn is_empty(&self) -> bool {
        self.at.is_empty()
    }

    pub fn all(&self) -> &[MatchAt] {
        &self.at
    }

    /// The current match's place in the list, from 0.
    pub fn current(&self) -> usize {
        self.current
    }

    pub fn current_at(&self) -> &MatchAt {
        &self.at[self.current]
    }

    /// Moves `delta` matches, wrapping at either end.
    pub fn step(&mut self, delta: isize) -> &MatchAt {
        let len = self.at.len() as isize;
        self.current = (self.current as isize + delta).rem_euclid(len) as usize;
        self.current_at()
    }

    /// "2 of 9 matches", "1 of 1 match".
    pub fn readout(&self) -> String {
        let noun = if self.at.len() == 1 {
            "match"
        } else {
            "matches"
        };
        format!("{} of {} {noun}", self.current + 1, self.at.len())
    }

    /// What VoiceOver says when the current match moves: "Match 2 of 9, Sam, 12 minutes 30
    /// seconds".
    pub fn spoken(&self, turns: &[Turn]) -> String {
        let lead = format!("Match {} of {}", self.current + 1, self.at.len());
        match self.current_at().place {
            Place::Title => format!("{lead}, in the title"),
            Place::Summary => format!("{lead}, in the summary"),
            Place::Turn(ix) => match turns.get(ix) {
                Some(turn) => match turn.range {
                    Some(range) => format!(
                        "{lead}, {}, {}",
                        turn.label,
                        a11y::spoken_duration(range.start())
                    ),
                    None => format!("{lead}, {}", turn.label),
                },
                None => lead,
            },
        }
    }

    /// The matches in `place`'s one title, turn, or the summary, in text order.
    fn within(&self, place: Place) -> &[MatchAt] {
        let start = self.at.partition_point(|m| m.place < place);
        let end = self.at.partition_point(|m| m.place <= place);
        &self.at[start..end]
    }

    pub fn in_title(&self) -> &[MatchAt] {
        self.within(Place::Title)
    }

    pub fn in_turn(&self, ix: usize) -> &[MatchAt] {
        self.within(Place::Turn(ix))
    }

    /// The matches in the summary, in order, so the view can pair each with the same word in the
    /// rendered Markdown. The second value is the current match's place among them.
    pub fn in_summary(&self) -> (&[MatchAt], Option<usize>) {
        let start = self.at.partition_point(|m| m.place != Place::Summary);
        let current = self.current.checked_sub(start);
        (&self.at[start..], current)
    }
}

/// How far to scroll the page, positive moving the content down, so that `item` sits inside
/// `visible` clear of `top` pixels (the sticky tab row) and `bottom` pixels (the docked player).
/// `None` when it already does. Otherwise the item lands a little below the top edge, so the
/// turns after it stay in view.
pub fn shift_into_view(
    item: Bounds<Pixels>,
    visible: Bounds<Pixels>,
    top: Pixels,
    bottom: Pixels,
) -> Option<Pixels> {
    const LEAD: Pixels = px(24.);
    let (low, high) = (visible.top() + top, visible.bottom() - bottom);
    if item.top() >= low && item.bottom() <= high {
        return None;
    }
    let dy = low + LEAD - item.top();
    (dy.abs() > px(0.5)).then_some(dy)
}

/// Which match a hit opens at: the first in the turn `locate` picks, else the first in the
/// summary line it picks, else the first of all. With one clause the first match is that turn's.
fn opening(query: &Query, at: &[MatchAt], turns: &[&str], summary: &str) -> usize {
    let first_where = |is: &dyn Fn(&MatchAt) -> bool| at.iter().position(is);
    let located = |items: &[&str]| {
        if query.clauses().len() > 1 {
            search::locate(query, items).map(|l| l.item)
        } else {
            None
        }
    };
    if let Some(ix) = first_where(&|m| matches!(m.place, Place::Turn(_))) {
        return located(turns)
            .and_then(|turn| first_where(&|m| m.place == Place::Turn(turn)))
            .unwrap_or(ix);
    }
    if let Some(ix) = first_where(&|m| m.place == Place::Summary) {
        let lines: Vec<&str> = summary.split('\n').collect();
        return located(&lines)
            .and_then(|line| {
                let start: usize = lines[..line].iter().map(|l| l.len() + 1).sum();
                let end = start + lines[line].len();
                first_where(&|m| m.place == Place::Summary && (start..=end).contains(&m.span.start))
            })
            .unwrap_or(ix);
    }
    0
}

#[cfg(test)]
mod tests {
    use dory_core::domain::{Ms, SpeakerId, TimeRange};
    use gpui_kit::{point, size};

    use super::*;

    fn rect(top: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(0.), px(top)), size(px(600.), px(height)))
    }

    #[test]
    fn the_page_scrolls_only_when_the_turn_is_hidden_and_puts_it_near_the_top() {
        let visible = rect(52., 800.);
        let shift = |item| shift_into_view(item, visible, px(42.), px(80.));
        assert_eq!(shift(rect(300., 60.)), None, "in view already");
        assert_eq!(shift(rect(94., 60.)), None, "just under the tab row");
        assert_eq!(
            shift(rect(900., 60.)),
            Some(px(94. + 24. - 900.)),
            "below the fold moves up to near the top"
        );
        assert_eq!(
            shift(rect(60., 60.)),
            Some(px(94. + 24. - 60.)),
            "under the sticky tab row moves down"
        );
        assert_eq!(
            shift(rect(780., 60.)),
            Some(px(94. + 24. - 780.)),
            "under the docked player moves up"
        );
        assert!(
            shift(rect(118., 900.)).is_none(),
            "a turn taller than the view that already sits at the top stays"
        );
    }

    fn q(text: &str) -> Query {
        Query::parse(text).unwrap()
    }

    fn places(found: &[MatchAt]) -> Vec<Place> {
        found.iter().map(|m| m.place).collect()
    }

    fn turn(label: &str, start: Option<u64>) -> Turn {
        Turn {
            speaker: SpeakerId(1),
            label: label.to_owned().into(),
            range: start.and_then(|s| TimeRange::new(Ms(s), Ms(s + 5_000))),
            text: String::new(),
        }
    }

    const TURNS: [&str; 4] = [
        "Hello everyone.",
        "The roadmap slips to Q3.",
        "Fine by me.",
        "Roadmap items and the roadmap review.",
    ];

    fn roadmap() -> Matches {
        Matches::open(
            &q("roadmap"),
            "Roadmap review",
            &TURNS,
            "Key points\nRoadmap moves to Q3",
        )
        .unwrap()
    }

    #[test]
    fn matches_come_in_reading_order_the_title_then_the_turns_then_the_summary() {
        let found = roadmap();
        assert_eq!(
            places(found.all()),
            [
                Place::Title,
                Place::Turn(1),
                Place::Turn(3),
                Place::Turn(3),
                Place::Summary,
            ]
        );
        let spans: Vec<&str> = found
            .all()
            .iter()
            .filter_map(|m| match m.place {
                Place::Turn(ix) => Some(&TURNS[ix][m.span.clone()]),
                _ => None,
            })
            .collect();
        assert_eq!(spans, ["roadmap", "Roadmap", "roadmap"]);
    }

    #[test]
    fn a_title_match_a_turn_match_and_a_summary_block_each_find_their_place() {
        let found = matches(
            &q("budget"),
            "Budget sync",
            &["no", "the budget"],
            "Budget notes",
        );
        assert_eq!(
            found,
            [
                MatchAt {
                    place: Place::Title,
                    span: 0..6
                },
                MatchAt {
                    place: Place::Turn(1),
                    span: 4..10
                },
                MatchAt {
                    place: Place::Summary,
                    span: 0..6
                },
            ]
        );
    }

    #[test]
    fn a_hit_opens_at_the_first_transcript_match_not_the_title() {
        let found = roadmap();
        assert_eq!(found.current(), 1);
        assert_eq!(found.current_at().place, Place::Turn(1));
        let summary_only = Matches::open(&q("budget"), "Sync", &["hello"], "Budget").unwrap();
        assert_eq!(summary_only.current_at().place, Place::Summary);
        let title_only = Matches::open(&q("sync"), "Sync", &["hello"], "Notes").unwrap();
        assert_eq!(title_only.current_at().place, Place::Title);
    }

    #[test]
    fn with_several_words_a_hit_opens_at_the_turn_that_holds_them_all() {
        let turns = ["we need a budget", "the budget roadmap is late", "roadmap"];
        let found = Matches::open(&q("budget roadmap"), "Sync", &turns, "").unwrap();
        assert_eq!(found.current_at().place, Place::Turn(1));
        assert_eq!(
            found.current(),
            1,
            "after the first turn's one match, at the second turn's first"
        );
    }

    #[test]
    fn with_several_words_a_summary_hit_opens_at_the_line_that_holds_them_all() {
        let summary = "Budget only\nThe budget roadmap plan\nRoadmap";
        let found = Matches::open(&q("budget roadmap"), "Sync", &[], summary).unwrap();
        assert_eq!(found.current(), 1, "after the first line's one match");
        assert_eq!(&summary[found.current_at().span.clone()], "budget");
    }

    #[test]
    fn a_phrase_split_between_two_speakers_still_matches_through_its_words() {
        let turns = ["we ship the road", "map is next"];
        let found = Matches::open(&q("\"road map\""), "Sync", &turns, "").unwrap();
        assert_eq!(places(found.all()), [Place::Turn(0)]);
    }

    #[test]
    fn nothing_matching_is_no_list() {
        assert!(Matches::open(&q("zebra"), "Sync", &["hello"], "Notes").is_none());
    }

    #[test]
    fn next_and_previous_wrap_at_the_ends() {
        let mut found = roadmap();
        let readouts = |found: &mut Matches, delta: isize, times: usize| -> Vec<String> {
            (0..times)
                .map(|_| {
                    found.step(delta);
                    found.readout()
                })
                .collect()
        };
        assert_eq!(found.readout(), "2 of 5 matches");
        assert_eq!(
            readouts(&mut found, 1, 4),
            [
                "3 of 5 matches",
                "4 of 5 matches",
                "5 of 5 matches",
                "1 of 5 matches"
            ],
            "next wraps from the last match to the first"
        );
        assert_eq!(
            readouts(&mut found, -1, 2),
            ["5 of 5 matches", "4 of 5 matches"],
            "previous wraps from the first to the last"
        );
    }

    #[test]
    fn one_match_reads_in_the_singular_and_steps_onto_itself() {
        let mut one = Matches::open(&q("zeta"), "Sync", &["zeta"], "").unwrap();
        assert_eq!(one.readout(), "1 of 1 match");
        one.step(1);
        assert_eq!(one.readout(), "1 of 1 match");
        one.step(-1);
        assert_eq!(one.current(), 0);
    }

    #[test]
    fn voiceover_hears_the_match_number_the_speaker_and_the_time() {
        let turns = [
            turn("Me", Some(0)),
            turn("Sam", Some(750_000)),
            turn("Rita", None),
        ];
        let mut found = Matches::open(
            &q("roadmap"),
            "Roadmap review",
            &["x", "roadmap", "roadmap"],
            "Roadmap",
        )
        .unwrap();
        assert_eq!(
            found.spoken(&turns),
            "Match 2 of 4, Sam, 12 minutes 30 seconds"
        );
        found.step(1);
        assert_eq!(found.spoken(&turns), "Match 3 of 4, Rita");
        found.step(1);
        assert_eq!(found.spoken(&turns), "Match 4 of 4, in the summary");
        found.step(1);
        assert_eq!(found.spoken(&turns), "Match 1 of 4, in the title");
    }

    #[test]
    fn a_view_reads_the_matches_of_one_turn_the_title_and_the_summary() {
        let found = roadmap();
        assert_eq!(found.in_title().len(), 1);
        assert_eq!(found.in_turn(0).len(), 0);
        assert_eq!(found.in_turn(3).len(), 2);
        let (summary, current) = found.in_summary();
        assert_eq!(places(summary), [Place::Summary]);
        assert_eq!(current, None, "the current match is in the transcript");
        let mut found = found;
        found.step(-2);
        assert_eq!(
            found.in_summary().1,
            Some(0),
            "previous from the second match wraps to the summary"
        );
    }

    #[test]
    fn keeping_the_current_match_clamps_it_to_a_shorter_list() {
        assert_eq!(roadmap().on(99).current(), 4);
        assert_eq!(roadmap().on(3).current(), 3);
    }

    #[test]
    fn legacy_turns_with_no_times_still_match() {
        let found =
            Matches::open(&q("mentoring"), "1:1", &["Sam wants a mentoring plan."], "").unwrap();
        assert_eq!(found.current_at().place, Place::Turn(0));
        assert_eq!(found.current_at().span, 12..21);
    }
}
