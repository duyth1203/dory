//! The composition root. This is the only crate that names adapters: apps depend on `dory-core`
//! for types and on this crate for [`open`].
//!
//! To add an engine, implement `ModelProvider<K>` in its own module and add one line to
//! [`providers`]. Nothing else changes.

mod clock;
mod config;
#[cfg(feature = "fake-engines")]
mod fake;
mod pending;
mod platform;

use std::path::Path;
use std::sync::Arc;

use dory_core::app::{Options, Ports};
use dory_core::models::{Llm, Stt};
use dory_core::ports::capture::CaptureBackend;
use dory_core::ports::{ModelProvider, StoreError};
use dory_store::{FsTakes, SqliteStore};
use thiserror::Error;

pub use clock::SystemClock;
pub use config::{Config, ConfigError, legacy_data_dir};
pub use dory_core::app::Dory;
pub use dory_store::{ImportReport, Skipped};
#[cfg(feature = "fake-engines")]
pub use fake::{FAKE_LLM, FAKE_STT};

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum OpenError {
    #[error("cannot create {path}")]
    DataDir {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot open the database")]
    Store(#[from] StoreError),
    #[error(transparent)]
    Core(#[from] dory_core::app::OpenError),
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ImportError {
    #[error(transparent)]
    Open(#[from] OpenError),
    #[error(transparent)]
    Legacy(#[from] dory_store::ImportError),
}

/// Speech and summary providers.
struct Providers {
    stt: Vec<Arc<dyn ModelProvider<Stt>>>,
    llm: Vec<Arc<dyn ModelProvider<Llm>>>,
}

/// The real engines. The summary list is empty and speech holds a stand-in until P3 adds Sona and
/// Ollama here, one line each.
fn providers() -> Providers {
    Providers {
        stt: vec![Arc::new(pending::PendingSpeech)],
        llm: Vec::new(),
    }
}

/// The store and takes under one data folder.
struct Storage {
    store: Arc<SqliteStore>,
    takes: Arc<FsTakes>,
}

fn storage(config: &Config) -> Result<Storage, OpenError> {
    std::fs::create_dir_all(&config.data_dir).map_err(|source| OpenError::DataDir {
        path: config.data_dir.clone(),
        source,
    })?;
    Ok(Storage {
        store: Arc::new(SqliteStore::open(config.db_path())?),
        takes: Arc::new(FsTakes::new(config.takes_dir())),
    })
}

fn open_with(
    config: &Config,
    capture: Arc<dyn CaptureBackend>,
    options: Options,
    providers: Providers,
) -> Result<Dory, OpenError> {
    let Storage { store, takes } = storage(config)?;
    let ports = Ports {
        store,
        takes,
        capture,
        clock: Arc::new(SystemClock::new()),
        stt: providers.stt,
        llm: providers.llm,
        diar: Vec::new(),
    };
    Ok(Dory::open(ports, options)?)
}

/// Opens Dory on `config.data_dir`: SQLite at `dory.sqlite`, takes under `takes/`, the platform's
/// capture backend, and the system clock.
pub fn open(config: &Config) -> Result<Dory, OpenError> {
    open_with(config, platform::capture(), Options::default(), providers())
}

/// Like [`open`], with dory-core's fake speech and summary engines registered.
#[cfg(feature = "fake-engines")]
pub fn open_with_fake_engines(config: &Config) -> Result<Dory, OpenError> {
    open_with(
        config,
        platform::capture(),
        Options::default(),
        fake::providers(),
    )
}

/// Like [`open`], with a fake capture backend that plays a tone on every channel in real time
/// until stopped. Engines stay as in [`open`], so a finished recording's job blocks.
#[cfg(feature = "fake-engines")]
pub fn open_with_fake_capture(config: &Config) -> Result<Dory, OpenError> {
    let options = Options::default();
    let capture = Arc::new(fake::ToneCapture::new(options.limits.rate_hz.get()));
    open_with(config, capture, options, providers())
}

/// Imports the old Tauri app's data folder into the store under `config.data_dir`. The old folder
/// is only read, and no actors start.
pub fn import_legacy(config: &Config, old_data_dir: &Path) -> Result<ImportReport, ImportError> {
    let Storage { store, takes } = storage(config)?;
    Ok(dory_store::import_legacy(&store, &takes, old_data_dir)?)
}
