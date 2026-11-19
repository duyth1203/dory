use std::path::PathBuf;

use dory_core::domain::{ChannelLayout, LangTag, Ms, Processing, RecordingId, Recovery};
use dory_core::models::{Llm, ModelId, ModelKind, Stt};
use dory_core::ports::capture::MicSelection;

#[derive(Debug, PartialEq)]
pub struct ImportArgs {
    pub path: PathBuf,
}

impl ImportArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        match args {
            [path] if !path.starts_with("--") => Ok(Self { path: path.into() }),
            _ => Err("import takes one argument: the old data folder".into()),
        }
    }
}

/// `add FILE [--title TEXT]` imports an audio file as a new recording.
#[derive(Debug, PartialEq)]
pub struct AddArgs {
    pub path: PathBuf,
    pub title: Option<String>,
}

impl AddArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        match args {
            [path] if !path.starts_with("--") => Ok(Self {
                path: path.into(),
                title: None,
            }),
            [path, flag, title] if flag == "--title" && !path.starts_with("--") => Ok(Self {
                path: path.into(),
                title: Some(title.clone()),
            }),
            _ => Err("add takes an audio file and an optional --title TEXT".into()),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct ListArgs;

impl ListArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        match args {
            [] => Ok(Self),
            _ => Err("list takes no arguments; search with dory-cli search TEXT".into()),
        }
    }
}

/// `search TEXT...`: the words, joined by spaces, as typed into the search field.
#[derive(Debug, PartialEq)]
pub struct SearchArgs {
    pub text: String,
}

impl SearchArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let text = args.join(" ");
        if text.trim().is_empty() {
            return Err("search needs the text to look for".into());
        }
        Ok(Self { text })
    }
}

/// `seed --recordings N --words N [--seed N]` writes synthetic recordings for search lanes.
#[derive(Debug, PartialEq)]
pub struct SeedArgs {
    pub recordings: u32,
    pub words: u32,
    pub seed: u64,
}

impl SeedArgs {
    /// Below this the planted words would share recordings.
    pub const MIN_RECORDINGS: u32 = 20;

    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut recordings = None;
        let mut words = None;
        let mut seed = 1;
        let mut it = args.iter();
        while let Some(flag) = it.next() {
            let value = it.next().ok_or_else(|| format!("{flag} needs a number"))?;
            let number = |v: &str| v.parse::<u64>().map_err(|e| format!("{flag}: {e}"));
            match flag.as_str() {
                "--recordings" => recordings = Some(number(value)?),
                "--words" => words = Some(number(value)?),
                "--seed" => seed = number(value)?,
                other => return Err(format!("unknown flag {other}")),
            }
        }
        let recordings = recordings.ok_or("seed needs --recordings N")?;
        let words = words.ok_or("seed needs --words N")?;
        if recordings < u64::from(Self::MIN_RECORDINGS) || recordings > u64::from(u16::MAX) {
            return Err(format!(
                "--recordings takes {} to {}",
                Self::MIN_RECORDINGS,
                u16::MAX
            ));
        }
        if !(30..=1_000_000).contains(&words) {
            return Err("--words takes 30 to 1000000".into());
        }
        Ok(Self {
            recordings: recordings as u32,
            words: words as u32,
            seed,
        })
    }
}

/// `show ID [--languages] [--original] [--summary-translation]`. With `--languages` it prints only
/// the languages heard, most heard first.
#[derive(Debug, PartialEq)]
pub struct ShowArgs {
    pub id: RecordingId,
    pub languages: bool,
    /// Prints the spoken transcript even when a translation exists.
    pub original: bool,
    /// Also prints the summary's saved translation, when there is one.
    pub summary_translation: bool,
}

impl ShowArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let usage =
            "show takes the recording id, --languages, --original, and --summary-translation";
        let mut id = None;
        let (mut languages, mut original, mut summary_translation) = (false, false, false);
        for arg in args {
            match arg.as_str() {
                "--languages" => languages = true,
                "--original" => original = true,
                "--summary-translation" => summary_translation = true,
                _ if arg.starts_with("--") => return Err(usage.into()),
                _ if id.is_none() => id = Some(parse_id(arg)?),
                _ => return Err(usage.into()),
            }
        }
        Ok(Self {
            id: id.ok_or(usage)?,
            languages,
            original,
            summary_translation,
        })
    }
}

