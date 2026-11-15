use std::ffi::OsString;
use std::path::{Path, PathBuf};

use thiserror::Error;

const DATA_DIR_VAR: &str = "DORY_DATA_DIR";
const APP_ID: &str = "com.dory.app";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// Holds `dory.sqlite` and `takes/`.
    pub data_dir: PathBuf,
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
}

/// The old Tauri app's data folder. The importer reads it. Nothing writes to it.
pub fn legacy_data_dir(home: &Path) -> PathBuf {
    home.join("Library/Application Support").join(APP_ID)
}

impl Config {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
        }
    }

    /// `DORY_DATA_DIR` when set, else `~/Library/Application Support/com.dory.app/v2` on macOS.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_vars(std::env::var_os(DATA_DIR_VAR), std::env::var_os("HOME"))
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
        Ok(Self { data_dir })
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("dory.sqlite")
    }

    pub fn takes_dir(&self) -> PathBuf {
        self.data_dir.join("takes")
    }
}

fn same_folder(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}
