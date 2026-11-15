//! Storage adapters for Dory: `SqliteStore`, `FsTakes`, and the legacy importer.

mod legacy;
mod sha256;
mod sqlite;
mod takes;

pub use legacy::{ImportError, ImportReport, Skipped, import_legacy};
pub use sqlite::SqliteStore;
pub use takes::FsTakes;
