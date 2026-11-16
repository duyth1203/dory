//! The system trash. A recoverable delete moves a take's files here instead of removing them, and
//! undo moves them back. The core never learns what the platform's trash looks like: it keeps the
//! [`TrashReceipt`] the adapter returned and hands it back.

use std::path::PathBuf;

use thiserror::Error;

pub trait Trash: Send + Sync {
    /// Moves every path to the trash, or none: if one fails, the adapter puts back the ones it
    /// already moved. An empty list moves nothing and returns an empty receipt.
    fn move_to_trash(&self, paths: &[PathBuf]) -> Result<TrashReceipt, TrashError>;

    /// Moves everything in `receipt` back to where it was, or nothing. It never overwrites: a
    /// path that is occupied again fails with [`TrashError::Occupied`].
    fn put_back(&self, receipt: &TrashReceipt) -> Result<(), TrashError>;
}

/// Where each moved file came from and where it went. Opaque to apps: only the adapter that made
/// it reads the paths.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrashReceipt {
    moved: Vec<Moved>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Moved {
    pub original: PathBuf,
    pub trashed: PathBuf,
}

impl TrashReceipt {
    pub fn new(moved: Vec<Moved>) -> Self {
        Self { moved }
    }

    pub fn moved(&self) -> &[Moved] {
        &self.moved
    }

    pub fn is_empty(&self) -> bool {
        self.moved.is_empty()
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TrashError {
    #[error("the trash has no adapter for this platform yet")]
    Unsupported,
    /// The file is no longer in the trash, usually because the user emptied it.
    #[error("{} is no longer in the trash", .0.display())]
    Missing(PathBuf),
    /// Something else is at the place the file would go back to.
    #[error("{} is taken, so the file cannot go back", .0.display())]
    Occupied(PathBuf),
    #[error("the trash failed: {0}")]
    Platform(String),
    #[error("the trash failed on {}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}
