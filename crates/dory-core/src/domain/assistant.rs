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

    /// The text a search at this level may match inside: exactly the text a read at this level
    /// shows. Speaker names come with summaries, because `get_recording` lists them from there.
    pub fn fields(self) -> Fields {
        Fields {
            title: true,
            speakers: self >= Exposure::Summaries,
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

/// The searchable text of a recording, as a mask. `speakers` is the names the user gave the
/// speakers of the current transcript.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Fields {
    pub title: bool,
    pub speakers: bool,
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
/// recording hidden from assistants, so `limit` counts only what the assistant may see. Without a
/// query it lists every recording the assistant may see.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssistantQuery {
    pub query: Option<crate::search::Query>,
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
    /// The user said no in Dory.
    Declined,
    /// Nobody answered in Dory before the request timed out.
    Unanswered,
    /// Dory's window is closed, so nobody saw the request.
    NoWindow,
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
            Refusal::Declined => f.write_str("The user declined this change in Dory. Nothing was changed."),
            Refusal::Unanswered => f.write_str(
                "Nobody answered in Dory, so nothing was changed. Ask the user to approve it in Dory, then try again.",
            ),
            Refusal::NoWindow => f.write_str(
                "Dory's window is closed, so nobody saw the request. Ask the user to open Dory's window, then try again. Nothing was changed.",
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
                speakers: false,
                summary: false,
                transcript: false
            }
        );
        assert_eq!(
            f(Exposure::Summaries),
            Fields {
                title: true,
                speakers: true,
                summary: true,
                transcript: false
            }
        );
        assert_eq!(
            f(Exposure::Transcripts),
            Fields {
                title: true,
                speakers: true,
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

/// Who wrote an audit entry: a `dory-mcp` helper, or the app handling a change request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Actor {
    Helper,
    App,
}

impl Actor {
    pub fn as_str(self) -> &'static str {
        match self {
            Actor::Helper => "mcp",
            Actor::App => "app",
        }
    }
}

/// What an assistant asked for. It names recordings and counts, never meeting text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Access {
    Search {
        text: String,
        hits: u32,
    },
    Recording(RecordingId),
    Summary(RecordingId),
    Transcript {
        id: RecordingId,
        turns: (u32, u32),
    },
    ActionItems {
        ids: Vec<RecordingId>,
    },
    ResourceList,
    Prompt {
        name: String,
    },
    Write {
        what: String,
        id: Option<RecordingId>,
    },
}

impl Access {
    /// How much of a search's text the log keeps.
    pub const SEARCH_TEXT_CHARS: usize = 200;

    /// What was asked for, as a phrase that follows the assistant's name.
    pub fn describe(&self) -> String {
        match self {
            Access::Search { text, hits } if text.is_empty() => {
                format!("listed meetings ({hits} found)")
            }
            Access::Search { text, hits } => format!("searched \"{text}\" ({hits} found)"),
            Access::Recording(id) => format!("looked at {id}"),
            Access::Summary(id) => format!("read the summary of {id}"),
            Access::Transcript { id, turns } => {
                format!("read turns {}-{} of {id}'s transcript", turns.0, turns.1)
            }
            Access::ActionItems { ids } => {
                format!("read action items of {} meeting(s)", ids.len())
            }
            Access::ResourceList => "listed meetings".to_owned(),
            Access::Prompt { name } => format!("used the {name} prompt"),
            Access::Write { what, id } => match id {
                Some(id) => format!("asked to {what} on {id}"),
                None => format!("asked to {what}"),
            },
        }
    }

    pub fn search(text: &str, hits: u32) -> Access {
        Access::Search {
            text: text.chars().take(Self::SEARCH_TEXT_CHARS).collect(),
            hits,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuditOutcome {
    Served { items: u32 },
    Refused(Refusal),
}

/// One line of the audit log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditEntry {
    pub at: Timestamp,
    pub actor: Actor,
    /// The label of the assistant whose key was presented, when the key was known.
    pub client: Option<String>,
    /// The name the client gave itself. Self-reported, so a label only, never a credential.
    pub reported_name: Option<String>,
    pub access: Access,
    pub outcome: AuditOutcome,
}

/// Days of audit kept. Older day folders are deleted by the process that owns the library.
pub const AUDIT_KEEP_DAYS: i64 = 30;

/// Whether the day starting at `day_start` is still within the kept window as of `now`.
pub fn keep_audit_day(day_start: Timestamp, now: Timestamp) -> bool {
    const DAY_MS: i64 = 86_400_000;
    let today_start = now.0.div_euclid(DAY_MS) * DAY_MS;
    today_start - day_start.0 < AUDIT_KEEP_DAYS * DAY_MS
}

#[cfg(test)]
mod audit_tests {
    use super::*;

    #[test]
    fn search_text_is_cut_on_a_char_boundary() {
        let long = "é".repeat(500);
        let Access::Search { text, hits } = Access::search(&long, 3) else {
            unreachable!()
        };
        assert_eq!(text.chars().count(), Access::SEARCH_TEXT_CHARS);
        assert_eq!(hits, 3);
    }

    #[test]
    fn keep_day_keeps_thirty_days() {
        let day = 86_400_000;
        let now = Timestamp(100 * day + 5);
        assert!(keep_audit_day(Timestamp(100 * day), now), "today");
        assert!(keep_audit_day(Timestamp(71 * day), now), "29 days ago");
        assert!(!keep_audit_day(Timestamp(70 * day), now), "30 days ago");
        assert!(!keep_audit_day(Timestamp(69 * day), now));
    }
}

/// A change an assistant asks for. There is no variant that starts or stops a recording:
/// recording other people is an act the user does with their own hand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteCommand {
    SetTitle {
        id: RecordingId,
        /// `None` clears the title, so Dory shows the date.
        title: Option<String>,
    },
    /// Names the speaker currently labelled `speaker` in the transcript at `revision`.
    RenameSpeaker {
        id: RecordingId,
        speaker: String,
        revision: super::Revision,
        name: Option<String>,
    },
    /// Summarizes the meeting again with what the user wants it to focus on. The summary it
    /// replaces is kept and can be restored.
    Resummarize {
        id: RecordingId,
        instruction: String,
    },
}

impl RemoteCommand {
    pub const MAX_TEXT_CHARS: usize = 200;
    pub const MAX_INSTRUCTION_CHARS: usize = 500;

    pub fn id(&self) -> &RecordingId {
        match self {
            RemoteCommand::SetTitle { id, .. }
            | RemoteCommand::RenameSpeaker { id, .. }
            | RemoteCommand::Resummarize { id, .. } => id,
        }
    }

    /// What the change does, in a few words, for the audit log.
    pub fn what(&self) -> &'static str {
        match self {
            RemoteCommand::SetTitle { .. } => "set the title",
            RemoteCommand::RenameSpeaker { .. } => "rename a speaker",
            RemoteCommand::Resummarize { .. } => "summarize again",
        }
    }
}

/// A change request as it reaches the app: who asks, under which key, for what.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteRequest {
    pub key: Option<ClientKey>,
    pub reported_name: Option<String>,
    pub command: RemoteCommand,
}

/// `Ok` once the change is applied.
pub type RemoteOutcome = Result<(), Refusal>;

/// What the user is asked in Dory, written by the core from the command itself, so the card
/// shows what will happen rather than what the assistant says will happen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsentQuestion {
    pub message: String,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsentAnswer {
    Allow,
    Deny,
    /// No window to ask in.
    NoWindow,
}
