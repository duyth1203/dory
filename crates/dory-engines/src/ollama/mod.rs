//! Summaries through a local Ollama server.

mod client;
mod engine;
mod wire;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use dory_core::domain::LangSet;
use dory_core::models::{
    Availability, Llm, LlmCaps, ModelAbout, ModelEntry, ModelId, ProviderId, Unavailable,
    UnavailableReason,
};
use dory_core::ports::{
    CancelToken, EngineError, LanguageModel, ModelProvider, ProvisionError, ProvisionSink,
};

use client::{Client, Fail, Next};
use engine::{OllamaModel, engine_error};
use wire::{PullBody, PullLine, Tags};

/// Fits a 2-hour transcript of about 30k tokens.
pub const DEFAULT_NUM_CTX: u32 = 32_768;

const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

struct Row {
    tag: &'static str,
    display: &'static str,
    recommended: bool,
    rank: i16,
    disk_bytes: Option<u64>,
    summary: &'static str,
    pros: &'static [&'static str],
    cons: &'static [&'static str],
}

const ROWS: [Row; 2] = [
    Row {
        tag: "gemma4:e4b",
        display: "Gemma 4 E4B",
        recommended: true,
        rank: 1,
        disk_bytes: Some(9_461_576_343),
        summary: "A compact model from Google that writes meeting summaries on your Mac.",
        pros: &[
            "Smaller and faster than the 12B model",
            "Handles a 2-hour meeting",
        ],
        cons: &[
            "Less accurate than the 12B model: 69.4% against 77.2% on the MMLU-Pro reasoning benchmark",
            "The first download is large, about 9.5 GB",
        ],
    },
    Row {
        tag: "gemma4:12b",
        display: "Gemma 4 12B",
        recommended: false,
        rank: 0,
        disk_bytes: Some(8_000_000_000),
        summary: "A larger model from Google that writes more accurate summaries but runs slower.",
        pros: &["More accurate: 77.2% against 69.4% on the MMLU-Pro reasoning benchmark"],
        cons: &[
            "Slower than the E4B model",
            "Needs more memory than the E4B model",
            "The first download is about 8 GB",
        ],
    },
];

/// `ModelProvider<Llm>` over Ollama's HTTP API.
#[derive(Clone)]
pub struct Ollama {
    client: Client,
    num_ctx: u32,
}

impl Ollama {
    /// `base_url` is normally `http://127.0.0.1:11434`. The composition root reads `OLLAMA_HOST`.
    pub fn new(base_url: &str) -> Self {
        Self {
            client: Client::new(base_url),
            num_ctx: DEFAULT_NUM_CTX,
        }
    }

    /// Sets the context window sent on every request and reported in each row's caps.
    pub fn with_num_ctx(mut self, num_ctx: u32) -> Self {
        self.num_ctx = num_ctx;
        self
    }

    fn installed(&self, model: &str) -> Result<bool, Fail> {
        let tags: Tags = self.client.get_json("/api/tags", PROBE_TIMEOUT)?;
        let wanted = canonical(model);
        Ok(tags
            .models
            .iter()
            .any(|t| canonical(&t.name) == wanted || canonical(&t.model) == wanted))
    }

    fn known_bytes(model: &str) -> Option<u64> {
        ROWS.iter()
            .find(|r| r.tag == model)
            .and_then(|r| r.disk_bytes)
    }
}

/// Ollama's own name rules: no tag means `latest`, and the registry prefix is optional.
fn canonical(name: &str) -> String {
    let name = name.to_ascii_lowercase();
    let name = name.strip_prefix("registry.ollama.ai/").unwrap_or(&name);
    let name = name.strip_prefix("library/").unwrap_or(name);
    if name.rsplit('/').next().is_some_and(|s| s.contains(':')) {
        name.to_owned()
    } else {
        format!("{name}:latest")
    }
}

impl ModelProvider<Llm> for Ollama {
    fn id(&self) -> ProviderId {
        ProviderId::parse("ollama").expect("a valid provider id")
    }

    fn catalog(&self) -> Vec<ModelEntry<Llm>> {
        ROWS.iter()
            .map(|row| ModelEntry {
                id: format!("ollama/{}", row.tag)
                    .parse()
                    .expect("a valid model id"),
                display_name: row.display.to_owned(),
                about: ModelAbout {
                    summary: row.summary.to_owned(),
                    pros: row.pros.iter().map(|s| (*s).to_owned()).collect(),
                    cons: row.cons.iter().map(|s| (*s).to_owned()).collect(),
                },
                caps: LlmCaps {
                    context_tokens: self.num_ctx,
                    languages: LangSet::Any,
                },
                recommended: row.recommended,
                rank: row.rank,
                disk_bytes: row.disk_bytes,
            })
            .collect()
    }

