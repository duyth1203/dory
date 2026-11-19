//! A case folder: one audio file and the plain-text files that say what it should become. The
//! parsers take text, so they run without a folder.

use std::path::{Path, PathBuf};

use dory_core::domain::LangTag;

use super::score::words;

/// The extensions of the audio files a case may hold. The takes importer reads `wav` and `flac`
/// itself and hands every other extension to ffmpeg, so these are the common ones ffmpeg decodes.
const AUDIO_EXTENSIONS: [&str; 12] = [
    "wav", "flac", "mp3", "m4a", "aac", "ogg", "opus", "aif", "aiff", "caf", "mp4", "webm",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Action,
    Decision,
}

/// Alternatives for one idea, each a folded word sequence. The idea is present when any one of
/// them appears in a bullet as consecutive words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Concept(pub Vec<Vec<String>>);

/// One gold item. It is found when every concept appears in the same bullet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expected {
    pub kind: Kind,
    pub concepts: Vec<Concept>,
    /// The line as written, to name the item in a report.
    #[allow(dead_code, reason = "for naming missed items in a report")]
    pub line: String,
}

#[derive(Debug, PartialEq)]
pub struct Case {
    pub name: String,
    pub audio: PathBuf,
    pub reference: String,
    /// Pins the transcription language. `None` keeps the recording's own preference.
    pub language: Option<LangTag>,
    pub terms: Vec<String>,
    pub expected: Vec<Expected>,
}

impl Case {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let name = dir
            .file_name()
            .map_or_else(|| dir.display().to_string(), |n| n.to_string_lossy().into());
        let in_case = |what: String| format!("case {name}: {what}");
        let read = |file: &str| -> Result<Option<String>, String> {
            match std::fs::read_to_string(dir.join(file)) {
                Ok(text) => Ok(Some(text)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(in_case(format!("cannot read {file}: {e}"))),
            }
        };
        let reference =
            read("reference.txt")?.ok_or_else(|| in_case("reference.txt is missing".into()))?;
        if words(&reference).is_empty() {
            return Err(in_case("reference.txt has no words".into()));
        }
        let terms = match read("terms.txt")? {
            Some(text) => parse_terms(&text).map_err(&in_case)?,
            None => Vec::new(),
        };
        let expected = match read("expected.txt")? {
            Some(text) => parse_expected(&text).map_err(&in_case)?,
            None => Vec::new(),
        };
        let language = match read("language")? {
            Some(text) => {
                Some(LangTag::parse(text.trim()).map_err(|e| in_case(format!("language: {e}")))?)
            }
            None => None,
        };
        let audio = find_audio(dir).map_err(&in_case)?;
        Ok(Self {
            name,
            audio,
            reference,
            language,
            terms,
            expected,
        })
    }
}

/// The case folders under `root`, sorted by name: `root` itself when it holds `reference.txt`,
/// else each subfolder that does.
pub fn discover(root: &Path) -> Result<Vec<PathBuf>, String> {
    if root.join("reference.txt").is_file() {
        return Ok(vec![root.to_path_buf()]);
    }
    let entries = std::fs::read_dir(root)
        .map_err(|e| format!("cannot read case folder {}: {e}", root.display()))?;
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("reference.txt").is_file())
        .collect();
    found.sort();
    if found.is_empty() {
        return Err(format!(
            "{} holds no case: a case folder has a reference.txt, and so does each folder inside a folder of cases",
            root.display()
        ));
    }
    Ok(found)
}

fn find_audio(dir: &Path) -> Result<PathBuf, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("cannot read the folder: {e}"))?;
    let mut audio: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && is_audio(path))
        .collect();
    audio.sort();
    let accepted = || AUDIO_EXTENSIONS.join(", ");
    match audio.len() {
        1 => Ok(audio.remove(0)),
        0 => Err(format!(
            "no audio file (accepted extensions: {})",
            accepted()
        )),
        n => Err(format!(
            "{n} audio files, a case has exactly one (accepted extensions: {})",
            accepted()
        )),
    }
}

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// The lines of a list file that count: not blank, not a `#` comment. Numbered from 1.
fn content_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(i, line)| (i + 1, line.trim()))
        .filter(|(_, line)| !line.is_empty() && !line.starts_with('#'))
}

/// One term per line. A term with no word in it can never match, so it is an error.
pub fn parse_terms(text: &str) -> Result<Vec<String>, String> {
    content_lines(text)
        .map(|(n, line)| {
            if words(line).is_empty() {
                Err(format!("terms.txt line {n}: {line:?} has no words"))
            } else {
                Ok(line.to_owned())
            }
        })
        .collect()
}

/// `action: c1, c2` or `decision: c1, c2|alt`.
pub fn parse_expected(text: &str) -> Result<Vec<Expected>, String> {
    content_lines(text)
        .map(|(n, line)| parse_item(line).map_err(|why| format!("expected.txt line {n}: {why}")))
        .collect()
}

fn parse_item(line: &str) -> Result<Expected, String> {
    let (head, rest) = line
        .split_once(':')
        .ok_or("expected `action: ...` or `decision: ...`")?;
    let kind = match head.trim().to_ascii_lowercase().as_str() {
        "action" => Kind::Action,
        "decision" => Kind::Decision,
        other => return Err(format!("{other:?} is not action or decision")),
    };
    let concepts = rest
        .split(',')
        .map(parse_concept)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Expected {
        kind,
        concepts,
        line: line.to_owned(),
    })
}

