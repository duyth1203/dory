//! The owner's side of assistant access: turning it on, adding and removing assistants, and
//! hiding recordings. Only the process that holds the data folder writes these.

use super::{Dory, DoryError};
use crate::domain::{
    AssistantAccess, AssistantChange, AssistantClient, ClientKey, Exposure, InvalidInput,
    RecordingId, Writes,
};
use crate::ports::{Change, StoreError};

pub struct Assistants<'a> {
    pub(super) dory: &'a Dory,
}

impl Assistants<'_> {
    pub fn access(&self) -> Result<AssistantAccess, StoreError> {
        self.dory.db.store.assistant_access()
    }

    pub fn set_enabled(&self, on: bool) -> Result<(), DoryError> {
        self.apply(AssistantChange::Enabled(on))
    }

    /// Adds an assistant that will present `key`. The label is trimmed and must be new.
    pub fn add(
        &self,
        label: &str,
        exposure: Exposure,
        writes: Writes,
        key: ClientKey,
    ) -> Result<AssistantClient, DoryError> {
        let label = label.trim();
        if label.is_empty() {
            return Err(InvalidInput::new("assistant name", "is empty").into());
        }
        if self.client(label)?.is_some() {
            return Err(DoryError::AssistantExists(label.to_owned()));
        }
        let client = AssistantClient {
            label: label.to_owned(),
            exposure,
            writes,
            added_at: self.dory.clock.now(),
        };
        self.apply(AssistantChange::ClientAdded {
            client: client.clone(),
            key,
        })?;
        Ok(client)
    }

    pub fn set_exposure(&self, label: &str, exposure: Exposure) -> Result<(), DoryError> {
        self.change(label, |c| c.exposure = exposure)
    }

    pub fn set_writes(&self, label: &str, writes: Writes) -> Result<(), DoryError> {
        self.change(label, |c| c.writes = writes)
    }

    pub fn remove(&self, label: &str) -> Result<(), DoryError> {
        self.apply(AssistantChange::ClientRemoved {
            label: label.trim().to_owned(),
        })
    }

    fn client(&self, label: &str) -> Result<Option<AssistantClient>, StoreError> {
        Ok(self
            .access()?
            .clients
            .into_iter()
            .find(|c| c.label == label.trim()))
    }

    fn change(
        &self,
        label: &str,
        edit: impl FnOnce(&mut AssistantClient),
    ) -> Result<(), DoryError> {
        let mut client = self
            .client(label)?
            .ok_or_else(|| StoreError::NotFound(format!("assistant {}", label.trim())))?;
        edit(&mut client);
        self.apply(AssistantChange::ClientChanged(client))
    }

    fn apply(&self, change: AssistantChange) -> Result<(), DoryError> {
        Ok(self.dory.db.apply(vec![Change::Assistant(change)])?)
    }
}

/// Hides a recording from every assistant, or shows it again.
pub(super) fn set_hidden(dory: &Dory, id: &RecordingId, hidden: bool) -> Result<(), DoryError> {
    Ok(dory
        .db
        .apply(vec![Change::Assistant(AssistantChange::RecordingHidden {
            id: id.clone(),
            hidden,
        })])?)
}
