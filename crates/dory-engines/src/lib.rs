//! Model providers that run on any desktop platform. Each module implements one
//! `ModelProvider<K>` and its engine trait. Only the composition root names them.

mod ollama;
mod sherpa;

pub use ollama::Ollama;
pub use sherpa::Sherpa;