/// `translate-summary ID TAG` writes the recording's summary in another language, beside the
/// original, and prints it.
#[derive(Debug, PartialEq)]
pub struct TranslateSummaryArgs {
    pub id: RecordingId,
    pub target: LangTag,
    pub fake_engines: bool,
    /// Translates with this model instead of the Settings choice.
    pub llm: Option<ModelId<Llm>>,
}

impl TranslateSummaryArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let usage = "translate-summary takes a recording id and a language tag";
        let mut words = Vec::new();
        let mut fake_engines = false;
        let mut llm = None;
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--fake-engines" => fake_engines = true,
                "--llm" => llm = Some(model_arg("--llm", it.next())?),
                _ if arg.starts_with("--") => return Err(format!("unknown flag {arg}")),
                _ => words.push(arg),
            }
        }
        match words.as_slice() {
            [id, tag] => Ok(Self {
                id: parse_id(id)?,
                target: LangTag::parse(tag).map_err(|e| e.to_string())?,
                fake_engines,
                llm,
            }),
            _ => Err(usage.into()),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct DeleteArgs {
    pub id: RecordingId,
}

impl DeleteArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        match args {
            [id] => Ok(Self { id: parse_id(id)? }),
            _ => Err("delete takes one argument: the recording id".into()),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct ProcessArgs {
    pub id: RecordingId,
    pub fake_engines: bool,
    /// Summarizes the stored transcript again without transcribing.
    pub summary_only: bool,
    /// Overrides the Processing setting for this run, for the bench.
    pub pace: Option<Processing>,
    /// Transcribes with this model instead of the Settings choice.
    pub stt: Option<ModelId<Stt>>,
    /// Summarizes with this model instead of the Settings choice. Only with `--summary`.
    pub llm: Option<ModelId<Llm>>,
}

/// A model id after `flag`, of the kind the flag picks.
fn model_arg<K: ModelKind>(flag: &str, word: Option<&String>) -> Result<ModelId<K>, String> {
    word.ok_or_else(|| format!("{flag} needs a model id"))?
        .parse()
        .map_err(|e| format!("{flag}: {e}"))
}

impl ProcessArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut id = None;
        let mut fake_engines = false;
        let mut summary_only = false;
        let mut pace = None;
        let mut stt = None;
        let mut llm = None;
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--fake-engines" => fake_engines = true,
                "--summary" => summary_only = true,
                "--stt" => stt = Some(model_arg("--stt", it.next())?),
                "--llm" => llm = Some(model_arg("--llm", it.next())?),
                "--pace" => {
                    pace = Some(match it.next().map(String::as_str) {
                        Some("fast") => Processing::Fast,
                        Some("light") => Processing::Light,
                        _ => return Err("--pace takes fast or light".into()),
                    })
                }
                flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}")),
                _ if id.is_none() => id = Some(parse_id(arg)?),
                _ => return Err("process takes one recording id".into()),
            }
        }
        match (summary_only, &stt, &llm) {
            (true, Some(_), _) => {
                return Err("--stt transcribes, so it does not go with --summary".into());
            }
            (false, _, Some(_)) => {
                return Err("--llm picks the summary model, so it needs --summary".into());
            }
            _ => {}
        }
        Ok(Self {
            id: id.ok_or("process needs a recording id")?,
            fake_engines,
            summary_only,
            pace,
            stt,
            llm,
        })
    }
}

#[derive(Debug, PartialEq)]
pub struct PlayArgs {
    pub id: RecordingId,
    pub seek: Ms,
}

impl PlayArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut id = None;
        let mut seek = Ms(0);
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--seek" => {
                    let text = it.next().ok_or("--seek needs a value in seconds")?;
                    let seconds: f64 = text.parse().map_err(|e| format!("--seek: {e}"))?;
                    if !seconds.is_finite() || seconds < 0.0 {
                        return Err("--seek needs a number of seconds, 0 or more".into());
                    }
                    seek = Ms((seconds * 1000.0).round() as u64);
                }
                flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}")),
                _ if id.is_none() => id = Some(parse_id(arg)?),
                _ => return Err("play takes one recording id".into()),
            }
        }
        Ok(Self {
            id: id.ok_or("play needs a recording id")?,
            seek,
        })
    }
}

