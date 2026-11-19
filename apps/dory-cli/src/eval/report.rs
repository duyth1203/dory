//! What `eval` prints and appends to the TSV. Pure, so the shapes are tested without a library.

use super::score::{SectionScore, SummaryScore, TermHits, WordErrors};

/// One scored case.
#[derive(Debug)]
pub struct Outcome {
    pub case: String,
    /// The model that wrote the transcript.
    pub stt: String,
    /// The model that wrote the summary, when there is one.
    pub llm: Option<String>,
    pub wer: WordErrors,
    pub terms: TermHits,
    /// Terms the reference never says, so they could not be scored.
    pub absent_terms: Vec<String>,
    /// `None` when the case has no `expected.txt` or the recording has no summary.
    pub summary: Option<SummaryScore>,
    /// Time in the transcribe stage. `None` when the run only summarized.
    pub stt_secs: Option<f64>,
    /// Time in the summarize stage. `None` when no summary was written this run.
    pub llm_secs: Option<f64>,
    pub audio_secs: f64,
}

pub const TSV_HEADER: &str = "run_at\tlabel\tcase\tstt\tllm\tref_words\twer\tsub\tdel\tins\tterms_expected\tterms_found\tactions_expected\tactions_found\tactions_extra\tdecisions_expected\tdecisions_found\tdecisions_extra\tstructured\tstt_secs\tllm_secs\taudio_secs";

fn percent(share: Option<f64>) -> String {
    share.map_or_else(|| "n/a".to_owned(), |s| format!("{:.1}%", s * 100.0))
}

fn found_of(score: &SectionScore) -> String {
    format!(
        "{}/{} (+{} extra)",
        score.found, score.expected, score.extra
    )
}

fn seconds(secs: Option<f64>) -> String {
    secs.map_or_else(|| "-".to_owned(), |s| format!("{s:.1} s"))
}

/// Seconds spent per second of audio. Under 1 is faster than real time.
fn real_time_factor(stt_secs: Option<f64>, audio_secs: f64) -> String {
    match stt_secs {
        Some(secs) if audio_secs > 0.0 => format!(" ({:.2}x real time)", secs / audio_secs),
        _ => String::new(),
    }
}

pub fn case_line(o: &Outcome) -> String {
    let summary = match &o.summary {
        Some(s) => format!(
            "actions {}  decisions {}  {}",
            found_of(&s.actions),
            found_of(&s.decisions),
            if s.structured {
                "structured"
            } else {
                "unstructured"
            }
        ),
        None => "summary not scored".to_owned(),
    };
    format!(
        "{}  WER {} (S {} D {} I {} of {})  terms {}/{} ({})  {summary}  stt {}{}  llm {}",
        o.case,
        percent(o.wer.rate()),
        o.wer.substitutions,
        o.wer.deletions,
        o.wer.insertions,
        o.wer.reference,
        o.terms.found,
        o.terms.expected,
        percent(o.terms.recall()),
        seconds(o.stt_secs),
        real_time_factor(o.stt_secs, o.audio_secs),
        seconds(o.llm_secs),
    )
}

/// The corpus line and the models used. Rates come from summed counts, so a long case weighs
/// more than a short one.
pub fn totals(outcomes: &[Outcome]) -> Vec<String> {
    let wer = outcomes
        .iter()
        .fold(WordErrors::default(), |a, o| a + o.wer);
    let terms = outcomes
        .iter()
        .fold(TermHits::default(), |a, o| a + o.terms);
    let scored: Vec<&SummaryScore> = outcomes.iter().filter_map(|o| o.summary.as_ref()).collect();
    let actions = scored
        .iter()
        .fold(SectionScore::default(), |a, s| a + s.actions);
    let decisions = scored
        .iter()
        .fold(SectionScore::default(), |a, s| a + s.decisions);
    let stt_secs: f64 = outcomes.iter().filter_map(|o| o.stt_secs).sum();
    let audio_secs: f64 = outcomes
        .iter()
        .filter(|o| o.stt_secs.is_some())
        .map(|o| o.audio_secs)
        .sum();
    let llm_secs: f64 = outcomes.iter().filter_map(|o| o.llm_secs).sum();
    let summary = if scored.is_empty() {
        "summary not scored".to_owned()
    } else {
        format!(
            "actions {}  decisions {}",
            found_of(&actions),
            found_of(&decisions)
        )
    };
    let models = |names: Vec<&str>| {
        let mut names = names;
        names.sort_unstable();
        names.dedup();
        if names.is_empty() {
            "-".to_owned()
        } else {
            names.join(", ")
        }
    };
    vec![
        format!(
            "total ({} cases)  WER {} (S {} D {} I {} of {})  terms {}/{} ({})  {summary}  stt {}{}  llm {}",
            outcomes.len(),
            percent(wer.rate()),
            wer.substitutions,
            wer.deletions,
            wer.insertions,
            wer.reference,
            terms.found,
            terms.expected,
            percent(terms.recall()),
            seconds((stt_secs > 0.0).then_some(stt_secs)),
            real_time_factor((stt_secs > 0.0).then_some(stt_secs), audio_secs),
            seconds((llm_secs > 0.0).then_some(llm_secs)),
        ),
        format!(
            "stt {}  llm {}",
            models(outcomes.iter().map(|o| o.stt.as_str()).collect()),
            models(outcomes.iter().filter_map(|o| o.llm.as_deref()).collect())
        ),
    ]
}

