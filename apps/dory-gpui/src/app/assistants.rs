//! Settings > Assistants: whether assistants may read the library, the assistants the user
//! connected, and what they read lately. Connecting creates the assistant's key and puts the exact
//! setup on the clipboard, with the helper's path inside this app, so the user only pastes.

use std::path::{Path, PathBuf};

use dory_core::domain::{AssistantClient, AuditEntry, Exposure, Timestamp, Writes};
use gpui_kit::{ClipboardItem, Context, SharedString};

use super::AppState;

/// How many activity lines Settings shows.
pub const ACTIVITY_LINES: usize = 12;

/// How long Copy Setup shows its check mark after a copy.
pub const COPIED_FOR: std::time::Duration = std::time::Duration::from_secs(2);

/// What Settings > Assistants shows, read off the main thread.
#[derive(Clone, Debug, Default)]
pub enum Assistants {
    #[default]
    Unread,
    Reading,
    Read(AssistantsView),
    Failed(SharedString),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssistantsView {
    pub enabled: bool,
    pub clients: Vec<AssistantClient>,
    /// Newest first.
    pub activity: Vec<AuditEntry>,
}

/// An assistant app Dory can write the setup for. Each is connected as one assistant, named
/// after the app.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Target {
    #[default]
    ClaudeCode,
    ClaudeDesktop,
    Cursor,
    Codex,
}

impl Target {
    pub const ALL: [Target; 4] = [
        Target::ClaudeCode,
        Target::ClaudeDesktop,
        Target::Cursor,
        Target::Codex,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Target::ClaudeCode => "Claude Code",
            Target::ClaudeDesktop => "Claude Desktop",
            Target::Cursor => "Cursor",
            Target::Codex => "Codex",
        }
    }

    /// What to do with the copied text.
    pub fn next_step(self) -> &'static str {
        match self {
            Target::ClaudeCode => {
                "Copied. Paste it into Terminal and press Return, then start a new Claude Code session."
            }
            Target::ClaudeDesktop => {
                "Copied. In Claude Desktop, open Settings > Developer > Edit Config, add the \"dory\" entry under \"mcpServers\", then quit and reopen Claude."
            }
            Target::Cursor => {
                "Copied. Add the \"dory\" entry under \"mcpServers\" in ~/.cursor/mcp.json, keeping any servers already there, then restart Cursor."
            }
            Target::Codex => {
                "Copied. Add it to ~/.codex/config.toml, then start a new Codex session."
            }
        }
    }
}

/// The result of the last Connect, shown under the buttons.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Connected {
    Copied {
        target: Target,
        /// What was copied, with the key shortened, for the user to check.
        shown: String,
        replaced: bool,
    },
    Failed(SharedString),
}

/// `dory-mcp`, beside this app's executable: in a bundle that is `Contents/MacOS/dory-mcp`, and
/// in a Cargo run the helper built into the same target folder.
pub fn helper_path() -> Option<PathBuf> {
    let helper = std::env::current_exe().ok()?.with_file_name("dory-mcp");
    helper.is_file().then_some(helper)
}

