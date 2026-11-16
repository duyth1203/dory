use chrono::NaiveDate;
use dory_core::app::RecorderStatus;
use dory_core::domain::{Fraction, JobState, Ms, RecordingId, Stage};
use dory_gpui::fixtures::{self, SAMPLES};
use dory_gpui::library::{
    DateGroup, Progress, RowStatus, SearchQuery, Selection, StatusFilter, after_removal,
    group_by_date, row_meta,
};

fn titles(rows: &[&dory_core::domain::RecordingRow]) -> Vec<String> {
    rows.iter()
        .map(|r| r.recording.title.clone().unwrap_or_default())
        .collect()
}

#[test]
fn the_samples_group_by_today_yesterday_previous_week_and_month() {
    let rows = fixtures::rows();
    let groups = group_by_date(&rows, &fixtures::now());
    let today = fixtures::today();
    let shown: Vec<(String, Vec<String>)> = groups
        .iter()
        .map(|(group, members)| (group.label(today), titles(members)))
        .collect();
    let want = |label: &str, titles: &[&str]| {
        (
            label.to_owned(),
            titles.iter().map(|t| (*t).to_owned()).collect(),
        )
    };
    assert_eq!(
        shown,
        [
            want("Today", &["Weekly sync", "Design review", "1:1 with Sam"]),
            want(
                "Yesterday",
                &[
                    "Kundengespräch Müller GmbH",
                    "Partner call, Lyon office",
                    "Hiring debrief"
                ]
            ),
            want("Previous 7 days", &["Customer interview: Ortega"]),
            want("October", &["Roadmap review"]),
        ]
    );
}

#[test]
fn date_group_boundaries() {
    let today = NaiveDate::from_ymd_opt(2026, 11, 12).unwrap();
    let day = |m, d| NaiveDate::from_ymd_opt(2026, m, d).unwrap();
    let cases = [
        (day(11, 13), DateGroup::Today),
        (day(11, 12), DateGroup::Today),
        (day(11, 11), DateGroup::Yesterday),
        (day(11, 10), DateGroup::Previous7Days),
        (day(11, 5), DateGroup::Previous7Days),
        (
            day(11, 4),
            DateGroup::Month {
                year: 2026,
                month: 11,
            },
        ),
    ];
    for (date, want) in cases {
        assert_eq!(DateGroup::of(date, today), want, "{date}");
    }
    let last_year = DateGroup::of(NaiveDate::from_ymd_opt(2025, 10, 28).unwrap(), today);
    assert_eq!(last_year.label(today), "October 2025");
    assert_eq!(
        DateGroup::Month {
            year: 2026,
            month: 11
        }
        .label(today),
        "November"
    );
}

#[test]
fn row_meta_follows_the_canvas_per_group() {
    let now = fixtures::now();
    let today = fixtures::today();
    let meta = |title: &str| {
        let row = SAMPLES.iter().find(|s| s.title == title).unwrap().row();
        let day = dory_gpui::library::local(row.recording.created_at, &now.timezone());
        row_meta(&row, DateGroup::of(day.date_naive(), today), &now)
    };
    assert_eq!(meta("Weekly sync"), "14:00 · 62 min");
    assert_eq!(meta("Kundengespräch Müller GmbH"), "16:15 · 35 min");
    assert_eq!(meta("Customer interview: Ortega"), "Mon 9 Nov · 94 min");
    assert_eq!(meta("Roadmap review"), "28 Oct · 71 min");
}

#[test]
fn row_status_reads_the_job_the_recorder_and_progress() {
    let rows = fixtures::rows();
    let row = |title: &str| {
        rows.iter()
            .find(|r| r.recording.title.as_deref() == Some(title))
            .unwrap()
    };
    let idle = RecorderStatus::Idle;
    let forty = Progress {
        stage: Stage::Transcribe,
        fraction: Fraction::new(0.4),
    };
    let weekly = row("Weekly sync");
    assert_eq!(
        RowStatus::derive(weekly, &idle, Some(forty)).label(),
        "Transcribing, 40%"
    );
    assert_eq!(
        RowStatus::derive(weekly, &idle, None).label(),
        "Transcribing"
    );
    assert_eq!(
        RowStatus::derive(row("Design review"), &idle, None),
        RowStatus::Done
    );
    assert_eq!(
        RowStatus::derive(row("Hiring debrief"), &idle, None).label(),
        "Needs attention"
    );

    let recording = RecorderStatus::Recording {
        id: weekly.recording.id.clone(),
        recorded: Ms(1_000),
    };
    assert_eq!(
        RowStatus::derive(weekly, &recording, Some(forty)),
        RowStatus::Recording
    );

    let mut fresh = row("Design review").clone();
    fresh.job = None;
    fresh.has_transcript = false;
    assert_eq!(
        RowStatus::derive(&fresh, &idle, None),
        RowStatus::ReadyToTranscribe
    );
    fresh.job = Some(JobState::Queued);
    assert_eq!(RowStatus::derive(&fresh, &idle, None), RowStatus::Queued);
    fresh.job = Some(JobState::Running {
        stage: Stage::Summarize,
    });
    assert_eq!(
        RowStatus::derive(&fresh, &idle, Some(forty)),
        RowStatus::Summarizing
    );
}

#[test]
fn the_status_filter_keeps_only_matching_samples() {
    let rows = fixtures::rows();
    let kept = |filter: StatusFilter| -> Vec<String> {
        rows.iter()
            .filter(|r| filter.admits(RowStatus::derive(r, &RecorderStatus::Idle, None)))
            .map(|r| r.recording.title.clone().unwrap())
            .collect()
    };
    assert_eq!(kept(StatusFilter::All).len(), SAMPLES.len());
    assert_eq!(kept(StatusFilter::Processing), ["Weekly sync"]);
    assert_eq!(kept(StatusFilter::NeedsAttention), ["Hiring debrief"]);
    assert_eq!(kept(StatusFilter::Done).len(), 6);
    assert!(StatusFilter::Processing.admits(RowStatus::ReadyToTranscribe));
    assert!(StatusFilter::Processing.admits(RowStatus::Recording));
}

#[test]
fn selection_after_delete_moves_to_the_next_then_the_previous_row() {
    let ids: Vec<RecordingId> = fixtures::rows()
        .into_iter()
        .map(|r| r.recording.id)
        .collect();
    let one = |ix: usize| Selection::One(ids[ix].clone());
    assert_eq!(after_removal(&ids, &ids[0]), one(1));
    assert_eq!(after_removal(&ids, &ids[3]), one(4));
    assert_eq!(after_removal(&ids, &ids[ids.len() - 1]), one(ids.len() - 2));
    assert_eq!(after_removal(&ids[..1], &ids[0]), Selection::None);
    assert_eq!(after_removal(&ids[1..], &ids[0]), Selection::None);
}

#[test]
fn search_trims_and_an_empty_search_lists_everything() {
    assert_eq!(SearchQuery::new("   ").to_query().text, None);
    assert_eq!(
        SearchQuery::new("  Lieferung ").to_query().text.as_deref(),
        Some("Lieferung")
    );
}
