//! Which assistants may read the library, and how much of it.
//!
//! An assistant connects through the MCP helper with its own key. Access is off until the user
//! turns it on, and each assistant sees only its exposure. A recording hidden from assistants is
//! invisible to all of them, at every level.

use std::fmt;

use super::{RecordingId, Timestamp};

/// How much of the library one assistant may read. Each level includes the one below.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Exposure {
    /// Id, title, date, duration, and language.
    Titles,
    /// Adds summaries, speaker names, and action items.
    Summaries,
    /// Adds transcripts.
    Transcripts,
}

impl Exposure {
    pub const ALL: [Exposure; 3] = [Exposure::Titles, Exposure::Summaries, Exposure::Transcripts];

    /// The text a search at this level may match inside.
    pub fn fields(self) -> Fields {
        Fields {
            title: true,
            summary: self >= Exposure::Summaries,
            transcript: self >= Exposure::Transcripts,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Exposure::Titles => "titles",
            Exposure::Summaries => "summaries",
            Exposure::Transcripts => "transcripts",
        }
    }

    pub fn parse(s: &str) -> Option<Exposure> {
        Exposure::ALL.into_iter().find(|e| e.as_str() == s)
    }
}

/// The searchable text of a recording, as a mask.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Fields {
    pub title: bool,
    pub summary: bool,
    pub transcript: bool,
}

/// Whether an assistant may ask to change the library. Every change still needs the user's click
/// in Dory.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Writes {
    #[default]
    Never,
    Ask,
}

impl Writes {
    pub fn as_str(self) -> &'static str {
        match self {
            Writes::Never => "never",
            Writes::Ask => "ask",
        }
    }

    pub fn parse(s: &str) -> Option<Writes> {
        [Writes::Never, Writes::Ask]
            .into_iter()
            .find(|w| w.as_str() == s)
    }
}

/// The secret an assistant presents, `dk_` and 64 lowercase hex digits. Its `Debug` never prints
/// it. The store keeps only a digest.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ClientKey(String);

impl ClientKey {
    const PREFIX: &'static str = "dk_";

    pub fn from_bytes(bytes: [u8; 32]) -> ClientKey {
        let mut s = String::with_capacity(Self::PREFIX.len() + 64);
        s.push_str(Self::PREFIX);
        for b in bytes {
            s.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
            s.push(char::from_digit(u32::from(b & 0xf), 16).unwrap_or('0'));
        }
        ClientKey(s)
    }

    pub fn parse(s: &str) -> Option<ClientKey> {
        let hex = s.trim().strip_prefix(Self::PREFIX)?;
        let valid = hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        valid.then(|| ClientKey(s.trim().to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ClientKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ClientKey(redacted)")
    }
}

/// One assistant the user added. The label is unique and is how the user names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssistantClient {
    pub label: String,
    pub exposure: Exposure,
    pub writes: Writes,
    pub added_at: Timestamp,
}

/// Whether assistants may read at all, and who may.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AssistantAccess {
    pub enabled: bool,
    pub clients: Vec<AssistantClient>,
}

/// A persisted change to assistant access, applied through `Change::Assistant`.
#[derive(Clone, Debug, PartialEq)]
pub enum AssistantChange {
    Enabled(bool),
    /// Refused when the label or the key is already used.
    ClientAdded {
        client: AssistantClient,
        key: ClientKey,
    },
    /// Replaces the exposure and writes of the client with this label.
    ClientChanged(AssistantClient),
    ClientRemoved {
        label: String,
    },
    RecordingHidden {
        id: RecordingId,
        hidden: bool,
    },
}

/// A search on behalf of an assistant. It matches only inside `fields` and never returns a
/// recording hidden from assistants, so `limit` counts only what the assistant may see.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssistantQuery {
    pub text: Option<String>,
    pub fields: Fields,
    /// Recordings created at or after this time.
    pub since: Option<Timestamp>,
    /// Recordings created before this time.
    pub until: Option<Timestamp>,
    /// Pages newest first: only recordings that sort after this one.
    pub after: Option<(Timestamp, RecordingId)>,
    pub limit: u32,
}

/// Why an assistant's request was refused. Each says what to do, in words an assistant can pass
/// on to the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    ServerOff,
    NoKey,
    UnknownKey,
    /// Missing, or hidden from assistants: the two answer the same.
    NotFound,
    NotShared {
        need: Exposure,
    },
    WritesOff,
    StaleRevision,
    AppNotRunning,
    Library(String),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::ServerOff => f.write_str(
                "Assistant access to Dory is off. The user can turn it on with `dory-cli assistants on`.",
            ),
            Refusal::NoKey => f.write_str(
                "No Dory key was given. Add this assistant with `dory-cli assistants add` and set DORY_MCP_KEY.",
            ),
            Refusal::UnknownKey => f.write_str(
                "Dory does not know this assistant's key. The user can add it with `dory-cli assistants add`.",
            ),
            Refusal::NotFound => f.write_str("No such recording."),
            Refusal::NotShared { need } => write!(
                f,
                "This assistant may not read that. It needs the {} level, which the user can grant with `dory-cli assistants exposure`.",
                need.as_str()
            ),
            Refusal::WritesOff => f.write_str(
                "This assistant may not change Dory. The user can allow it with `dory-cli assistants edits`.",
            ),
            Refusal::StaleRevision => {
                f.write_str("The transcript changed since it was read. Read it again and retry.")
            }
            Refusal::AppNotRunning => f.write_str(
                "Dory isn't open. Open Dory to let the assistant make changes. Nothing was changed.",
            ),
            Refusal::Library(why) => write!(f, "Dory could not read its library: {why}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_level_includes_the_one_below() {
        let f = |e: Exposure| e.fields();
        assert_eq!(
            f(Exposure::Titles),
            Fields {
                title: true,
                summary: false,
                transcript: false
            }
        );
        assert_eq!(
            f(Exposure::Summaries),
            Fields {
                title: true,
                summary: true,
                transcript: false
            }
        );
        assert_eq!(
            f(Exposure::Transcripts),
            Fields {
                title: true,
                summary: true,
                transcript: true
            }
        );
        for e in Exposure::ALL {
            assert_eq!(Exposure::parse(e.as_str()), Some(e));
        }
    }

    #[test]
    fn a_key_round_trips_and_never_prints() {
        let key = ClientKey::from_bytes([0xab; 32]);
        assert_eq!(key.as_str().len(), 67);
        assert_eq!(ClientKey::parse(key.as_str()), Some(key.clone()));
        assert_eq!(format!("{key:?}"), "ClientKey(redacted)");
        for bad in [
            "",
            "dk_",
            "dk_ABAB",
            "xx_".to_owned().as_str(),
            &key.as_str()[..66],
        ] {
            assert_eq!(ClientKey::parse(bad), None, "{bad}");
        }
        let upper = key.as_str().to_uppercase().replacen("DK_", "dk_", 1);
        assert_eq!(ClientKey::parse(&upper), None);
    }
}
