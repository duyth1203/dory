//! The composition root. This is the only crate that names adapters: apps depend on `dory-core`
//! for types and on this crate for [`open`].
//!
//! To add an engine, implement `ModelProvider<K>` in its own module and add one line to
//! `providers`. Nothing else changes.

mod clock;
mod config;
#[cfg(feature = "fake-engines")]
mod fake;
mod lock;
#[cfg(feature = "app")]
mod machine;
#[cfg(feature = "app")]
mod platform;

use std::path::Path;
use std::sync::Arc;

#[cfg(feature = "app")]
use dory_core::app::{Options, Ports, ReadPorts};
use dory_core::assistant::Assistant;
use dory_core::domain::{Actor, ClientKey};
use dory_core::models::CatalogError;
#[cfg(feature = "app")]
use dory_core::models::{Llm, Stt};
#[cfg(feature = "app")]
use dory_core::ports::ModelProvider;
#[cfg(feature = "app")]
use dory_core::ports::capture::CaptureBackend;
use dory_core::ports::{Clock, StoreError};
#[cfg(feature = "app")]
use dory_engines::{Ollama, Sherpa};
use dory_store::{FsTakes, JsonlAudit, SqliteReader, SqliteStore};
use thiserror::Error;

pub use clock::SystemClock;
pub use config::{
    Config, ConfigError, DATA_DIR_VAR, MACHINE_VAR, OLLAMA_HOST_VAR, OLLAMA_NUM_CTX_VAR,
    legacy_data_dir, machine_override,
};
pub use dory_core::app::{Dory, LibraryReader};
pub use dory_store::{ImportReport, Skipped};
#[cfg(feature = "fake-engines")]
pub use fake::{FAKE_LLM, FAKE_STT};

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum OpenError {
    #[error("cannot create {}", .path.display())]
    DataDir {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "Dory is already running on {}, quit it or set {DATA_DIR_VAR} to another folder",
        .0.display()
    )]
    InUse(std::path::PathBuf),
    #[error("cannot lock {}", .path.display())]
    Lock {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot open the database")]
    Store(#[from] StoreError),
    #[error(transparent)]
    Catalog(#[from] CatalogError),
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

#[cfg(feature = "app")]
/// Speech and summary providers.
struct Providers {
    stt: Vec<Arc<dyn ModelProvider<Stt>>>,
    llm: Vec<Arc<dyn ModelProvider<Llm>>>,
}

#[cfg(feature = "app")]
/// The real engines. To add one, push its provider here.
fn providers(config: &Config) -> Providers {
    let sherpa = Sherpa::new(config.models_dir().join("sherpa"), Sherpa::RELEASES);
    let ollama = Ollama::new(&config.ollama_url);
    let ollama = match config.ollama_num_ctx {
        Some(n) => ollama.with_num_ctx(n),
        None => ollama,
    };
    Providers {
        stt: vec![Arc::new(sherpa)],
        llm: vec![Arc::new(ollama)],
    }
}

/// The store and takes under one data folder.
struct Storage {
    store: Arc<SqliteStore>,
    takes: Arc<FsTakes>,
}

fn storage(config: &Config) -> Result<Storage, OpenError> {
    create_data_dir(config)?;
    Ok(Storage {
        store: Arc::new(SqliteStore::open(config.db_path())?),
        takes: Arc::new(FsTakes::new(config.takes_dir())),
    })
}

fn create_data_dir(config: &Config) -> Result<(), OpenError> {
    std::fs::create_dir_all(&config.data_dir).map_err(|source| OpenError::DataDir {
        path: config.data_dir.clone(),
        source,
    })
}

#[cfg(feature = "app")]
fn open_with(
    config: &Config,
    capture: Arc<dyn CaptureBackend>,
    options: Options,
    providers: Providers,
) -> Result<Dory, OpenError> {
    let options = Options {
        processing: config.processing,
        ..options
    };
    create_data_dir(config)?;
    // Before anything reads the folder: recovery must never see another process's live take.
    let lock = lock::DataLock::acquire(&config.data_dir)?;
    keep_audit(config);
    let Storage { store, takes } = storage(config)?;
    let ports = Ports {
        store,
        takes,
        trash: platform::trash(),
        capture,
        playback: platform::playback(),
        clock: Arc::new(SystemClock::new()),
        machine: machine::for_config(config),
        stt: providers.stt,
        llm: providers.llm,
        diar: Vec::new(),
        guard: Some(Box::new(lock)),
    };
    Ok(Dory::open(ports, options)?)
}

#[cfg(feature = "app")]
fn open_library_with(config: &Config, providers: Providers) -> Result<LibraryReader, OpenError> {
    let store = Arc::new(SqliteReader::open(config.db_path())?);
    Ok(LibraryReader::open(ReadPorts {
        store,
        stt: providers.stt,
        llm: providers.llm,
        diar: Vec::new(),
    })?)
}

#[cfg(feature = "app")]
/// Opens Dory on `config.data_dir`: SQLite at `dory.sqlite`, takes under `takes/`, the platform's
/// capture backend, and the system clock. Holds an exclusive lock on `.lock` in that folder until
/// the returned `Dory` is dropped, so a second open of the same folder, in this process or
/// another, fails with [`OpenError::InUse`].
pub fn open(config: &Config) -> Result<Dory, OpenError> {
    open_with(
        config,
        platform::capture(),
        Options::default(),
        providers(config),
    )
}

/// Creates the audit folder and deletes days past the kept window. Only the lock holder runs it.
/// A failure here never stops Dory opening, so it is reported and skipped.
fn keep_audit(config: &Config) -> usize {
    let root = config.audit_dir();
    let pruned = JsonlAudit::create_root(&root)
        .and_then(|()| JsonlAudit::prune(&root, SystemClock::new().now()));
    pruned.unwrap_or_else(|e| {
        eprintln!("audit: cannot keep {}: {e}", root.display());
        0
    })
}

/// Deletes audit days past the kept window, holding the data folder's lock while it does.
/// Returns how many days went. Fails with [`OpenError::InUse`] while Dory runs, which prunes
/// on its own at every start.
pub fn prune_audit(config: &Config) -> Result<usize, OpenError> {
    create_data_dir(config)?;
    let _lock = lock::DataLock::acquire(&config.data_dir)?;
    Ok(keep_audit(config))
}

/// The audit log of assistant access, for reading. Works while Dory runs.
pub fn audit_reader(config: &Config) -> Arc<dyn dory_core::ports::AuditRead> {
    Arc::new(JsonlAudit::reader(config.audit_dir()))
}

/// The read path for one assistant, as the `dory-mcp` helper runs it: a read-only view of the
/// library, the audit log this process appends to, and the key the assistant presented. It takes
/// no lock, so it works while Dory runs, and it never writes the library.
pub fn open_assistant(config: &Config, key: Option<ClientKey>) -> Result<Assistant, OpenError> {
    let store = Arc::new(SqliteReader::open(config.db_path())?);
    let clock = Arc::new(SystemClock::new());
    let audit = Arc::new(JsonlAudit::writer(
        config.audit_dir(),
        Actor::Helper,
        clock.now(),
    ));
    Ok(Assistant::new(store, audit, clock, key))
}

/// A new key for an assistant, from the system's random source. Show it once and keep it nowhere:
/// the store keeps only its digest.
pub fn new_client_key() -> std::io::Result<dory_core::domain::ClientKey> {
    use std::io::Read;
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(dory_core::domain::ClientKey::from_bytes(bytes))
}

#[cfg(feature = "app")]
/// Opens the library under `config.data_dir` for reading only. It takes no lock, repairs nothing,
/// and never writes: it creates no file, sets no journal mode, and never migrates, so it works
/// while another process records into the same folder and cannot change that process's schema.
/// A library that does not exist yet, or was written by another build, is refused.
pub fn open_library(config: &Config) -> Result<LibraryReader, OpenError> {
    open_library_with(config, providers(config))
}

/// Like [`open_library`], with dory-core's fake speech and summary engines registered.
#[cfg(feature = "fake-engines")]
pub fn open_library_with_fake_engines(config: &Config) -> Result<LibraryReader, OpenError> {
    open_library_with(config, fake::providers())
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
    open_with(config, capture, options, providers(config))
}

/// Imports the old app's data folder into the store under `config.data_dir`. The old folder
/// is only read, and no actors start. Holds the data folder's lock while it writes, so it fails
/// with [`OpenError::InUse`] while Dory runs there.
pub fn import_legacy(config: &Config, old_data_dir: &Path) -> Result<ImportReport, ImportError> {
    create_data_dir(config)?;
    let _lock = lock::DataLock::acquire(&config.data_dir)?;
    let Storage { store, takes } = storage(config)?;
    Ok(dory_store::import_legacy(&store, &takes, old_data_dir)?)
}

#[cfg(feature = "app")]
/// Removes the audio devices the old app left behind and puts the system default devices
/// back, once. Reads the old app's restore files from its data folder and writes only a marker
/// under `config.data_dir`. A no-op off macOS and when `HOME` is unset. It is also skipped when
/// `DORY_DATA_DIR` is set, because the devices are system wide and a throwaway data folder must
/// never change the machine's audio setup.
pub fn cleanup_legacy_audio(config: &Config) {
    #[cfg(target_os = "macos")]
    {
        if std::env::var_os(DATA_DIR_VAR).is_some() {
            eprintln!("legacy-cleanup: skipped, {DATA_DIR_VAR} is set");
            return;
        }
        let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) else {
            return;
        };
        let paths = dory_macos::legacy_cleanup::Paths {
            legacy_dir: legacy_data_dir(Path::new(&home)),
            marker: config.data_dir.join("legacy-audio-cleanup.done"),
        };
        dory_macos::legacy_cleanup::run_at_launch(&paths);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = config;
}