fn parse_concept(text: &str) -> Result<Concept, String> {
    let alternatives = text
        .split('|')
        .map(|alternative| match words(alternative) {
            w if w.is_empty() => Err(format!("{:?} has no words", alternative.trim())),
            w => Ok(w),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Concept(alternatives))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn concept(alternatives: &[&str]) -> Concept {
        Concept(alternatives.iter().map(|a| words(a)).collect())
    }

    #[test]
    fn terms_skip_blank_and_comment_lines() {
        let terms = parse_terms("# names\nOmar\n\n  Lena Park  \n#Skipped\nKubernetes\n").unwrap();
        assert_eq!(terms, ["Omar", "Lena Park", "Kubernetes"]);
        assert!(parse_terms("").unwrap().is_empty());
    }

    #[test]
    fn a_term_with_no_word_names_its_line() {
        let error = parse_terms("Omar\n\n---\n").unwrap_err();
        assert!(error.starts_with("terms.txt line 3"), "{error}");
    }

    #[test]
    fn expected_items_split_into_concepts_and_alternatives() {
        let items = parse_expected(
            "# gold\naction: Omar, trial limits|usage caps, by Monday\n\nDecision: ship friday\n",
        )
        .unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].kind, Kind::Action);
        assert_eq!(
            items[0].concepts,
            [
                concept(&["omar"]),
                concept(&["trial limits", "usage caps"]),
                concept(&["by monday"]),
            ]
        );
        assert_eq!(
            items[0].line,
            "action: Omar, trial limits|usage caps, by Monday"
        );
        assert_eq!(items[1].kind, Kind::Decision);
        assert_eq!(items[1].concepts, [concept(&["ship friday"])]);
    }

    #[test]
    fn concepts_fold_like_the_transcript_does() {
        let items = parse_expected("action: Zoë's Café|The Deck\n").unwrap();
        assert_eq!(
            items[0].concepts,
            [Concept(vec![
                vec!["zoe".to_owned(), "s".to_owned(), "cafe".to_owned()],
                vec!["the".to_owned(), "deck".to_owned()],
            ])]
        );
    }

    #[test]
    fn a_bad_expected_line_names_the_file_and_line() {
        for (text, line) in [
            ("action: a\nnote: b\n", 2),
            ("\n\naction a\n", 3),
            ("decision: a,, b\n", 1),
            ("action: a|\n", 1),
            ("# c\naction:\n", 2),
        ] {
            let error = parse_expected(text).unwrap_err();
            assert!(
                error.starts_with(&format!("expected.txt line {line}:")),
                "{text:?} gave {error}"
            );
        }
    }

    #[test]
    fn a_case_loads_from_its_folder() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("standup");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("take.WAV"), b"").unwrap();
        std::fs::write(dir.join("reference.txt"), "Omar ships Friday.").unwrap();
        std::fs::write(dir.join("terms.txt"), "Omar\n").unwrap();
        std::fs::write(dir.join("expected.txt"), "decision: ship friday\n").unwrap();
        std::fs::write(dir.join("language"), "fil-PH\n").unwrap();
        let case = Case::load(&dir).unwrap();
        assert_eq!(case.name, "standup");
        assert_eq!(case.audio, dir.join("take.WAV"));
        assert_eq!(case.language, Some(LangTag::parse("fil-PH").unwrap()));
        assert_eq!(case.terms, ["Omar"]);
        assert_eq!(case.expected.len(), 1);
    }

    #[test]
    fn a_case_needs_exactly_one_audio_file_and_a_reference() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        let error = Case::load(dir).unwrap_err();
        assert!(error.contains("reference.txt is missing"), "{error}");
        std::fs::write(dir.join("reference.txt"), "hello").unwrap();
        let error = Case::load(dir).unwrap_err();
        assert!(
            error.contains("no audio file") && error.contains("flac"),
            "{error}"
        );
        std::fs::write(dir.join("a.mp3"), b"").unwrap();
        std::fs::write(dir.join("b.m4a"), b"").unwrap();
        let error = Case::load(dir).unwrap_err();
        assert!(error.contains("2 audio files"), "{error}");
        std::fs::remove_file(dir.join("b.m4a")).unwrap();
        std::fs::write(dir.join("language"), "not a tag").unwrap();
        let error = Case::load(dir).unwrap_err();
        assert!(error.contains("language"), "{error}");
        std::fs::remove_file(dir.join("language")).unwrap();
        assert!(Case::load(dir).is_ok());
        std::fs::write(dir.join("reference.txt"), " \n ").unwrap();
        assert!(Case::load(dir).unwrap_err().contains("no words"));
    }

    #[test]
    fn discovery_takes_one_case_or_each_case_inside_a_folder_sorted() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        assert!(discover(root).is_err());
        for name in ["b", "a", "notes"] {
            std::fs::create_dir(root.join(name)).unwrap();
        }
        std::fs::write(root.join("b/reference.txt"), "x").unwrap();
        std::fs::write(root.join("a/reference.txt"), "x").unwrap();
        assert_eq!(discover(root).unwrap(), [root.join("a"), root.join("b")]);
        assert_eq!(discover(&root.join("a")).unwrap(), [root.join("a")]);
    }
}
