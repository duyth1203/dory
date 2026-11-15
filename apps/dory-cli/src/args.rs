use std::path::PathBuf;

use dory_core::domain::{ChannelLayout, RecordingId};
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
pub struct ModelsArgs {
    pub fake_engines: bool,
}

impl ModelsArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        match args {
            [] => Ok(Self {
                fake_engines: false,
            }),
            [flag] if flag == "--fake-engines" => Ok(Self { fake_engines: true }),
            _ => Err("models takes only --fake-engines".into()),
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
    }
}
