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
