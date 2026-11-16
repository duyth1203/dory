use std::ffi::OsString;
use std::path::{Path, PathBuf};

use dory_core::domain::Processing;
use dory_core::ports::{Conditions, Power, Thermal};
use thiserror::Error;

/// The variable that names the data folder.
pub const DATA_DIR_VAR: &str = "DORY_DATA_DIR";
const APP_ID: &str = "com.dory.app";
/// Where Ollama listens, in Ollama's own format: `host`, `host:port`, or a URL.
pub const OLLAMA_HOST_VAR: &str = "OLLAMA_HOST";
/// Overrides the context window sent to Ollama, for testing map-reduce on a small window.
pub const OLLAMA_NUM_CTX_VAR: &str = "DORY_OLLAMA_NUM_CTX";
const OLLAMA_DEFAULT: &str = "http://127.0.0.1:11434";
/// Pretends the Mac is in other conditions, for lanes: `battery`, `ac`, `low-power`, and
/// `thermal=nominal|fair|serious|critical`, comma-separated. What it leaves out reads as a cool
/// Mac on power. Refused unless `DORY_DATA_DIR` is set, like the `--fake-*` flags.
pub const MACHINE_VAR: &str = "DORY_MACHINE";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// Holds `dory.sqlite`, `takes/`, and `models/`.
    pub data_dir: PathBuf,
    pub ollama_url: String,
    pub ollama_num_ctx: Option<u32>,
    /// Conditions that replace the machine's own, from `DORY_MACHINE`.
    pub machine: Option<Conditions>,
    /// Overrides the Processing setting for this process, for the bench.
    pub processing: Option<Processing>,
}

#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfigError {
    #[error("{DATA_DIR_VAR} is set but empty")]
    EmptyDataDir,
    #[error(
        "there is no default data folder on this platform or without a home folder, set {DATA_DIR_VAR}"
    )]
    NoDefault,
    #[error("{0} is the old app's data folder, which Dory never writes to, choose another")]
    LegacyDir(PathBuf),
    #[error("{OLLAMA_NUM_CTX_VAR} must be a whole number of tokens, not {0:?}")]
    NumCtx(String),
    #[error(
        "{MACHINE_VAR} pretends the Mac's conditions, so it needs {DATA_DIR_VAR} set to a throwaway folder"
    )]
    MachineNeedsDataDir,
    #[error(
        "{MACHINE_VAR} takes battery, ac, low-power, and thermal=nominal|fair|serious|critical, not {0:?}"
    )]
    Machine(String),
}

/// The old app's data folder. The importer reads it. Nothing writes to it.
pub fn legacy_data_dir(home: &Path) -> PathBuf {
    home.join("Library/Application Support").join(APP_ID)
}

impl Config {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            ollama_url: OLLAMA_DEFAULT.to_owned(),
            ollama_num_ctx: None,
            machine: None,
            processing: None,
        }
    }

    /// `DORY_DATA_DIR` when set, else `~/Library/Application Support/com.dory.app/v2` on macOS.
    /// `OLLAMA_HOST` and `DORY_OLLAMA_NUM_CTX` set the summary engine, and `DORY_MACHINE`
    /// pretends the machine's conditions.
    pub fn from_env() -> Result<Self, ConfigError> {
        let data_dir = std::env::var_os(DATA_DIR_VAR);
        let machine = std::env::var(MACHINE_VAR)
            .ok()
            .map(|value| machine_override(&value, data_dir.as_ref().is_some_and(|d| !d.is_empty())))
            .transpose()?;
        let mut config = Self::from_vars(data_dir, std::env::var_os("HOME"))?;
        config.machine = machine;
        if let Ok(host) = std::env::var(OLLAMA_HOST_VAR) {
            config.ollama_url = ollama_url(&host);
        }
        if let Ok(n) = std::env::var(OLLAMA_NUM_CTX_VAR) {
            config.ollama_num_ctx = Some(n.trim().parse().map_err(|_| ConfigError::NumCtx(n))?);
        }
        Ok(config)
    }

    /// [`Config::from_env`] with the two variables passed in.
    pub fn from_vars(
        data_dir: Option<OsString>,
        home: Option<OsString>,
    ) -> Result<Self, ConfigError> {
        let home = home.filter(|h| !h.is_empty()).map(PathBuf::from);
        let legacy = home.as_deref().map(legacy_data_dir);
        let data_dir = match data_dir {
            Some(dir) if dir.is_empty() => return Err(ConfigError::EmptyDataDir),
            Some(dir) => PathBuf::from(dir),
            None => match (&legacy, cfg!(target_os = "macos")) {
                (Some(legacy), true) => legacy.join("v2"),
                _ => return Err(ConfigError::NoDefault),
            },
        };
        if let Some(legacy) = legacy
            && same_folder(&data_dir, &legacy)
        {
            return Err(ConfigError::LegacyDir(legacy));
        }
        Ok(Self::new(data_dir))
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("dory.sqlite")
    }

    pub fn takes_dir(&self) -> PathBuf {
        self.data_dir.join("takes")
    }

    pub fn models_dir(&self) -> PathBuf {
        self.data_dir.join("models")
    }
}