/// A TSV cell: a tab or a line break would split the row.
fn cell(text: &str) -> String {
    text.replace(['\t', '\n', '\r'], " ")
}

fn number<T: ToString>(value: Option<T>) -> String {
    value.map(|v| v.to_string()).unwrap_or_default()
}

pub fn tsv_row(run_at: &str, label: &str, o: &Outcome) -> String {
    let section = |pick: fn(&SummaryScore) -> SectionScore| {
        let score = o.summary.as_ref().map(pick);
        [
            number(score.map(|s| s.expected)),
            number(score.map(|s| s.found)),
            number(score.map(|s| s.extra)),
        ]
    };
    let [actions_expected, actions_found, actions_extra] = section(|s| s.actions);
    let [decisions_expected, decisions_found, decisions_extra] = section(|s| s.decisions);
    let fields = [
        run_at.to_owned(),
        cell(label),
        cell(&o.case),
        cell(&o.stt),
        cell(o.llm.as_deref().unwrap_or("")),
        o.wer.reference.to_string(),
        number(o.wer.rate().map(|r| format!("{r:.4}"))),
        o.wer.substitutions.to_string(),
        o.wer.deletions.to_string(),
        o.wer.insertions.to_string(),
        o.terms.expected.to_string(),
        o.terms.found.to_string(),
        actions_expected,
        actions_found,
        actions_extra,
        decisions_expected,
        decisions_found,
        decisions_extra,
        number(o.summary.as_ref().map(|s| s.structured)),
        number(o.stt_secs.map(|s| format!("{s:.1}"))),
        number(o.llm_secs.map(|s| format!("{s:.1}"))),
        format!("{:.1}", o.audio_secs),
    ];
    fields.join("\t")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(case: &str, errors: usize, reference: usize) -> Outcome {
        Outcome {
            case: case.into(),
            stt: "sherpa/a".into(),
            llm: Some("ollama/g".into()),
            wer: WordErrors {
                reference,
                substitutions: errors,
                deletions: 0,
                insertions: 0,
            },
            terms: TermHits {
                expected: 4,
                found: 3,
            },
            absent_terms: Vec::new(),
            summary: Some(SummaryScore {
                structured: true,
                actions: SectionScore {
                    expected: 2,
                    found: 1,
                    extra: 3,
                },
                decisions: SectionScore {
                    expected: 1,
                    found: 1,
                    extra: 0,
                },
            }),
            stt_secs: Some(30.0),
            llm_secs: Some(4.0),
            audio_secs: 60.0,
        }
    }

    #[test]
    fn a_case_line_reads_left_to_right() {
        assert_eq!(
            case_line(&outcome("standup", 5, 100)),
            "standup  WER 5.0% (S 5 D 0 I 0 of 100)  terms 3/4 (75.0%)  actions 1/2 (+3 extra)  decisions 1/1 (+0 extra)  structured  stt 30.0 s (0.50x real time)  llm 4.0 s"
        );
    }

    #[test]
    fn a_summary_only_case_without_scores_says_so() {
        let mut o = outcome("standup", 0, 0);
        o.summary = None;
        o.stt_secs = None;
        let line = case_line(&o);
        assert!(line.contains("WER n/a"), "{line}");
        assert!(
            line.contains("summary not scored  stt -  llm 4.0 s"),
            "{line}"
        );
    }

    #[test]
    fn totals_weigh_each_case_by_its_words() {
        let lines = totals(&[outcome("short", 5, 10), outcome("long", 5, 90)]);
        assert!(
            lines[0]
                .starts_with("total (2 cases)  WER 10.0% (S 10 D 0 I 0 of 100)  terms 6/8 (75.0%)"),
            "{}",
            lines[0]
        );
        assert!(lines[0].contains("actions 2/4 (+6 extra)"), "{}", lines[0]);
        assert!(
            lines[0].contains("stt 60.0 s (0.50x real time)"),
            "{}",
            lines[0]
        );
        assert_eq!(lines[1], "stt sherpa/a  llm ollama/g");
    }

    #[test]
    fn a_tsv_row_lines_up_with_the_header() {
        let row = tsv_row("2026-11-19 10:00:00", "base\tline", &outcome("a b", 5, 100));
        let cells: Vec<&str> = row.split('\t').collect();
        assert_eq!(cells.len(), TSV_HEADER.split('\t').count());
        assert_eq!(cells[1], "base line");
        assert_eq!(cells[6], "0.0500");
        assert_eq!(cells[12..15], ["2", "1", "3"]);
        assert_eq!(cells[18], "true");
        assert_eq!(cells[19..], ["30.0", "4.0", "60.0"]);
    }

    #[test]
    fn unscored_values_leave_their_cells_empty() {
        let mut o = outcome("a", 0, 0);
        o.summary = None;
        o.llm = None;
        o.stt_secs = None;
        let row = tsv_row("t", "", &o);
        let cells: Vec<&str> = row.split('\t').collect();
        assert_eq!(cells.len(), TSV_HEADER.split('\t').count());
        assert_eq!(cells[4], "");
        assert_eq!(cells[6], "");
        assert!(cells[12..19].iter().all(|c| c.is_empty()));
        assert_eq!(cells[19], "");
    }
}
