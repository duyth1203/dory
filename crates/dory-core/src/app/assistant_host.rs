//! The app's side of an assistant's change request. The app holds the data folder, so it alone
//! decides: it resolves the assistant from the key, checks the rules, asks the user in Dory, and
//! only then applies the change through the same write the app's own edits use.

use std::sync::Arc;
use std::time::Duration;

use super::Dory;
use super::events::Db;
use crate::assistant::policy;
use crate::domain::{
    Access, Actor, AuditEntry, AuditOutcome, ConsentAnswer, ConsentQuestion, Exposure,
    RecordingEdit, RecordingId, Refusal, RemoteCommand, RemoteOutcome, RemoteRequest, SpeakerId,
    Timestamp,
};
use crate::ports::{AuditRead, AuditSink, Change, Clock, Consent, StoreError};

/// How long a request waits for the user before it is refused as unanswered.
pub const CONSENT_WAIT: Duration = Duration::from_secs(90);

/// A command the user allowed. Only [`AssistantHost`] makes one, so nothing can apply a change
/// that skipped the question.
struct Approved {
    command: RemoteCommand,
}

pub struct AssistantHost {
    db: Db,
    nudge: super::pipeline::Nudge,
    clock: Arc<dyn Clock>,
    consent: Arc<dyn Consent>,
    audit: Arc<dyn AuditSink>,
    audit_read: Arc<dyn AuditRead>,
    wait: Duration,
}

impl Dory {
    /// The handler for assistants' change requests. It holds its own handles, so it can run on
    /// any thread for as long as the process lives.
    pub fn assistant_host(
        &self,
        consent: Arc<dyn Consent>,
        audit: Arc<dyn AuditSink>,
        audit_read: Arc<dyn AuditRead>,
    ) -> AssistantHost {
        AssistantHost {
            db: self.db.clone(),
            nudge: self.pipeline.nudge.clone(),
            clock: Arc::clone(&self.clock),
            consent,
            audit,
            audit_read,
            wait: CONSENT_WAIT,
        }
    }
}

fn library(e: StoreError) -> Refusal {
    Refusal::Library(e.to_string())
}

