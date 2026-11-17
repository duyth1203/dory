//! The audit log of assistant access. Several helper processes append at once, so each writes its
//! own log and readers merge them.

use super::AuditError;
use crate::domain::{AuditEntry, Timestamp};

pub trait AuditSink: Send + Sync {
    /// Written before it returns, so a read is never served without its entry. The entry survives
    /// the process dying; only a crash of the whole system may lose the last ones.
    fn append(&self, entry: &AuditEntry) -> Result<(), AuditError>;
}

pub trait AuditRead: Send + Sync {
    /// Entries at or after `since`, newest first, at most `limit`.
    fn since(&self, since: Timestamp, limit: usize) -> Result<Vec<AuditEntry>, AuditError>;
}
