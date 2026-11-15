use std::collections::HashSet;

use thiserror::Error;

use super::{Choice, KindTag, ModelEntry, ModelId, ModelKind, ModelRef};

/// Every model of one kind that providers offer, validated once at startup.
#[derive(Clone, Debug)]
pub struct Catalog<K: ModelKind> {
    entries: Vec<ModelEntry<K>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum CatalogError {
    #[error("duplicate model id {0}")]
    Duplicate(ModelRef),
    #[error("no {0} models registered")]
    Empty(KindTag),
}

impl<K: ModelKind> Catalog<K> {
    /// Rejects duplicate ids, and an empty list for kinds the user cannot turn off.
    pub fn new(entries: Vec<ModelEntry<K>>) -> Result<Self, CatalogError> {
        let mut seen = HashSet::new();
        for entry in &entries {
            if !seen.insert(&entry.id) {
                return Err(CatalogError::Duplicate(entry.id.erase()));
            }
        }
        if entries.is_empty() && !K::OPTIONAL {
            return Err(CatalogError::Empty(K::TAG));
        }
        Ok(Self { entries })
    }

    pub fn entries(&self) -> &[ModelEntry<K>] {
        &self.entries
    }

    pub fn get(&self, id: &ModelId<K>) -> Option<&ModelEntry<K>> {
        self.entries.iter().find(|e| &e.id == id)
    }

    /// A setting that names a model since removed falls back to `Auto`. The second value is the
    /// id that was dropped, so the caller can tell the user.
    pub fn reconcile(&self, choice: Choice<K>) -> (Choice<K>, Option<ModelId<K>>) {
        match choice {
            Choice::Pinned(id) if self.get(&id).is_none() => (Choice::Auto, Some(id)),
            other => (other, None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{LangSet, Ms};
    use crate::models::{Diar, DiarizationSupport, LangSupport, Stt, SttCaps};

    fn stt(id: &str) -> ModelEntry<Stt> {
        ModelEntry {
            id: id.parse().unwrap(),
            display_name: id.into(),
            caps: SttCaps {
                languages: LangSupport::Multilingual {
                    auto_detect: true,
                    tags: LangSet::Any,
                },
                diarization: DiarizationSupport::None,
                max_input: Some(Ms(1)),
            },
            recommended: false,
            rank: 0,
            disk_bytes: None,
        }
    }

    #[test]
    fn rejects_duplicates_and_empty_required_kinds() {
        let dup = Catalog::new(vec![stt("a/x"), stt("b/y"), stt("a/x")]).unwrap_err();
        assert_eq!(
            dup,
            CatalogError::Duplicate(ModelRef::parse("a/x").unwrap())
        );
        assert_eq!(
            Catalog::<Stt>::new(vec![]).unwrap_err(),
            CatalogError::Empty(KindTag::Stt)
        );
    }

    #[test]
    fn optional_kinds_may_be_empty() {
        assert!(Catalog::<Diar>::new(vec![]).is_ok());
    }

    #[test]
    fn same_name_under_different_providers_is_not_a_duplicate() {
        assert!(Catalog::new(vec![stt("a/x"), stt("b/x")]).is_ok());
    }

    #[test]
    fn reconcile_replaces_only_removed_pins() {
        let catalog = Catalog::new(vec![stt("a/x")]).unwrap();
        let id = |s: &str| -> ModelId<Stt> { s.parse().unwrap() };
        let cases = [
            (Choice::Pinned(id("a/x")), Choice::Pinned(id("a/x")), None),
            (
                Choice::Pinned(id("a/gone")),
                Choice::Auto,
                Some(id("a/gone")),
            ),
            (Choice::Auto, Choice::Auto, None),
            (Choice::Off, Choice::Off, None),
        ];
        for (input, want, dropped) in cases {
            assert_eq!(catalog.reconcile(input), (want, dropped));
        }
    }
}