    fn availability(&self, model: &ModelId<Llm>) -> Availability {
        match self.installed(model.name()) {
            Ok(true) => Availability::Ready,
            Ok(false) => Availability::NeedsProvision {
                bytes: Self::known_bytes(model.name()),
            },
            Err(_) => Availability::Unavailable(Unavailable {
                reason: UnavailableReason::ServerUnreachable {
                    endpoint: self.client.base().to_owned(),
                },
                fix: Some("Ollama is not running".to_owned()),
            }),
        }
    }

    fn provision(
        &self,
        model: &ModelId<Llm>,
        progress: &mut dyn ProvisionSink,
        cancel: &CancelToken,
    ) -> Result<(), ProvisionError> {
        let body = PullBody {
            model: model.name(),
            stream: true,
        };
        let mut layers: BTreeMap<String, (u64, u64)> = BTreeMap::new();
        let mut step: Option<&'static str> = None;
        let mut succeeded = false;
        let mut announce = |progress: &mut dyn ProvisionSink, label: &'static str| {
            if step != Some(label) {
                step = Some(label);
                progress.step(label);
            }
        };

        self.client
            .post_lines("/api/pull", &body, None, cancel, |line| {
                let line: PullLine = serde_json::from_str(line)
                    .map_err(|e| Fail::Malformed(format!("unreadable pull line: {e}")))?;
                if let Some(message) = line.error {
                    return Err(Fail::Remote {
                        status: None,
                        message,
                    });
                }
                if let (Some(digest), Some(total)) = (line.digest, line.total) {
                    layers.insert(digest, (line.completed.unwrap_or(0), total));
                    announce(progress, "Downloading");
                    let (done, total) = layers
                        .values()
                        .fold((0, 0), |(d, t), (ld, lt)| (d + ld, t + lt));
                    progress.bytes(done, Some(total));
                    return Ok(Next::Continue);
                }
                match line.status.as_deref() {
                    Some("success") => {
                        succeeded = true;
                        return Ok(Next::Stop);
                    }
                    Some(s) if s.starts_with("pulling manifest") => {
                        announce(progress, "Fetching manifest");
                    }
                    Some(s) if s.starts_with("verifying") => announce(progress, "Verifying"),
                    Some(s) if s.starts_with("writing manifest") => {
                        announce(progress, "Finishing");
                    }
                    _ => {}
                }
                Ok(Next::Continue)
            })
            .map_err(|fail| {
                provision_error(fail, model.name(), layers.values().map(|l| l.1).sum())
            })?;

        if succeeded {
            Ok(())
        } else {
            Err(ProvisionError::Network(
                "Ollama closed the download before it finished".into(),
            ))
        }
    }

    fn open(
        &self,
        model: &ModelId<Llm>,
        cancel: &CancelToken,
    ) -> Result<Arc<dyn LanguageModel>, EngineError> {
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        let present = self
            .installed(model.name())
            .map_err(|fail| engine_error(fail, model.name(), self.client.base()))?;
        if !present {
            return Err(EngineError::ModelMissing(model.name().to_owned()));
        }
        Ok(Arc::new(OllamaModel {
            client: self.client.clone(),
            model: model.name().to_owned(),
            num_ctx: self.num_ctx,
        }))
    }
}

fn provision_error(fail: Fail, model: &str, bytes_seen: u64) -> ProvisionError {
    match fail {
        Fail::Cancelled => ProvisionError::Cancelled,
        Fail::Unreachable(_) => ProvisionError::NotProvisionable("Ollama is not running".into()),
        Fail::Stream(why) => ProvisionError::Network(why),
        Fail::Malformed(why) => ProvisionError::Other(why),
        Fail::Remote { status, message } => {
            let lower = message.to_ascii_lowercase();
            if lower.contains("file does not exist") || lower.contains("not found") {
                ProvisionError::NotProvisionable(format!("Ollama has no model named {model}"))
            } else if lower.contains("no space left") {
                ProvisionError::DiskFull { needed: bytes_seen }
            } else if status.is_some_and(|s| s >= 500)
                || ["dial tcp", "timeout", "eof", "connection"]
                    .iter()
                    .any(|w| lower.contains(w))
            {
                ProvisionError::Network(message)
            } else {
                ProvisionError::Other(message)
            }
        }
    }
}
