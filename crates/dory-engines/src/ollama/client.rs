use std::io::{BufRead, BufReader};
use std::time::Duration;

use dory_core::ports::CancelToken;
use serde::Serialize;
use serde::de::DeserializeOwned;
use ureq::Agent;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Why a call to the Ollama server failed, before the caller maps it to its own error type.
pub(super) enum Fail {
    Cancelled,
    /// Nothing answered, or what answered is not Ollama.
    Unreachable(String),
    /// The connection broke after the response began.
    Stream(String),
    Malformed(String),
    /// Ollama answered with an error, either as an HTTP status or as an `error` line in a stream.
    Remote {
        status: Option<u16>,
        message: String,
    },
}

pub(super) enum Next {
    Continue,
    Stop,
}

#[derive(Clone)]
pub(super) struct Client {
    base: String,
}

impl Client {
    pub(super) fn new(base_url: &str) -> Self {
        Self {
            base: base_url.trim_end_matches('/').to_owned(),
        }
    }

    pub(super) fn base(&self) -> &str {
        &self.base
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn agent(global: Option<Duration>) -> Agent {
        let config = Agent::config_builder()
            .proxy(None)
            .http_status_as_error(false)
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(global)
            .build();
        Agent::new_with_config(config)
    }

    pub(super) fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        timeout: Duration,
    ) -> Result<T, Fail> {
        let agent = Self::agent(Some(timeout));
        let mut response = agent
            .get(&self.url(path))
            .call()
            .map_err(|e| Fail::Unreachable(e.to_string()))?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            return Err(remote(status, &read_error_body(&mut response)));
        }
        response
            .body_mut()
            .read_json()
            .map_err(|e| Fail::Unreachable(format!("not an Ollama response: {e}")))
    }

    /// POSTs `body` and hands each NDJSON line of the response to `on_line` as it arrives.
    ///
    /// A blocked read is interrupted by shutting the agent's sockets down from the cancel hook, so
    /// a cancel returns within the time the kernel takes to wake the reader. The agent is private
    /// to this call, so the shutdown touches no other request.
    pub(super) fn post_lines(
        &self,
        path: &str,
        body: &impl Serialize,
        timeout: Option<Duration>,
        cancel: &CancelToken,
        mut on_line: impl FnMut(&str) -> Result<Next, Fail>,
    ) -> Result<(), Fail> {
        if cancel.is_cancelled() {
            return Err(Fail::Cancelled);
        }
        let agent = Self::agent(timeout);
        let interrupt = agent.clone();
        cancel.on_cancel(move || interrupt.shutdown());

        let sent = agent.post(&self.url(path)).send_json(body);
        let mut response = match sent {
            Ok(response) => response,
            Err(_) if cancel.is_cancelled() => return Err(Fail::Cancelled),
            Err(e) => return Err(Fail::Unreachable(e.to_string())),
        };
        if !response.status().is_success() {
            if cancel.is_cancelled() {
                return Err(Fail::Cancelled);
            }
            let status = response.status().as_u16();
            return Err(remote(status, &read_error_body(&mut response)));
        }

        let mut reader = BufReader::new(response.body_mut().as_reader());
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {}
                Err(_) if cancel.is_cancelled() => return Err(Fail::Cancelled),
                Err(e) => return Err(Fail::Stream(e.to_string())),
            }
            if cancel.is_cancelled() {
                return Err(Fail::Cancelled);
            }
            let text = line.trim();
            if text.is_empty() {
                continue;
            }
            if let Next::Stop = on_line(text)? {
                return Ok(());
            }
        }
        if cancel.is_cancelled() {
            return Err(Fail::Cancelled);
        }
        Ok(())
    }
}

#[derive(serde::Deserialize)]
struct ErrorBody {
    error: String,
}

fn remote(status: u16, body: &str) -> Fail {
    let message = serde_json::from_str::<ErrorBody>(body)
        .map(|b| b.error)
        .unwrap_or_else(|_| body.trim().to_owned());
    Fail::Remote {
        status: Some(status),
        message,
    }
}

fn read_error_body(response: &mut ureq::http::Response<ureq::Body>) -> String {
    response.body_mut().read_to_string().unwrap_or_default()
}
