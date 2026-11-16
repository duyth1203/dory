use std::path::PathBuf;

use dory_core::domain::{ChannelLayout, LangTag, Ms, RecordingId};
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

#[derive(Debug, PartialEq)]
pub struct ListArgs {
    pub search: Option<String>,
}

impl ListArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        match args {
            [] => Ok(Self { search: None }),
            [flag, text] if flag == "--search" => Ok(Self {
                search: Some(text.clone()),
            }),
            _ => Err("list takes only --search TEXT".into()),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct ShowArgs {
    pub id: RecordingId,
}

impl ShowArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        match args {
            [id] => Ok(Self { id: parse_id(id)? }),
            _ => Err("show takes one argument: the recording id".into()),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct ProcessArgs {
    pub id: RecordingId,
    pub fake_engines: bool,
}

impl ProcessArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut id = None;
        let mut fake_engines = false;
        for arg in args {
            match arg.as_str() {
                "--fake-engines" => fake_engines = true,
                flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}")),
                _ if id.is_none() => id = Some(parse_id(arg)?),
                _ => return Err("process takes one recording id".into()),
            }
        }
        Ok(Self {
            id: id.ok_or("process needs a recording id")?,
            fake_engines,
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

/// `models choose KIND auto|off|ID`. The choice stays a string here because its type depends on
/// the kind, which the command resolves.
#[derive(Debug, PartialEq)]
pub struct ChooseArgs {
    pub kind: KindArg,
    pub choice: String,
}

impl ChooseArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        match args {
            [kind, choice] => Ok(Self {
                kind: KindArg::parse(kind)?,
                choice: choice.clone(),
            }),
            _ => Err("models choose takes a kind and auto, off, or a model id".into()),
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
        assert_eq!(
            ListArgs::parse(&words("--search weekly sync")).unwrap_err(),
            "list takes only --search TEXT"
        );
        assert_eq!(
            ListArgs::parse(&["--search".into(), "weekly sync".into()])
                .unwrap()
                .search,
            Some("weekly sync".into())
        );
        assert!(ShowArgs::parse(&words("rec_1")).is_ok());
        assert!(ShowArgs::parse(&words("../etc")).is_err());
        assert!(ShowArgs::parse(&[]).is_err());
        let p = ProcessArgs::parse(&words("--fake-engines rec_1")).unwrap();
        assert!(p.fake_engines);
        assert_eq!(p.id.as_str(), "rec_1");
        assert!(ProcessArgs::parse(&words("--fake-engines")).is_err());
        assert!(ProcessArgs::parse(&words("a b")).is_err());
        assert!(ImportArgs::parse(&words("/tmp/old")).is_ok());
        assert!(ImportArgs::parse(&[]).is_err());
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
                choice: "sherpa/whisper-turbo".into()
            }
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
}