#[derive(Debug, PartialEq)]
pub struct ModelsArgs {
    pub fake_engines: bool,
    /// Prints each model's explainer: what it is, what it is good at, and what to watch out for.
    pub about: bool,
}

impl ModelsArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut parsed = Self {
            fake_engines: false,
            about: false,
        };
        for arg in args {
            match arg.as_str() {
                "--fake-engines" => parsed.fake_engines = true,
                "--about" => parsed.about = true,
                _ => return Err("models takes --fake-engines and --about".into()),
            }
        }
        Ok(parsed)
    }
}

/// Which model list a setting applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KindArg {
    Stt,
    Llm,
    Diar,
}

impl KindArg {
    fn parse(word: &str) -> Result<Self, String> {
        match word {
            "stt" | "speech" => Ok(Self::Stt),
            "llm" | "summary" => Ok(Self::Llm),
            "diar" | "speakers" => Ok(Self::Diar),
            other => Err(format!(
                "{other:?} is not a model kind: use stt, llm, or diar"
            )),
        }
    }
}

/// `models choose KIND auto|off|ID [--fake-engines]`. The choice stays a string here because its type depends on
/// the kind, which the command resolves.
#[derive(Debug, PartialEq)]
pub struct ChooseArgs {
    pub kind: KindArg,
    pub choice: String,
    /// Checks the id against the fake catalogs, so a lane can pin a fake model.
    pub fake_engines: bool,
}

impl ChooseArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let (args, fake_engines) = match args {
            [rest @ .., flag] if flag == "--fake-engines" => (rest, true),
            _ => (args, false),
        };
        match args {
            [kind, choice] => Ok(Self {
                kind: KindArg::parse(kind)?,
                choice: choice.clone(),
                fake_engines,
            }),
            _ => Err(
                "models choose takes a kind and auto, off, or a model id, then --fake-engines"
                    .into(),
            ),
        }
    }
}

/// `models provision KIND ID`.
#[derive(Debug, PartialEq)]
pub struct ProvisionArgs {
    pub kind: KindArg,
    pub id: String,
}

impl ProvisionArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        match args {
            [kind, id] => Ok(Self {
                kind: KindArg::parse(kind)?,
                id: id.clone(),
            }),
            _ => Err("models provision takes a kind and a model id".into()),
        }
    }
}

/// `languages [TAG...]` prints the meeting languages, or replaces them. `languages --clear`
/// removes them all.
#[derive(Debug, PartialEq)]
pub enum LanguagesArgs {
    Show,
    Set(Vec<LangTag>),
}

impl LanguagesArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        match args {
            [] => Ok(Self::Show),
            [flag] if flag == "--clear" => Ok(Self::Set(Vec::new())),
            tags => tags
                .iter()
                .map(|t| LangTag::parse(t).map_err(|e| e.to_string()))
                .collect::<Result<_, _>>()
                .map(Self::Set),
        }
    }
}

/// `switch-recovery [on|off|auto]` prints whether transcription recovers speech lost when the
/// language changes, or sets it.
#[derive(Debug, PartialEq)]
pub enum SwitchRecoveryArgs {
    Show,
    Set(Recovery),
}

impl SwitchRecoveryArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        match args {
            [] => Ok(Self::Show),
            [word] => match word.as_str() {
                "on" => Ok(Self::Set(Recovery::On)),
                "off" => Ok(Self::Set(Recovery::Off)),
                "auto" => Ok(Self::Set(Recovery::Auto)),
                _ => Err(format!(
                    "switch-recovery takes on, off, or auto, not {word:?}"
                )),
            },
            _ => Err("switch-recovery takes one of on, off, or auto".into()),
        }
    }
}

/// `output-language` shows the language transcripts and summaries are written in. A tag sets it,
/// and `same` goes back to writing in the language that was spoken.
#[derive(Debug, PartialEq)]
pub enum OutputLanguageArgs {
    Show,
    Set(Option<LangTag>),
}