/// `s` quoted for a POSIX shell when it needs it.
fn shell_quote(s: &str) -> String {
    let plain = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-=:@".contains(c));
    if plain {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// The text the user pastes: a `claude mcp add` command for Claude Code, or the `mcpServers`
/// entry for Claude Desktop's config. `data_dir` is passed on only when this run reads a folder
/// other than the default, so the helper reads the same library.
pub fn setup_text(target: Target, helper: &Path, key: &str, data_dir: Option<&Path>) -> String {
    let helper = helper.display().to_string();
    match target {
        Target::ClaudeCode => {
            let mut env = format!("--env DORY_MCP_KEY={key}");
            if let Some(dir) = data_dir {
                env.push_str(&format!(
                    " --env {}",
                    shell_quote(&format!("DORY_DATA_DIR={}", dir.display()))
                ));
            }
            format!(
                "claude mcp add {env} --scope user dory -- {}",
                shell_quote(&helper)
            )
        }
        Target::ClaudeDesktop | Target::Cursor => {
            let mut env = serde_json::Map::new();
            env.insert("DORY_MCP_KEY".into(), key.into());
            if let Some(dir) = data_dir {
                env.insert("DORY_DATA_DIR".into(), dir.display().to_string().into());
            }
            let mut server = serde_json::json!({ "command": helper, "env": env });
            if target == Target::Cursor {
                server["type"] = "stdio".into();
            }
            let entry = serde_json::json!({ "mcpServers": { "dory": server } });
            serde_json::to_string_pretty(&entry).unwrap_or_default()
        }
        Target::Codex => {
            let mut env = format!("DORY_MCP_KEY = {}", toml_string(key));
            if let Some(dir) = data_dir {
                env.push_str(&format!(
                    ", DORY_DATA_DIR = {}",
                    toml_string(&dir.display().to_string())
                ));
            }
            format!(
                "[mcp_servers.dory]\ncommand = {}\nenv = {{ {env} }}\n",
                toml_string(&helper)
            )
        }
    }
}

/// `s` as a TOML basic string.
fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `text` with the key cut to its first characters, for showing on screen.
pub fn shorten_key(text: &str, key: &str) -> String {
    text.replace(key, &format!("{}…", &key[..key.len().min(9)]))
}

/// The data folder to pass to the helper, when this run does not use the default.
fn custom_data_dir() -> Option<PathBuf> {
    std::env::var_os(dory::DATA_DIR_VAR)
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
}

impl AppState {
    pub fn assistants(&self) -> &Assistants {
        &self.assistants
    }

    pub fn connected(&self) -> Option<&Connected> {
        self.connected.as_ref()
    }

    /// Whether Copy Setup copied within the last [`COPIED_FOR`].
    pub fn just_copied(&self) -> bool {
        self.copied
    }

    /// Shows the check mark, and clears it [`COPIED_FOR`] later unless another copy came first.
    fn flash_copied(&mut self, cx: &mut Context<Self>) {
        self.copied = true;
        self.copy_flash += 1;
        let flash = self.copy_flash;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_FOR).await;
            let _ = this.update(cx, |state, cx| {
                if state.copy_flash == flash {
                    state.copied = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Where Connect says the helper is. Found beside the app at launch; tests point it at a
    /// file of their own.
    pub fn set_assistant_helper(&mut self, helper: Option<PathBuf>) {
        self.assistant_helper = helper;
    }

    /// Reads access, the assistants, and the latest activity, off the main thread.
    pub fn read_assistants(&mut self, cx: &mut Context<Self>) {
        if matches!(self.assistants, Assistants::Unread | Assistants::Failed(_)) {
            self.assistants = Assistants::Reading;
            cx.notify();
        }
        let read = self.core.run(cx, |dory| {
            let access = dory.assistants().access().map_err(|e| e.to_string())?;
            let activity = dory::Config::from_env()
                .map_err(|e| e.to_string())
                .and_then(|config| {
                    dory::audit_reader(&config)
                        .since(Timestamp(0), ACTIVITY_LINES)
                        .map_err(|e| e.to_string())
                })
                .unwrap_or_default();
            Ok::<_, String>(AssistantsView {
                enabled: access.enabled,
                clients: access.clients,
                activity,
            })
        });
        cx.spawn(async move |this, cx| {
            let read = read.await;
            let _ = this.update(cx, |state, cx| {
                state.assistants = match read {
                    Ok(view) => Assistants::Read(view),
                    Err(error) => Assistants::Failed(error.into()),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Runs one change to assistant access, then reads it all again.
    fn change_assistants(
        &mut self,
        cx: &mut Context<Self>,
        change: impl FnOnce(&dory_core::app::Dory) -> Result<(), String> + Send + 'static,
    ) {
        let work = self.core.run(cx, change);
        cx.spawn(async move |this, cx| {
            let done = work.await;
            let _ = this.update(cx, |state, cx| {
                if let Err(error) = done {
                    let text = format!("Could not change assistant access: {error}");
                    crate::a11y::announce(&text);
                    state.assistants = Assistants::Failed(text.into());
                }
                state.read_assistants(cx);
            });
        })
        .detach();
    }

    /// Says why Connect did nothing, under its buttons.
    pub fn not_connected(&mut self, why: &'static str, cx: &mut Context<Self>) {
        crate::a11y::announce(why);
        self.connected = Some(Connected::Failed(why.into()));
        cx.notify();
    }

    pub fn set_assistants_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        self.change_assistants(cx, move |dory| {
            dory.assistants().set_enabled(on).map_err(|e| e.to_string())
        });
    }

    pub fn set_assistant_writes(&mut self, label: String, writes: Writes, cx: &mut Context<Self>) {
        self.change_assistants(cx, move |dory| {
            dory.assistants()
                .set_writes(&label, writes)
                .map_err(|e| e.to_string())
        });
    }

    pub fn remove_assistant(&mut self, label: String, cx: &mut Context<Self>) {
        if self.connected.as_ref().is_some_and(
            |c| matches!(c, Connected::Copied { target, .. } if target.label() == label),
        ) {
            self.connected = None;
        }
        self.change_assistants(cx, move |dory| {
            dory.assistants().remove(&label).map_err(|e| e.to_string())
        });
    }

    /// Gives `target` a new key at `exposure`, replacing the one it had, and copies its setup.
    pub fn connect_assistant(
        &mut self,
        target: Target,
        exposure: Exposure,
        writes: Writes,
        cx: &mut Context<Self>,
    ) {
        let Some(helper) = self.assistant_helper.clone() else {
            self.connected = Some(Connected::Failed(
                "This copy of Dory has no dory-mcp helper beside it. Build the app bundle with scripts/bundle-macos.sh.".into(),
            ));
            cx.notify();
            return;
        };
        let work = self.core.run(cx, move |dory| {
            let key = dory::new_client_key().map_err(|e| e.to_string())?;
            let assistants = dory.assistants();
            let replaced = assistants
                .access()
                .map_err(|e| e.to_string())?
                .clients
                .iter()
                .any(|c| c.label == target.label());
            if replaced {
                assistants
                    .remove(target.label())
                    .map_err(|e| e.to_string())?;
            }
            assistants
                .add(target.label(), exposure, writes, key.clone())
                .map_err(|e| e.to_string())?;
            Ok::<_, String>((key, replaced))
        });
        cx.spawn(async move |this, cx| {
            let done = work.await;
            let _ = this.update(cx, |state, cx| {
                state.connected = Some(match done {
                    Ok((key, replaced)) => {
                        let text =
                            setup_text(target, &helper, key.as_str(), custom_data_dir().as_deref());
                        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                        crate::a11y::announce(target.next_step());
                        state.flash_copied(cx);
                        Connected::Copied {
                            target,
                            shown: shorten_key(&text, key.as_str()),
                            replaced,
                        }
                    }
                    Err(error) => Connected::Failed(format!("Could not connect: {error}").into()),
                });
                state.read_assistants(cx);
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "dk_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn claude_code_gets_one_command_with_the_key_and_the_helper() {
        let helper = Path::new("/Applications/Dory.app/Contents/MacOS/dory-mcp");
        assert_eq!(
            setup_text(Target::ClaudeCode, helper, KEY, None),
            format!(
                "claude mcp add --env DORY_MCP_KEY={KEY} --scope user dory -- /Applications/Dory.app/Contents/MacOS/dory-mcp"
            )
        );
        let spaced = Path::new("/Users/a b/Dory's.app/Contents/MacOS/dory-mcp");
        let text = setup_text(
            Target::ClaudeCode,
            spaced,
            KEY,
            Some(Path::new("/tmp/lane 1")),
        );
        assert!(text.contains("--env 'DORY_DATA_DIR=/tmp/lane 1'"), "{text}");
        assert!(
            text.ends_with("-- '/Users/a b/Dory'\\''s.app/Contents/MacOS/dory-mcp'"),
            "{text}"
        );
    }

    #[test]
    fn claude_desktop_gets_an_mcp_servers_entry() {
        let helper = Path::new("/Applications/Dory.app/Contents/MacOS/dory-mcp");
        let text = setup_text(Target::ClaudeDesktop, helper, KEY, None);
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        let dory = &value["mcpServers"]["dory"];
        assert_eq!(dory["command"], helper.display().to_string());
        assert_eq!(dory["env"]["DORY_MCP_KEY"], KEY);
        assert!(dory["env"].get("DORY_DATA_DIR").is_none());
    }

    #[test]
    fn cursor_gets_a_stdio_entry_and_codex_a_config_table() {
        let helper = Path::new("/Applications/Dory.app/Contents/MacOS/dory-mcp");
        let cursor: serde_json::Value =
            serde_json::from_str(&setup_text(Target::Cursor, helper, KEY, None)).unwrap();
        assert_eq!(cursor["mcpServers"]["dory"]["type"], "stdio");
        assert_eq!(cursor["mcpServers"]["dory"]["env"]["DORY_MCP_KEY"], KEY);
        let desktop: serde_json::Value =
            serde_json::from_str(&setup_text(Target::ClaudeDesktop, helper, KEY, None)).unwrap();
        assert!(desktop["mcpServers"]["dory"].get("type").is_none());

        let codex = setup_text(Target::Codex, helper, KEY, Some(Path::new("/tmp/a \"b\"")));
        assert_eq!(
            codex,
            format!(
                "[mcp_servers.dory]\ncommand = \"/Applications/Dory.app/Contents/MacOS/dory-mcp\"\nenv = {{ DORY_MCP_KEY = \"{KEY}\", DORY_DATA_DIR = \"/tmp/a \\\"b\\\"\" }}\n"
            )
        );
    }

    #[test]
    fn the_key_is_shortened_on_screen() {
        let shown = shorten_key(&format!("x {KEY} y"), KEY);
        assert_eq!(shown, "x dk_012345… y");
        assert!(!shown.contains(&KEY[12..]));
    }
}
