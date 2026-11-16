use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub(super) struct ChatBody<'a> {
    pub model: &'a str,
    pub messages: [Message<'a>; 2],
    pub stream: bool,
    pub think: bool,
    pub options: Options,
}

#[derive(Serialize)]
pub(super) struct Message<'a> {
    pub role: &'static str,
    pub content: &'a str,
}

#[derive(Serialize)]
pub(super) struct Options {
    pub num_ctx: u32,
    pub temperature: f32,
    pub num_predict: u32,
}

#[derive(Deserialize)]
pub(super) struct ChatLine {
    #[serde(default)]
    pub message: Option<ChatMessage>,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct ChatMessage {
    #[serde(default)]
    pub content: String,
}

#[derive(Serialize)]
pub(super) struct PullBody<'a> {
    pub model: &'a str,
    pub stream: bool,
}

#[derive(Deserialize)]
pub(super) struct PullLine {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub digest: Option<String>,
    #[serde(default)]
    pub total: Option<u64>,
    #[serde(default)]
    pub completed: Option<u64>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct Tags {
    pub models: Vec<Tag>,
}

#[derive(Deserialize)]
pub(super) struct Tag {
    pub name: String,
    #[serde(default)]
    pub model: String,
}
