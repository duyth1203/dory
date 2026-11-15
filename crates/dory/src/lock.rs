//! The data folder's exclusive lock. One process at a time runs actors and recovery on a folder:
//! to a second one, a take being recorded looks exactly like a take a crash left open.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

use crate::OpenError;

const LOCK_FILE: &str = ".lock";

/// Held while the folder is open. The OS releases it when the file closes, including when the
/// process dies, so a crash never leaves the folder locked.
pub(crate) struct DataLock {
    _file: File,
}

impl DataLock {
    pub fn acquire(data_dir: &Path) -> Result<DataLock, OpenError> {
        let path = data_dir.join(LOCK_FILE);
        let failed = |source| OpenError::Lock {
            path: path.clone(),
            source,
        };
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(failed)?;
        match file.try_lock() {
            Ok(()) => Ok(DataLock { _file: file }),
            Err(TryLockError::WouldBlock) => Err(OpenError::InUse(PathBuf::from(data_dir))),
            Err(TryLockError::Error(source)) => Err(failed(source)),
        }
    }
}
