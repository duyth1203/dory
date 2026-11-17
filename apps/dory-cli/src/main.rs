//! Headless driver and test bench for Dory. Commands go through the `dory` composition root, so
//! this crate names no adapter. The macOS capture harness lives with its adapter, as the
//! `dory-capture` example of `dory-macos`.

mod args;
mod assistants;
mod delete;
mod library;
mod play;
mod process;
mod record;
mod settings;
mod view;
mod wait;

use std::process::ExitCode;

use dory::{Config, DATA_DIR_VAR, Dory, LibraryReader};
use dory_core::app::Recovery;

const USAGE: &str = "usage:
  dory-cli import OLD_DATA_DIR
  dory-cli add AUDIO_FILE [--title TEXT]
  dory-cli list [--search TEXT]
  dory-cli show ID
  dory-cli delete ID
  dory-cli record [--layout mic-system|mic|system] [--mic DEVICE_UID] [--fake-capture SECONDS]
  dory-cli process ID [--summary] [--pace fast|light] [--fake-engines]
  dory-cli play ID [--seek SECONDS]
  dory-cli models [--about] [--fake-engines]
  dory-cli models choose stt|llm|diar auto|off|MODEL_ID
  dory-cli models provision stt|llm|diar MODEL_ID
  dory-cli languages [TAG...|--clear]
  dory-cli assistants on|off|list
  dory-cli assistants add NAME --exposure titles|summaries|transcripts [--edits never|ask]
  dory-cli assistants exposure NAME titles|summaries|transcripts
  dory-cli assistants edits NAME never|ask
  dory-cli assistants remove NAME
  dory-cli private ID on|off

The data folder is $DORY_DATA_DIR, else ~/Library/Application Support/com.dory.app/v2.
The --fake-* flags write fake audio or fake transcripts, so they need $DORY_DATA_DIR set.
$DORY_MACHINE=battery,low-power,thermal=serious pretends the Mac's conditions, also only with
$DORY_DATA_DIR set.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or_default();
    let result =
        match args.first().map(String::as_str) {
            Some("import") => args::ImportArgs::parse(rest)
                .and_then(|a| config().and_then(|c| library::import(&c, &a))),
            Some("list") => args::ListArgs::parse(rest)
                .and_then(|a| config().and_then(|c| library::list(&c, &a))),
            Some("show") => args::ShowArgs::parse(rest)
                .and_then(|a| config().and_then(|c| library::show(&c, &a))),
            Some("delete") => args::DeleteArgs::parse(rest)
                .and_then(|a| config().and_then(|c| delete::run(&c, &a))),
            Some("models") => match rest.first().map(String::as_str) {
                Some("choose") => args::ChooseArgs::parse(&rest[1..])
                    .and_then(|a| config().and_then(|c| settings::choose(&c, &a))),
                Some("provision") => args::ProvisionArgs::parse(&rest[1..])
                    .and_then(|a| config().and_then(|c| settings::provision(&c, &a))),
                _ => args::ModelsArgs::parse(rest)
                    .and_then(|a| config().and_then(|c| library::models(&c, &a))),
            },
            Some("add") => args::AddArgs::parse(rest)
                .and_then(|a| config().and_then(|c| settings::add(&c, &a))),
            Some("languages") => args::LanguagesArgs::parse(rest)
                .and_then(|a| config().and_then(|c| settings::languages(&c, &a))),
            Some("record") => args::RecordArgs::parse(rest)
                .and_then(|a| config().and_then(|c| record::run(&c, &a))),
            Some("play") => {
                args::PlayArgs::parse(rest).and_then(|a| config().and_then(|c| play::run(&c, &a)))
            }
            Some("process") => args::ProcessArgs::parse(rest)
                .and_then(|a| config().and_then(|c| process::run(&c, &a))),
            Some("assistants") => assistants::AssistantsArgs::parse(rest)
                .and_then(|a| config().and_then(|c| assistants::run(&c, &a))),
            Some("private") => assistants::PrivateArgs::parse(rest)
                .and_then(|a| config().and_then(|c| assistants::private(&c, &a))),
            _ => Err(USAGE.to_string()),
        };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn config() -> Result<Config, String> {
    Config::from_env().map_err(|e| e.to_string())
}

/// Fake audio and fake transcripts never go into the default library: a `--fake-*` flag needs the
/// data folder named explicitly.
fn allow_fakes(flag: &str) -> Result<(), String> {
    match std::env::var_os(DATA_DIR_VAR) {
        Some(dir) if !dir.is_empty() => Ok(()),
        _ => Err(format!(
            "{flag} writes fake data, so it needs {DATA_DIR_VAR} set to a throwaway folder"
        )),
    }
}

/// Opens with the actors running and the data folder locked.
fn open(config: &Config, fake_engines: bool) -> Result<Dory, String> {
    let dory = match fake_engines {
        true => {
            allow_fakes("--fake-engines")?;
            dory::open_with_fake_engines(config)
        }
        false => dory::open(config),
    }
    .map_err(chain)?;
    report_recovery(dory.recovery());
    Ok(dory)
}

/// Opens for reading only, which works while another process records.
fn open_library(config: &Config, fake_engines: bool) -> Result<LibraryReader, String> {
    match fake_engines {
        true => {
            allow_fakes("--fake-engines")?;
            dory::open_library_with_fake_engines(config)
        }
        false => dory::open_library(config),
    }
    .map_err(chain)
}

/// What startup repaired, and what it could not.
fn report_recovery(recovery: &Recovery) {
    for id in &recovery.sealed {
        println!("recovered {id}: sealed the audio a crash left open");
    }
    for id in &recovery.deleted_orphans {
        println!("recovered {id}: deleted audio whose recording was gone");
    }
    for job in &recovery.interrupted {
        println!("recovered job {}: queued again after a crash", job.0);
    }
    for (kind, model) in &recovery.choices_reset {
        println!("the {kind} model {model} is gone, using auto");
    }
    for (id, why) in &recovery.problems {
        eprintln!("could not repair {id}, the next start tries again: {why}");
    }
}

/// An error and its causes on one line.
fn chain(error: impl std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}