impl OutputLanguageArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        match args {
            [] => Ok(Self::Show),
            [word] if word == "same" => Ok(Self::Set(None)),
            [tag] => LangTag::parse(tag)
                .map(|tag| Self::Set(Some(tag)))
                .map_err(|e| e.to_string()),
            _ => Err("output-language takes one language tag, or same".into()),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct RecordArgs {
    pub layout: ChannelLayout,
    pub mic: MicSelection,
    /// Records this many seconds from a scripted fake capture instead of the machine's audio.
    pub fake_capture: Option<u32>,
}

impl RecordArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut layout = ChannelLayout::MicSystem;
        let mut mic = MicSelection::SystemDefault;
        let mut fake_capture = None;
        let mut it = args.iter();
        while let Some(flag) = it.next() {
            let mut value = || it.next().ok_or_else(|| format!("{flag} needs a value"));
            match flag.as_str() {
                "--layout" => layout = parse_layout(value()?)?,
                "--mic" => mic = MicSelection::Uid(value()?.clone()),
                "--fake-capture" => {
                    let seconds: u32 = value()?
                        .parse()
                        .map_err(|e| format!("--fake-capture: {e}"))?;
                    if seconds == 0 {
                        return Err("--fake-capture needs at least 1 second".into());
                    }
                    fake_capture = Some(seconds);
                }
                other => return Err(format!("unknown flag {other}")),
            }
        }
        Ok(Self {
            layout,
            mic,
            fake_capture,
        })
    }
}

fn parse_id(s: &str) -> Result<RecordingId, String> {
    RecordingId::parse(s).map_err(|e| e.to_string())
}

