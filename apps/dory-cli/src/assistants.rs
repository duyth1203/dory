//! `assistants` and `private`: who may read the library through `dory-mcp`, and which recordings
//! no assistant ever sees. Every change takes the data folder's lock; `list` only reads.

use dory::Config;
use dory_core::domain::{Exposure, RecordingId, Writes};

use crate::{chain, open, open_library};

/// What leaves the Mac once an assistant reads. Printed before access is turned on.
pub const WHAT_LEAVES: &str = "Anything an assistant reads from Dory is sent to that assistant's \
provider, like anything else you paste into it. Dory itself sends nothing.";

pub enum AssistantsArgs {
    On,
    Off,
    List,
    Add {
        label: String,
        exposure: Exposure,
        writes: Writes,
    },
    Exposure {
        label: String,
        exposure: Exposure,
    },
    Edits {
        label: String,
        writes: Writes,
    },
    Remove {
        label: String,
    },
}

const USAGE: &str = "assistants takes one of: on, off, list, \
add NAME --exposure titles|summaries|transcripts [--edits never|ask], \
exposure NAME titles|summaries|transcripts, edits NAME never|ask, remove NAME";

fn exposure(s: &str) -> Result<Exposure, String> {
    Exposure::parse(s)
        .ok_or_else(|| format!("unknown exposure {s}: use titles, summaries, or transcripts"))
}

fn writes(s: &str) -> Result<Writes, String> {
    Writes::parse(s).ok_or_else(|| format!("unknown edits {s}: use never or ask"))
}

impl AssistantsArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        match args.as_slice() {
            ["on"] => Ok(Self::On),
            ["off"] => Ok(Self::Off),
            ["list"] => Ok(Self::List),
            ["add", label, rest @ ..] if !label.starts_with("--") => {
                let mut exposure_arg = None;
                let mut writes_arg = Writes::Never;
                let mut it = rest.iter();
                while let Some(flag) = it.next() {
                    let value = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
                    match *flag {
                        "--exposure" => exposure_arg = Some(exposure(value)?),
                        "--edits" => writes_arg = writes(value)?,
                        other => return Err(format!("unknown flag {other}")),
                    }
                }
                Ok(Self::Add {
                    label: (*label).to_owned(),
                    exposure: exposure_arg.ok_or(
                        "add needs --exposure titles, summaries, or transcripts: there is no default",
                    )?,
                    writes: writes_arg,
                })
            }
            ["exposure", label, level] => Ok(Self::Exposure {
                label: (*label).to_owned(),
                exposure: exposure(level)?,
            }),
            ["edits", label, mode] => Ok(Self::Edits {
                label: (*label).to_owned(),
                writes: writes(mode)?,
            }),
            ["remove", label] => Ok(Self::Remove {
                label: (*label).to_owned(),
            }),
            _ => Err(USAGE.into()),
        }
    }
}

pub fn run(config: &Config, args: &AssistantsArgs) -> Result<(), String> {
    if let AssistantsArgs::List = args {
        return list(config);
    }
    let dory = open(config, false)?;
    let assistants = dory.assistants();
    match args {
        AssistantsArgs::List => unreachable!("listed above"),
        AssistantsArgs::On => {
            println!("{WHAT_LEAVES}");
            assistants.set_enabled(true).map_err(chain)?;
            println!("assistant access is on");
        }
        AssistantsArgs::Off => {
            assistants.set_enabled(false).map_err(chain)?;
            println!("assistant access is off");
        }
        AssistantsArgs::Add {
            label,
            exposure,
            writes,
        } => {
            let key = dory::new_client_key().map_err(chain)?;
            let printed = key.as_str().to_owned();
            let client = assistants
                .add(label, *exposure, *writes, key)
                .map_err(chain)?;
            println!(
                "added {} with {} access, edits {}",
                client.label,
                client.exposure.as_str(),
                client.writes.as_str()
            );
            println!("key (shown once, Dory keeps only a digest): {printed}");
        }
        AssistantsArgs::Exposure { label, exposure } => {
            assistants.set_exposure(label, *exposure).map_err(chain)?;
            println!("{label} now has {} access", exposure.as_str());
        }
        AssistantsArgs::Edits { label, writes } => {
            assistants.set_writes(label, *writes).map_err(chain)?;
            println!("{label} edits: {}", writes.as_str());
        }
        AssistantsArgs::Remove { label } => {
            assistants.remove(label).map_err(chain)?;
            println!("removed {label}; its key stops working on its next request");
        }
    }
    Ok(())
}

fn list(config: &Config) -> Result<(), String> {
    let access = open_library(config, false)?
        .assistant_access()
        .map_err(chain)?;
    println!(
        "assistant access is {}",
        if access.enabled { "on" } else { "off" }
    );
    if access.clients.is_empty() {
        println!("no assistants added");
    }
    for c in &access.clients {
        println!(
            "  {}  {}  edits {}",
            c.label,
            c.exposure.as_str(),
            c.writes.as_str()
        );
    }
    Ok(())
}

pub struct PrivateArgs {
    pub id: RecordingId,
    pub hidden: bool,
}

impl PrivateArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        match args {
            [id, state] => Ok(Self {
                id: RecordingId::parse(id).map_err(|e| e.to_string())?,
                hidden: match state.as_str() {
                    "on" => true,
                    "off" => false,
                    other => return Err(format!("private takes on or off, not {other}")),
                },
            }),
            _ => Err("private takes a recording id and on or off".into()),
        }
    }
}

pub fn private(config: &Config, args: &PrivateArgs) -> Result<(), String> {
    let dory = open(config, false)?;
    dory.library()
        .set_hidden(&args.id, args.hidden)
        .map_err(chain)?;
    println!(
        "{} is {}",
        args.id,
        if args.hidden {
            "hidden from assistants"
        } else {
            "visible to assistants"
        }
    );
    Ok(())
}
