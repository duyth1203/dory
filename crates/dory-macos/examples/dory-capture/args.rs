use std::path::PathBuf;

use dory_core::domain::ChannelLayout;
use dory_core::ports::capture::MicSelection;

fn parse_layout(s: &str) -> Result<ChannelLayout, String> {
    match s {
        "mic-system" => Ok(ChannelLayout::MicSystem),
        "mic" => Ok(ChannelLayout::MicOnly),
        "system" => Ok(ChannelLayout::SystemOnly),
        other => Err(format!("--layout: unknown layout {other}")),
    }
}

#[derive(Debug, PartialEq)]
pub struct CaptureArgs {
    pub seconds: f64,
    pub out: PathBuf,
    pub layout: ChannelLayout,
    pub rate_hz: u32,
    pub mic: MicSelection,
    /// Plays a tone from this process during the capture, to check self-exclusion.
    pub self_tone: bool,
    /// Harness only: rebuilds the device graph this many seconds in, as a device change would.
    pub rebuild_at: Option<f64>,
}

impl CaptureArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut seconds = None;
        let mut out = None;
        let mut layout = ChannelLayout::MicSystem;
        let mut rate_hz = 48_000;
        let mut mic = MicSelection::SystemDefault;
        let mut self_tone = false;
        let mut rebuild_at = None;
        let mut it = args.iter();
        while let Some(flag) = it.next() {
            let mut value = || it.next().ok_or_else(|| format!("{flag} needs a value"));
            match flag.as_str() {
                "--seconds" => {
                    seconds = Some(
                        value()?
                            .parse::<f64>()
                            .map_err(|e| format!("--seconds: {e}"))?,
                    )
                }
                "--out" => out = Some(PathBuf::from(value()?)),
                "--layout" => layout = parse_layout(value()?)?,
                "--rate" => rate_hz = value()?.parse().map_err(|e| format!("--rate: {e}"))?,
                "--mic" => mic = MicSelection::Uid(value()?.clone()),
                "--self-tone" => self_tone = true,
                "--rebuild-at" => {
                    rebuild_at = Some(
                        value()?
                            .parse::<f64>()
                            .map_err(|e| format!("--rebuild-at: {e}"))?,
                    )
                }
                other => return Err(format!("unknown flag {other}")),
            }
        }
        let seconds = seconds
            .filter(|s| *s > 0.0)
            .ok_or("--seconds N (N > 0) is required")?;
        let out = out.ok_or("--out FILE.wav is required")?;
        if rate_hz == 0 {
            return Err("--rate must be positive".into());
        }
        Ok(Self {
            seconds,
            out,
            layout,
            rate_hz,
            mic,
            self_tone,
            rebuild_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<CaptureArgs, String> {
        CaptureArgs::parse(&s.split_whitespace().map(String::from).collect::<Vec<_>>())
    }

    #[test]
    fn defaults_to_mic_system_at_48k() {
        let a = parse("--seconds 3 --out x.wav").unwrap();
        assert_eq!(a.layout, ChannelLayout::MicSystem);
        assert_eq!(a.rate_hz, 48_000);
        assert_eq!(a.mic, MicSelection::SystemDefault);
    }

    #[test]
    fn reads_every_flag() {
        let a = parse("--layout system --rate 16000 --mic BuiltIn --seconds 2.5 --out /tmp/a.wav")
            .unwrap();
        assert_eq!(a.layout, ChannelLayout::SystemOnly);
        assert_eq!(a.rate_hz, 16_000);
        assert_eq!(a.seconds, 2.5);
        assert_eq!(a.mic, MicSelection::Uid("BuiltIn".into()));
        assert!(!a.self_tone);
        assert_eq!(a.rebuild_at, None);
        assert_eq!(
            parse("--seconds 20 --out x.wav --rebuild-at 10")
                .unwrap()
                .rebuild_at,
            Some(10.0)
        );
        assert!(
            parse("--seconds 1 --out x.wav --self-tone")
                .unwrap()
                .self_tone
        );
    }

    #[test]
    fn rejects_missing_and_unknown_input() {
        assert!(parse("--out x.wav").is_err());
        assert!(parse("--seconds 1").is_err());
        assert!(parse("--seconds 1 --out x.wav --layout mixed").is_err());
        assert!(parse("--seconds 1 --out x.wav --rate").is_err());
    }
}