fn parse_layout(s: &str) -> Result<ChannelLayout, String> {
    match s {
        "mic-system" => Ok(ChannelLayout::MicSystem),
        "mic" => Ok(ChannelLayout::MicOnly),
        "system" => Ok(ChannelLayout::SystemOnly),
        other => Err(format!("--layout: unknown layout {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn record_flags() {
        let a = RecordArgs::parse(&words("--layout mic --mic UID1 --fake-capture 3")).unwrap();
        assert_eq!(a.layout, ChannelLayout::MicOnly);
        assert_eq!(a.mic, MicSelection::Uid("UID1".into()));
        assert_eq!(a.fake_capture, Some(3));
        let d = RecordArgs::parse(&[]).unwrap();
        assert_eq!(d.layout, ChannelLayout::MicSystem);
        assert_eq!(d.fake_capture, None);
        assert!(RecordArgs::parse(&words("--fake-capture 0")).is_err());
        assert!(RecordArgs::parse(&words("--layout both")).is_err());
        assert!(RecordArgs::parse(&words("--mic")).is_err());
    }

    #[test]
    fn play_takes_an_id_and_an_optional_seek_in_seconds() {
        let a = PlayArgs::parse(&words("rec_1 --seek 3.5")).unwrap();
        assert_eq!((a.id.as_str(), a.seek), ("rec_1", Ms(3_500)));
        assert_eq!(
            PlayArgs::parse(&words("--seek 1 rec_1")).unwrap().seek,
            Ms(1_000)
        );
        assert_eq!(PlayArgs::parse(&words("rec_1")).unwrap().seek, Ms(0));
        assert!(PlayArgs::parse(&[]).is_err());
        assert!(PlayArgs::parse(&words("--seek 3")).is_err());
        assert!(PlayArgs::parse(&words("rec_1 --seek")).is_err());
        assert!(PlayArgs::parse(&words("rec_1 --seek -2")).is_err());
        assert!(PlayArgs::parse(&words("rec_1 --seek soon")).is_err());
        assert!(PlayArgs::parse(&words("rec_1 --loud")).is_err());
        assert!(PlayArgs::parse(&words("rec_1 rec_2")).is_err());
    }

    #[test]
    fn positional_commands() {
        assert_eq!(ListArgs::parse(&[]), Ok(ListArgs));
        assert!(ListArgs::parse(&words("--search roadmap")).is_err());
        assert_eq!(
            SearchArgs::parse(&words("review budget")).unwrap().text,
            "review budget"
        );
        assert_eq!(
            SearchArgs::parse(&["\"budget review\"".into()])
                .unwrap()
                .text,
            "\"budget review\""
        );
        assert!(SearchArgs::parse(&[]).is_err());
        assert!(SearchArgs::parse(&words("  ")).is_err());
        assert_eq!(
            SeedArgs::parse(&words("--recordings 500 --words 18000")).unwrap(),
            SeedArgs {
                recordings: 500,
                words: 18_000,
                seed: 1
            }
        );
        assert_eq!(
            SeedArgs::parse(&words("--seed 7 --words 40 --recordings 20"))
                .unwrap()
                .seed,
            7
        );
        assert!(SeedArgs::parse(&words("--recordings 500")).is_err());
        assert!(SeedArgs::parse(&words("--recordings 19 --words 100")).is_err());
        assert!(SeedArgs::parse(&words("--recordings 50 --words 10")).is_err());
        assert!(SeedArgs::parse(&words("--recordings 50 --words")).is_err());
        assert!(SeedArgs::parse(&words("--recordings x --words 100")).is_err());
        assert!(ShowArgs::parse(&words("rec_1")).is_ok());
        assert!(ShowArgs::parse(&words("../etc")).is_err());
        assert!(ShowArgs::parse(&[]).is_err());
        assert!(!ShowArgs::parse(&words("rec_1")).unwrap().languages);
        for form in ["rec_1 --languages", "--languages rec_1"] {
            let show = ShowArgs::parse(&words(form)).unwrap();
            assert_eq!(show.id, RecordingId::parse("rec_1").unwrap(), "{form}");
            assert!(show.languages, "{form}");
        }
        assert!(ShowArgs::parse(&words("--languages")).is_err());
        assert!(ShowArgs::parse(&words("rec_1 --lang")).is_err());
        assert!(DeleteArgs::parse(&words("rec_1")).is_ok());
        assert!(DeleteArgs::parse(&words("rec_1 rec_2")).is_err());
        let p = ProcessArgs::parse(&words("--fake-engines rec_1")).unwrap();
        assert!(p.fake_engines);
        assert_eq!(p.id.as_str(), "rec_1");
        assert!(ProcessArgs::parse(&words("--fake-engines")).is_err());
        assert!(ProcessArgs::parse(&words("a b")).is_err());
        assert!(
            ProcessArgs::parse(&words("rec_1 --summary"))
                .unwrap()
                .summary_only
        );
        assert_eq!(
            ProcessArgs::parse(&words("rec_1 --stt a/x")).unwrap().stt,
            Some("a/x".parse().unwrap())
        );
        assert_eq!(
            ProcessArgs::parse(&words("rec_1 --summary --llm a/y"))
                .unwrap()
                .llm,
            Some("a/y".parse().unwrap())
        );
        for bad in [
            "rec_1 --llm a/y",
            "rec_1 --summary --stt a/x",
            "rec_1 --stt",
            "rec_1 --stt nonsense",
        ] {
            assert!(ProcessArgs::parse(&words(bad)).is_err(), "{bad}");
        }
        assert_eq!(
            ProcessArgs::parse(&words("rec_1 --pace light"))
                .unwrap()
                .pace,
            Some(Processing::Light)
        );
        assert_eq!(
            ProcessArgs::parse(&words("--pace fast rec_1"))
                .unwrap()
                .pace,
            Some(Processing::Fast)
        );
        assert_eq!(ProcessArgs::parse(&words("rec_1")).unwrap().pace, None);
        assert!(ProcessArgs::parse(&words("rec_1 --pace auto")).is_err());
        assert!(ProcessArgs::parse(&words("rec_1 --pace")).is_err());
        assert!(ImportArgs::parse(&words("/tmp/old")).is_ok());
        assert!(ImportArgs::parse(&[]).is_err());
        assert_eq!(
            AddArgs::parse(&words("talk.wav --title Planning")).unwrap(),
            AddArgs {
                path: "talk.wav".into(),
                title: Some("Planning".into())
            }
        );
        assert!(AddArgs::parse(&words("--title x")).is_err());
        assert!(
            ModelsArgs::parse(&words("--fake-engines"))
                .unwrap()
                .fake_engines
        );
        assert!(ModelsArgs::parse(&words("--about")).unwrap().about);
    }

    #[test]
    fn model_settings_and_meeting_languages_parse() {
        assert_eq!(
            ChooseArgs::parse(&words("stt sherpa/whisper-turbo")).unwrap(),
            ChooseArgs {
                kind: KindArg::Stt,
                choice: "sherpa/whisper-turbo".into(),
                fake_engines: false,
            }
        );
        assert!(
            ChooseArgs::parse(&words("llm fake/en-only-llm --fake-engines"))
                .unwrap()
                .fake_engines
        );
        assert!(ChooseArgs::parse(&words("video auto")).is_err());
        assert!(ChooseArgs::parse(&words("stt")).is_err());
        assert_eq!(
            ProvisionArgs::parse(&words("llm ollama/gemma4:e4b"))
                .unwrap()
                .kind,
            KindArg::Llm
        );
        assert_eq!(LanguagesArgs::parse(&[]).unwrap(), LanguagesArgs::Show);
        assert_eq!(
            LanguagesArgs::parse(&words("tl en")).unwrap(),
            LanguagesArgs::Set(vec![
                LangTag::parse("tl").unwrap(),
                LangTag::parse("en").unwrap()
            ])
        );
        assert_eq!(
            LanguagesArgs::parse(&words("--clear")).unwrap(),
            LanguagesArgs::Set(Vec::new())
        );
        assert!(LanguagesArgs::parse(&words("not_a_tag!")).is_err());
    }

    #[test]
    fn switch_recovery_parses() {
        assert_eq!(
            SwitchRecoveryArgs::parse(&[]).unwrap(),
            SwitchRecoveryArgs::Show
        );
        for (word, want) in [
            ("on", Recovery::On),
            ("off", Recovery::Off),
            ("auto", Recovery::Auto),
        ] {
            assert_eq!(
                SwitchRecoveryArgs::parse(&words(word)).unwrap(),
                SwitchRecoveryArgs::Set(want)
            );
        }
        assert!(SwitchRecoveryArgs::parse(&words("maybe")).is_err());
        assert!(SwitchRecoveryArgs::parse(&words("on off")).is_err());
    }

    #[test]
    fn output_language_shows_sets_or_goes_back_to_same_as_spoken() {
        assert_eq!(
            OutputLanguageArgs::parse(&[]).unwrap(),
            OutputLanguageArgs::Show
        );
        assert_eq!(
            OutputLanguageArgs::parse(&words("en")).unwrap(),
            OutputLanguageArgs::Set(Some(LangTag::parse("en").unwrap()))
        );
        assert_eq!(
            OutputLanguageArgs::parse(&words("same")).unwrap(),
            OutputLanguageArgs::Set(None)
        );
        assert!(OutputLanguageArgs::parse(&words("not_a_tag!")).is_err());
        assert!(OutputLanguageArgs::parse(&words("en fr")).is_err());
    }

    #[test]
    fn show_takes_an_id_and_an_optional_original_flag() {
        assert!(!ShowArgs::parse(&words("r1")).unwrap().original);
        assert!(ShowArgs::parse(&words("r1 --original")).unwrap().original);
        assert!(ShowArgs::parse(&words("--original r1")).unwrap().original);
        assert!(ShowArgs::parse(&words("--original")).is_err());
        assert!(ShowArgs::parse(&words("r1 --nope")).is_err());
        assert!(ShowArgs::parse(&words("r1 r2")).is_err());
    }

    #[test]
    fn show_prints_a_saved_summary_translation_only_when_asked() {
        let plain = ShowArgs::parse(&words("r1")).unwrap();
        assert!(!plain.summary_translation);
        let asked = ShowArgs::parse(&words("r1 --summary-translation --original")).unwrap();
        assert!(asked.summary_translation && asked.original);
        assert!(
            ShowArgs::parse(&words("--summary-translation r1"))
                .unwrap()
                .summary_translation
        );
    }

    #[test]
    fn translate_summary_takes_an_id_and_a_language_tag() {
        assert_eq!(
            TranslateSummaryArgs::parse(&words("r1 en")).unwrap(),
            TranslateSummaryArgs {
                id: RecordingId::parse("r1").unwrap(),
                target: LangTag::parse("en").unwrap(),
                fake_engines: false,
                llm: None,
            }
        );
        assert_eq!(
            TranslateSummaryArgs::parse(&words("r1 en --llm a/x"))
                .unwrap()
                .llm,
            Some("a/x".parse().unwrap())
        );
        assert!(TranslateSummaryArgs::parse(&words("r1 en --llm")).is_err());
        assert!(
            TranslateSummaryArgs::parse(&words("--fake-engines r1 tl-PH"))
                .unwrap()
                .fake_engines
        );
        for bad in [
            "",
            "r1",
            "r1 en fr",
            "r1 not_a_tag!",
            "../etc en",
            "r1 en --nope",
        ] {
            assert!(TranslateSummaryArgs::parse(&words(bad)).is_err(), "{bad:?}");
        }
    }
}
