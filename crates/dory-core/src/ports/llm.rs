use super::{CancelToken, EngineError};

pub trait LanguageModel: Send + Sync {
    /// For sizing chunks. A conservative estimate is fine.
    fn count_tokens(&self, text: &str) -> u32;

    /// Streams pieces into `sink` and returns the full text.
    fn complete(
        &self,
        req: &CompletionRequest<'_>,
        sink: &mut dyn TextSink,
        cancel: &CancelToken,
    ) -> Result<String, EngineError>;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompletionRequest<'a> {
    pub system: &'a str,
    pub user: &'a str,
    pub max_output_tokens: u32,
    pub temperature: f32,
}

pub trait TextSink {
    fn text(&mut self, piece: &str);
}
