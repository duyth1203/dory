//! The rules that decide what one assistant may read. Pure functions over stored facts.

use crate::domain::{AssistantClient, Exposure, Fields, Refusal, Writes};

/// Proof that an assistant is allowed in. Only [`grant`] builds one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    client: AssistantClient,
}

impl Grant {
    pub fn label(&self) -> &str {
        &self.client.label
    }

    pub fn exposure(&self) -> Exposure {
        self.client.exposure
    }

    pub fn writes(&self) -> Writes {
        self.client.writes
    }

    /// The text this assistant's searches may match inside.
    pub fn fields(&self) -> Fields {
        self.client.exposure.fields()
    }
}

/// Lets an assistant in when access is on, it presented a key, and the key belongs to a client
/// the user added.
pub fn grant(
    enabled: bool,
    key_given: bool,
    client: Option<AssistantClient>,
) -> Result<Grant, Refusal> {
    if !enabled {
        return Err(Refusal::ServerOff);
    }
    if !key_given {
        return Err(Refusal::NoKey);
    }
    client
        .map(|client| Grant { client })
        .ok_or(Refusal::UnknownKey)
}

/// Whether the assistant may see a recording at the `need` level. A hidden recording answers
/// exactly like a missing one, so an assistant cannot learn that it exists.
pub fn reveal(grant: &Grant, hidden: bool, need: Exposure) -> Result<(), Refusal> {
    if hidden {
        return Err(Refusal::NotFound);
    }
    if grant.exposure() < need {
        return Err(Refusal::NotShared { need });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Timestamp;

    fn client(exposure: Exposure) -> AssistantClient {
        AssistantClient {
            label: "Claude".into(),
            exposure,
            writes: Writes::Never,
            added_at: Timestamp(0),
        }
    }

    #[test]
    fn grant_needs_access_on_a_key_and_a_known_client() {
        let c = || Some(client(Exposure::Titles));
        assert_eq!(grant(false, true, c()), Err(Refusal::ServerOff));
        assert_eq!(grant(false, false, None), Err(Refusal::ServerOff));
        assert_eq!(grant(true, false, c()), Err(Refusal::NoKey));
        assert_eq!(grant(true, true, None), Err(Refusal::UnknownKey));
        assert_eq!(grant(true, true, c()).unwrap().label(), "Claude");
    }

    #[test]
    fn reveal_covers_every_level_and_hiding() {
        for have in Exposure::ALL {
            let g = grant(true, true, Some(client(have))).unwrap();
            for need in Exposure::ALL {
                let expected = if have >= need {
                    Ok(())
                } else {
                    Err(Refusal::NotShared { need })
                };
                assert_eq!(
                    reveal(&g, false, need),
                    expected,
                    "{have:?} reading {need:?}"
                );
                assert_eq!(
                    reveal(&g, true, need),
                    Err(Refusal::NotFound),
                    "hidden {need:?}"
                );
            }
        }
    }
}

/// Whether this assistant may ask for changes at all. Each change still needs the user's yes.
pub fn may_write(grant: &Grant) -> Result<(), Refusal> {
    match grant.writes() {
        Writes::Never => Err(Refusal::WritesOff),
        Writes::Ask => Ok(()),
    }
}

/// How recent a read of meeting text has to be to warn about it on the consent card.
pub const TAINT_WINDOW_MS: i64 = 5 * 60 * 1000;

/// Seconds since `label` last read meeting text, if that was within [`TAINT_WINDOW_MS`]. Meeting
/// text can carry instructions from other people, so a change asked for right after reading it
/// deserves a second look.
pub fn taint(
    entries: &[crate::domain::AuditEntry],
    label: &str,
    now: crate::domain::Timestamp,
) -> Option<i64> {
    use crate::domain::{Access, AuditOutcome};
    entries
        .iter()
        .filter(|e| e.client.as_deref() == Some(label))
        .filter(|e| matches!(e.outcome, AuditOutcome::Served { .. }))
        .filter(|e| {
            matches!(
                e.access,
                Access::Search { .. }
                    | Access::Summary(_)
                    | Access::Transcript { .. }
                    | Access::ActionItems { .. }
            )
        })
        .map(|e| now.0 - e.at.0)
        .filter(|age| (0..TAINT_WINDOW_MS).contains(age))
        .min()
        .map(|ms| ms / 1000)
}

#[cfg(test)]
mod write_tests {
    use super::*;
    use crate::domain::{
        Access, Actor, AssistantClient, AuditEntry, AuditOutcome, RecordingId, Timestamp,
    };

    fn grant_with(writes: Writes) -> Grant {
        grant(
            true,
            true,
            Some(AssistantClient {
                label: "Claude".into(),
                exposure: Exposure::Titles,
                writes,
                added_at: Timestamp(0),
            }),
        )
        .unwrap()
    }

    #[test]
    fn writes_need_ask() {
        assert_eq!(
            may_write(&grant_with(Writes::Never)),
            Err(Refusal::WritesOff)
        );
        assert_eq!(may_write(&grant_with(Writes::Ask)), Ok(()));
    }

    #[test]
    fn taint_is_a_recent_served_read_of_meeting_text_by_the_same_client() {
        let id = RecordingId::parse("a").unwrap();
        let entry = |at: i64, client: &str, access: Access, served: bool| AuditEntry {
            at: Timestamp(at),
            actor: Actor::Helper,
            client: Some(client.into()),
            reported_name: None,
            access,
            outcome: if served {
                AuditOutcome::Served { items: 1 }
            } else {
                AuditOutcome::Refused(Refusal::NotFound)
            },
        };
        let now = Timestamp(1_000_000);
        let log = vec![
            entry(
                990_000,
                "Claude",
                Access::Transcript {
                    id: id.clone(),
                    turns: (0, 3),
                },
                true,
            ),
            entry(999_000, "Other", Access::Summary(id.clone()), true),
            entry(999_500, "Claude", Access::Summary(id.clone()), false),
            entry(999_800, "Claude", Access::Recording(id.clone()), true),
        ];
        assert_eq!(taint(&log, "Claude", now), Some(10));
        assert_eq!(
            taint(&log[1..], "Claude", now),
            None,
            "refused and metadata reads do not count"
        );
        let old = vec![entry(
            1_000_000 - TAINT_WINDOW_MS,
            "Claude",
            Access::Summary(id),
            true,
        )];
        assert_eq!(taint(&old, "Claude", now), None);
    }
}