/// Ollama's rules for `OLLAMA_HOST`: an empty value or a bare `0.0.0.0` means the local default,
/// a missing scheme means http, and a missing port means 11434.
fn ollama_url(host: &str) -> String {
    let host = host.trim().trim_end_matches('/');
    if host.is_empty() {
        return OLLAMA_DEFAULT.to_owned();
    }
    let (scheme, rest) = host.split_once("://").unwrap_or(("http", host));
    let rest = if rest.starts_with("0.0.0.0") {
        rest.replacen("0.0.0.0", "127.0.0.1", 1)
    } else {
        rest.to_owned()
    };
    let has_port = rest
        .rsplit_once(':')
        .is_some_and(|(_, port)| port.chars().all(|c| c.is_ascii_digit()));
    if has_port {
        format!("{scheme}://{rest}")
    } else {
        format!("{scheme}://{rest}:11434")
    }
}

/// Parses `DORY_MACHINE`. Pretend conditions never reach the default library.
pub fn machine_override(value: &str, data_dir_set: bool) -> Result<Conditions, ConfigError> {
    if !data_dir_set {
        return Err(ConfigError::MachineNeedsDataDir);
    }
    let mut conditions = Conditions {
        power: Power::Ac,
        low_power: false,
        thermal: Thermal::Nominal,
    };
    for word in value.split(',').map(str::trim).filter(|w| !w.is_empty()) {
        match word {
            "battery" => conditions.power = Power::Battery,
            "ac" => conditions.power = Power::Ac,
            "low-power" => conditions.low_power = true,
            "thermal=nominal" => conditions.thermal = Thermal::Nominal,
            "thermal=fair" => conditions.thermal = Thermal::Fair,
            "thermal=serious" => conditions.thermal = Thermal::Serious,
            "thermal=critical" => conditions.thermal = Thermal::Critical,
            _ => return Err(ConfigError::Machine(value.to_owned())),
        }
    }
    Ok(conditions)
}

fn same_folder(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dory_machine_names_the_conditions_and_needs_a_data_folder() {
        let cool = Conditions {
            power: Power::Ac,
            low_power: false,
            thermal: Thermal::Nominal,
        };
        for (value, want) in [
            (
                "battery",
                Conditions {
                    power: Power::Battery,
                    ..cool
                },
            ),
            (
                "battery,low-power,thermal=serious",
                Conditions {
                    power: Power::Battery,
                    low_power: true,
                    thermal: Thermal::Serious,
                },
            ),
            (
                " thermal=critical ",
                Conditions {
                    thermal: Thermal::Critical,
                    ..cool
                },
            ),
            ("ac", cool),
        ] {
            assert_eq!(machine_override(value, true), Ok(want), "{value:?}");
        }
        assert_eq!(
            machine_override("battery", false),
            Err(ConfigError::MachineNeedsDataDir)
        );
        assert_eq!(
            machine_override("thermal=hot", true),
            Err(ConfigError::Machine("thermal=hot".into()))
        );
    }

    #[test]
    fn ollama_host_follows_ollama_rules() {
        for (host, want) in [
            ("", "http://127.0.0.1:11434"),
            ("0.0.0.0", "http://127.0.0.1:11434"),
            ("localhost", "http://localhost:11434"),
            ("10.0.0.5:8080", "http://10.0.0.5:8080"),
            (
                "https://ollama.example.com/",
                "https://ollama.example.com:11434",
            ),
        ] {
            assert_eq!(ollama_url(host), want, "{host:?}");
        }
    }
}
