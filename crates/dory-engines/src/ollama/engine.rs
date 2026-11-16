use std::time::Duration;

use dory_core::ports::{CancelToken, CompletionRequest, EngineError, LanguageModel, TextSink};

use super::client::{Client, Fail, Next};
use super::wire::{ChatBody, ChatLine, Message, Options};

const CHAT_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Holds no connection and no server state. Ollama unloads the model after its own keep-alive,
/// because requests never set `keep_alive`, so dropping this releases everything Dory owns.
pub(super) struct OllamaModel {
    pub client: Client,
    pub model: String,
    pub num_ctx: u32,
}

impl LanguageModel for OllamaModel {
    /// One token per 3 UTF-8 bytes. Gemma averages about 4 bytes per token on English, so this
    /// overestimates by a third there, and CJK at 3 bytes per character costs at least a token each.
    fn count_tokens(&self, text: &str) -> u32 {
        u32::try_from(text.len().div_ceil(3)).unwrap_or(u32::MAX)
    }

    fn complete(
        &self,
        req: &CompletionRequest<'_>,
        sink: &mut dyn TextSink,
        cancel: &CancelToken,
    ) -> Result<String, EngineError> {
        let body = ChatBody {
            model: &self.model,
            messages: [
                Message {
                    role: "system",
                    content: req.system,
                },
                Message {
                    role: "user",
                    content: req.user,
                },
            ],
            stream: true,
            think: false,
            options: Options {
                num_ctx: self.num_ctx,
                temperature: req.temperature,
                num_predict: req.max_output_tokens,
            },
        };
        let mut text = String::new();
        let mut finished = false;
        self.client
            .post_lines("/api/chat", &body, Some(CHAT_TIMEOUT), cancel, |line| {
                let chunk: ChatLine = serde_json::from_str(line)
                    .map_err(|e| Fail::Malformed(format!("unreadable chat line: {e}")))?;
                if let Some(message) = chunk.error {
                    return Err(Fail::Remote {
                        status: None,
                        message,
                    });
                }
                if let Some(piece) = chunk.message.map(|m| m.content).filter(|c| !c.is_empty()) {
                    sink.text(&piece);
                    text.push_str(&piece);
                }
                finished = chunk.done;
                Ok(if finished { Next::Stop } else { Next::Continue })
            })
            .map_err(|fail| engine_error(fail, &self.model, self.client.base()))?;
        if finished {
            Ok(text)
        } else {
            Err(EngineError::ServerUnavailable(
                "Ollama closed the stream before the model finished".into(),
            ))
        }
    }
}

pub(super) fn engine_error(fail: Fail, model: &str, base: &str) -> EngineError {
    match fail {
        Fail::Cancelled => EngineError::Cancelled,
        Fail::Unreachable(why) => {
            EngineError::ServerUnavailable(format!("Ollama is not running at {base}: {why}"))
        }
        Fail::Stream(why) => {
            EngineError::ServerUnavailable(format!("the connection to Ollama broke: {why}"))
        }
        Fail::Malformed(message) => EngineError::Other {
            message,
            source: None,
        },
        Fail::Remote { status, message } => {
            let lower = message.to_ascii_lowercase();
            if status == Some(404) || lower.contains("not found") {
                EngineError::ModelMissing(model.to_owned())
            } else if lower.contains("terminated") || lower.contains("unexpectedly stopped") {
                EngineError::Crashed(message)
            } else if status == Some(503) {
                EngineError::ServerUnavailable(message)
            } else if status.is_some_and(|s| (400..500).contains(&s)) {
                EngineError::InputRejected(message)
            } else {
                EngineError::Other {
                    message,
                    source: None,
                }
            }
        }
    }
}