/// User-facing text from a request: one line, no control characters, at most 200 characters.
fn text(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(RemoteCommand::MAX_TEXT_CHARS)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// An instruction for the summary model: one line, no control characters, at most 500
/// characters, not empty.
fn instruction_text(s: &str) -> Result<String, Refusal> {
    let cleaned: String = s.chars().filter(|c| !c.is_control()).collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() || cleaned.chars().count() > RemoteCommand::MAX_INSTRUCTION_CHARS {
        return Err(Refusal::Library(format!(
            "an instruction is 1 to {} characters",
            RemoteCommand::MAX_INSTRUCTION_CHARS
        )));
    }
    Ok(cleaned.to_owned())
}

fn optional(s: &Option<String>) -> Option<String> {
    s.as_deref().map(text).filter(|s| !s.is_empty())
}

impl AssistantHost {
    /// Waits `wait` for the user instead of [`CONSENT_WAIT`]. For tests.
    pub fn with_wait(mut self, wait: Duration) -> AssistantHost {
        self.wait = wait;
        self
    }

    pub fn handle(&self, request: &RemoteRequest) -> RemoteOutcome {
        let (label, outcome) = self.decide(request);
        let entry = AuditEntry {
            at: self.clock.now(),
            actor: Actor::App,
            client: label,
            reported_name: request.reported_name.clone(),
            access: Access::Write {
                what: request.command.what().to_owned(),
                id: Some(request.command.id().clone()),
            },
            outcome: match &outcome {
                Ok(()) => AuditOutcome::Served { items: 1 },
                Err(why) => AuditOutcome::Refused(why.clone()),
            },
        };
        if let Err(e) = self.audit.append(&entry) {
            eprintln!("assistants: could not log a change request: {e}");
        }
        outcome
    }

    fn decide(&self, request: &RemoteRequest) -> (Option<String>, RemoteOutcome) {
        let store = &self.db.store;
        let client = match &request.key {
            Some(key) => match store.assistant_client(key) {
                Ok(client) => client,
                Err(e) => return (None, Err(library(e))),
            },
            None => None,
        };
        let label = client.as_ref().map(|c| c.label.clone());
        let outcome = (|| {
            let access = store.assistant_access().map_err(library)?;
            let grant = policy::grant(access.enabled, request.key.is_some(), client)?;
            policy::may_write(&grant)?;
            let question = self.question(&grant, &request.command)?;
            match self.consent.ask(question).recv_timeout(self.wait) {
                Ok(ConsentAnswer::Allow) => self.perform(Approved {
                    command: request.command.clone(),
                }),
                Ok(ConsentAnswer::Deny) => Err(Refusal::Declined),
                Ok(ConsentAnswer::NoWindow)
                | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => Err(Refusal::NoWindow),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => Err(Refusal::Unanswered),
            }
        })();
        (label, outcome)
    }

    /// The question for the user, after every check that does not need them.
    fn question(
        &self,
        grant: &policy::Grant,
        command: &RemoteCommand,
    ) -> Result<ConsentQuestion, Refusal> {
        let store = &self.db.store;
        let id = command.id();
        let recording = store
            .recording(id)
            .map_err(library)?
            .ok_or(Refusal::NotFound)?;
        let hidden = store.hidden_from_assistants(id).map_err(library)?;
        let need = match command {
            RemoteCommand::SetTitle { .. } => Exposure::Titles,
            RemoteCommand::RenameSpeaker { .. } | RemoteCommand::Resummarize { .. } => {
                Exposure::Summaries
            }
        };
        policy::reveal(grant, hidden, need)?;
        let title = recording
            .title
            .clone()
            .unwrap_or_else(|| "an untitled meeting".to_owned());
        let (message, mut detail) = match command {
            RemoteCommand::SetTitle { title: new, .. } => (
                format!("{} wants to rename a meeting.", grant.label()),
                format!(
                    "From: {}\nTo: {}",
                    text(&title),
                    optional(new).unwrap_or_else(|| "no title (Dory shows the date)".to_owned())
                ),
            ),
            RemoteCommand::RenameSpeaker {
                speaker,
                revision,
                name,
                ..
            } => {
                self.speaker(id, speaker, *revision)?;
                (
                    format!(
                        "{} wants to name a speaker in \"{}\".",
                        grant.label(),
                        text(&title)
                    ),
                    format!(
                        "{} becomes {}",
                        text(speaker),
                        optional(name).unwrap_or_else(|| "unnamed".to_owned())
                    ),
                )
            }
            RemoteCommand::Resummarize { instruction, .. } => {
                let instruction = instruction_text(instruction)?;
                store
                    .transcript(id)
                    .map_err(library)?
                    .ok_or(Refusal::NotFound)?;
                let keeps = if store.summary(id).map_err(library)?.is_some() {
                    "\n\nThis replaces the current summary. Dory keeps it, and `dory-cli summary restore` brings it back."
                } else {
                    ""
                };
                (
                    format!(
                        "{} wants to summarize \"{}\" again.",
                        grant.label(),
                        text(&title)
                    ),
                    format!("With this instruction: \"{instruction}\"{keeps}"),
                )
            }
        };
        let recent = self
            .audit_read
            .since(
                Timestamp(self.clock.now().0 - policy::TAINT_WINDOW_MS),
                1_000,
            )
            .unwrap_or_default();
        if let Some(seconds) = policy::taint(&recent, grant.label(), self.clock.now()) {
            detail.push_str(&format!(
                "\n\nThis request came {seconds} seconds after {} read meeting text. Meeting text can carry instructions from other people, so check that you asked for this.",
                grant.label()
            ));
        }
        Ok(ConsentQuestion { message, detail })
    }

    /// The speaker labelled `label` in the transcript at `revision`.
    fn speaker(
        &self,
        id: &RecordingId,
        label: &str,
        revision: crate::domain::Revision,
    ) -> Result<SpeakerId, Refusal> {
        let store = &self.db.store;
        let transcript = store
            .transcript(id)
            .map_err(library)?
            .ok_or(Refusal::NotFound)?;
        if transcript.revision != revision {
            return Err(Refusal::StaleRevision);
        }
        let names = store.speaker_names(id).map_err(library)?;
        transcript
            .segments
            .iter()
            .find(|s| names.label(s, revision) == label.trim())
            .map(|s| s.speaker)
            .ok_or(Refusal::NotFound)
    }

    fn perform(&self, approved: Approved) -> RemoteOutcome {
        match approved.command {
            RemoteCommand::SetTitle { id, title } => self
                .db
                .apply(vec![Change::RecordingEdited {
                    id,
                    edit: RecordingEdit::Title(optional(&title)),
                }])
                .map_err(library),
            RemoteCommand::Resummarize { id, instruction } => {
                let instruction = instruction_text(&instruction)?;
                self.db
                    .apply(vec![
                        Change::SummaryInstructionSet {
                            id: id.clone(),
                            instruction: Some(instruction),
                        },
                        Change::JobEnqueued {
                            id,
                            goal: crate::domain::Goal::SUMMARIZE,
                            priority: crate::domain::Priority::Normal,
                            at: self.clock.now(),
                        },
                    ])
                    .map_err(library)?;
                self.nudge.wake();
                Ok(())
            }
            RemoteCommand::RenameSpeaker {
                id,
                speaker,
                revision,
                name,
            } => {
                let speaker = self.speaker(&id, &speaker, revision)?;
                let mut names = self.db.store.speaker_names(&id).map_err(library)?;
                names.rename(revision, speaker, optional(&name));
                self.db
                    .apply(vec![Change::SpeakerNamesReplaced { id, names }])
                    .map_err(library)
            }
        }
    }
}

/// A question waiting for the user, as the app receives it.
pub struct ConsentRequest {
    pub question: ConsentQuestion,
    reply: crossbeam_channel::Sender<ConsentAnswer>,
}

impl ConsentRequest {
    pub fn answer(self, answer: ConsentAnswer) {
        let _ = self.reply.send(answer);
    }
}

/// A [`Consent`] that hands each question to the app's UI through a channel.
pub struct ChannelConsent {
    requests: async_channel::Sender<ConsentRequest>,
}

impl Consent for ChannelConsent {
    fn ask(&self, question: ConsentQuestion) -> crossbeam_channel::Receiver<ConsentAnswer> {
        let (reply, answer) = crossbeam_channel::bounded(1);
        let request = ConsentRequest {
            question,
            reply: reply.clone(),
        };
        if self.requests.try_send(request).is_err() {
            let _ = reply.send(ConsentAnswer::NoWindow);
        }
        answer
    }
}

/// The consent port and the queue the app's UI answers from.
pub fn consent_channel() -> (ChannelConsent, async_channel::Receiver<ConsentRequest>) {
    let (requests, queue) = async_channel::unbounded();
    (ChannelConsent { requests }, queue)
}
