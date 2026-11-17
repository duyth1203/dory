//! How an assistant's change request reaches the app, and how the app asks the user.

use crate::domain::{ConsentAnswer, ConsentQuestion, RemoteOutcome, RemoteRequest};

/// The helper's side: carries a request to the running app and waits for its outcome. When the
/// app is not running it answers `Refusal::AppNotRunning` at once.
pub trait Remote: Send + Sync {
    fn send(&self, request: &RemoteRequest) -> RemoteOutcome;
}

/// The app's side: shows the user a question and answers once they click. The caller waits a
/// bounded time on the receiver.
pub trait Consent: Send + Sync {
    fn ask(&self, question: ConsentQuestion) -> crossbeam_channel::Receiver<ConsentAnswer>;
}
