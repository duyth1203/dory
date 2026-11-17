//! `SqliteReader`: reads a library another process owns, and never changes it.
//!
//! It opens one read-only connection and nothing else. It never creates the database or its
//! folder, never sets the journal mode, never migrates, and never writes the database, so a
//! reader from a newer or older build cannot change the schema under the app that owns the
//! folder. Like any WAL reader, SQLite may create the `-wal` and `-shm` sidecar files beside the
//! database, which is how readers and the writer coordinate. It reads only the schema it was
//! built for, and checks that on every read, because the owner may migrate while it is open.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use dory_core::ports::StoreError;
use dory_core::search::Vocabulary;
use rusqlite::{Connection, OpenFlags};

use super::{BUSY_TIMEOUT, MIGRATIONS, db, lock, user_version};

/// Implements `StoreRead` and not `Store`, so it has no write path:
///
/// ```compile_fail
/// use dory_core::ports::Store;
/// fn write(reader: &dory_store::SqliteReader) {
///     reader.apply(Vec::new()).unwrap();
/// }
/// ```
pub struct SqliteReader {
    conn: Mutex<Connection>,
    pub(super) vocabulary: Mutex<Option<Arc<Vocabulary>>>,
    path: PathBuf,
}

impl std::fmt::Debug for SqliteReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteReader")
            .field("path", &self.path)
            .finish()
    }
}

fn supported() -> u32 {
    u32::try_from(MIGRATIONS.len()).unwrap_or(u32::MAX)
}

impl SqliteReader {
    /// Opens the library at `path` for reading. A missing file gives [`StoreError::Missing`], and
    /// a schema other than the one this build reads gives `SchemaTooOld` or `SchemaTooNew`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref();
        if !path.is_file() {
            return Err(StoreError::Missing(path.to_owned()));
        }
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(db)?;
        conn.busy_timeout(BUSY_TIMEOUT).map_err(db)?;
        let found = user_version(&conn)?;
        let supported = supported();
        match found {
            0 => Err(StoreError::Missing(path.to_owned())),
            f if f < supported => Err(StoreError::SchemaTooOld { found, supported }),
            f if f > supported => Err(StoreError::SchemaTooNew { found, supported }),
            _ => {
                super::search::open_vocabulary_view(&conn)?;
                Ok(Self {
                    conn: Mutex::new(conn),
                    vocabulary: Mutex::new(None),
                    path: path.to_owned(),
                })
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn read<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let mut conn = lock(&self.conn);
        let tx = conn.transaction().map_err(db)?;
        let found = user_version(&tx)?;
        let supported = supported();
        if found != supported {
            return Err(StoreError::SchemaChanged { found, supported });
        }
        f(&tx)
    }
}
