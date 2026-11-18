//! The three sections of a summary: key points, decisions, and action items, in that order.
//!
//! The summary prompt writes the headings in the meeting's language, so a section is found by its
//! position, never by its heading's words. A summary that does not have exactly three sections at
//! its top heading level is reported as unstructured, never guessed at.

/// A summary split into its sections.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sections {
    Three {
        key_points: String,
        decisions: String,
        action_items: String,
    },
    Unstructured,
}

/// The ATX heading level of a line, if it is a heading.
fn heading_level(line: &str) -> Option<usize> {
    let line = line.trim_start();
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    let rest = &line[hashes..];
    ((1..=6).contains(&hashes) && (rest.is_empty() || rest.starts_with(' '))).then_some(hashes)
}

pub fn sections(markdown: &str) -> Sections {
    let Some(level) = markdown.lines().filter_map(heading_level).min() else {
        return Sections::Unstructured;
    };
    let mut bodies: Vec<String> = Vec::new();
    for line in markdown.lines() {
        if heading_level(line) == Some(level) {
            bodies.push(String::new());
        } else if let Some(body) = bodies.last_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    match <[String; 3]>::try_from(bodies) {
        Ok([key_points, decisions, action_items]) => Sections::Three {
            key_points,
            decisions,
            action_items,
        },
        Err(_) => Sections::Unstructured,
    }
}

/// The list items of a section's body, without their markers. A section that says "None" in any
/// language has no list lines, so it has no items.
pub fn list_items(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(list_item)
        .map(str::to_owned)
        .collect()
}

fn list_item(line: &str) -> Option<&str> {
    let line = line.trim_start();
    let rest = ["- ", "* ", "+ "]
        .iter()
        .find_map(|m| line.strip_prefix(m))
        .or_else(|| {
            let digits = line.bytes().take_while(u8::is_ascii_digit).count();
            (digits > 0)
                .then(|| &line[digits..])
                .and_then(|r| r.strip_prefix(". ").or_else(|| r.strip_prefix(") ")))
        })?;
    let rest = rest
        .strip_prefix("[ ] ")
        .or_else(|| rest.strip_prefix("[x] "))
        .unwrap_or(rest)
        .trim();
    (!rest.is_empty()).then_some(rest)
}

/// A summary as the user should get it when the model started over or repeated itself: text
/// from a fourth top-level heading on is dropped, and a list item that repeats an earlier one in
/// its section is dropped. A small model can loop until its output cap, and the loop would
/// otherwise reach the user and leave the summary unstructured.
pub fn tidy(markdown: &str) -> String {
    let level = markdown.lines().filter_map(heading_level).min();
    let mut out = String::new();
    let mut headings = 0;
    let mut seen = std::collections::HashSet::new();
    for line in markdown.lines() {
        if level.is_some() && heading_level(line) == level {
            headings += 1;
            if headings > 3 {
                break;
            }
            seen.clear();
        } else if let Some(item) = list_item(line)
            && !seen.insert(item.to_lowercase())
        {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    format!("{}\n", out.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(md: &str) -> Option<Vec<String>> {
        match sections(md) {
            Sections::Three { action_items, .. } => Some(list_items(&action_items)),
            Sections::Unstructured => None,
        }
    }

    /// Shortened from Apple Intelligence's summary of a 10-minute meeting, which wrote the three
    /// sections, then started them over nine times, one line 289 times in all.
    const LOOPED: &str = "## Key Points\n\n* Ship a simple version first.\n* Ship a simple version first.\n* Some customers reported confusion.\n* Ship a simple version first.\n\n## Decisions\n\n* Ship a simple version first.\n* Keep the scope so it does not slip again.\n* ship a simple version first.\n\n## Action Items\n\n* Omar: trial limits by Monday\n* Lena: invoice template by Thursday\n* Omar: trial limits by Monday\n\n## Key Points\n\n* Ship a simple version first.\n* Ship a simple version first.\n\n## Decisions\n\n* Ship a simple version first.\n";

    #[test]
    fn a_summary_that_starts_over_keeps_its_first_three_sections_with_each_item_once() {
        let tidied = tidy(LOOPED);
        assert_eq!(
            tidied,
            "## Key Points\n\n* Ship a simple version first.\n* Some customers reported confusion.\n\n## Decisions\n\n* Ship a simple version first.\n* Keep the scope so it does not slip again.\n\n## Action Items\n\n* Omar: trial limits by Monday\n* Lena: invoice template by Thursday\n"
        );
        assert_eq!(sections(LOOPED), Sections::Unstructured);
        assert_eq!(
            items(&tidied),
            Some(vec![
                "Omar: trial limits by Monday".to_owned(),
                "Lena: invoice template by Thursday".to_owned()
            ])
        );
    }

    #[test]
    fn a_tidy_summary_is_left_as_it_is() {
        for md in [
            "## Key points\n- A\n\n## Decisions\n- Ship Friday\n\n## Action items\n- Ship Friday\n- [ ] Ana: notes\n",
            "## A\n- a\n## B\n- b\n",
            "Just prose, no headings.\nJust prose, no headings.\n",
            "# Title\n## One\n- x\n## Two\n- y\n## Three\n- z\n## Four\n- w\n",
        ] {
            assert_eq!(tidy(md), md, "{md}");
        }
    }

    #[test]
    fn the_third_section_is_the_action_items_in_any_language() {
        let cases: &[(&str, Option<&[&str]>)] = &[
            (
                "## Key points\n- A\n## Decisions\n- B\n## Action items\n- Ana: notes by Thursday\n- [ ] Ship Friday\n",
                Some(&["Ana: notes by Thursday", "Ship Friday"]),
            ),
            (
                "## Ý chính\n- A\n## Quyết định\n- B\n## Việc cần làm\n1. Minh gửi báo cáo\n2) Lan họp lại\n",
                Some(&["Minh gửi báo cáo", "Lan họp lại"]),
            ),
            (
                "# Mga pangunahing punto\n- A\n# Mga desisyon\nWala.\n# Mga gagawin\nWala.\n",
                Some(&[]),
            ),
            (
                "Intro line\n# One\n## Sub\n- x\n# Two\n- y\n# Three\n- z\n  - nested\n",
                Some(&["z", "nested"]),
            ),
            ("## A\n- a\n## B\n- b\n", None),
            ("## A\n## B\n## C\n## D\n- d\n", None),
            ("Just prose, no headings.\n- item\n", None),
            ("#NotAHeading\n- x\n", None),
        ];
        for (md, expected) in cases {
            let expected = expected.map(|e| e.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>());
            assert_eq!(items(md), expected, "{md}");
        }
    }
}
