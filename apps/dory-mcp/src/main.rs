//! `dory-mcp`: an MCP server over stdio that lets an assistant search and read the user's Dory
//! meetings. An assistant client such as Claude Desktop or Claude Code launches it with the key
//! the user gave that assistant in `DORY_MCP_KEY`.
//!
//! It reads the library without the data folder's lock and never writes it, so it works whether
//! or not Dory is open. stdout carries only MCP messages; diagnostics go to stderr.

mod out;
mod prompts;
mod server;

use std::process::ExitCode;

use dory::Config;
use dory_core::domain::ClientKey;
use rmcp::ServiceExt;

pub const KEY_VAR: &str = "DORY_MCP_KEY";

fn assistant(config: &Config) -> Result<dory_core::assistant::Assistant, String> {
    let key = match std::env::var(KEY_VAR) {
        Ok(raw) if !raw.trim().is_empty() => match ClientKey::parse(&raw) {
            Some(key) => Some(key),
            None => {
                return Err(format!(
                    "{KEY_VAR} is not a Dory key. Copy it again from `dory-cli assistants add`."
                ));
            }
        },
        _ => None,
    };
    dory::open_assistant(config, key).map_err(|e| {
        let mut text = e.to_string();
        let mut source = std::error::Error::source(&e);
        while let Some(cause) = source {
            text.push_str(": ");
            text.push_str(&cause.to_string());
            source = cause.source();
        }
        text
    })
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let assistant = Config::from_env()
        .map_err(|e| e.to_string())
        .and_then(|config| assistant(&config));
    if let Err(why) = &assistant {
        eprintln!("dory-mcp: {why}");
    }
    let service = match server::Server::new(assistant)
        .serve(rmcp::transport::stdio())
        .await
    {
        Ok(service) => service,
        Err(e) => {
            eprintln!("dory-mcp: the client closed before it started: {e}");
            return ExitCode::SUCCESS;
        }
    };
    match service.waiting().await {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("dory-mcp: {e}");
            ExitCode::FAILURE
        }
    }
}
