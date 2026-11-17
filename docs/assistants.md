# Let an assistant read your meetings

Dory can let an AI assistant such as Claude Code, Claude Desktop, Cursor, or Codex search and read your recorded meetings. The assistant runs `dory-mcp`, a small helper inside Dory.app, which speaks the Model Context Protocol (MCP) over stdio. The helper only reads. It works whether or not Dory is open, and it never changes your library.

Access is off until you turn it on. Each assistant you add gets its own key and its own level. You can hide any recording from every assistant.

## What leaves your Mac

Anything an assistant reads from Dory is sent to that assistant's provider, like anything else you paste into it. Dory itself sends nothing. A cloud assistant such as Claude sends what it reads to its provider under that provider's retention settings, and Dory cannot see or limit what happens after that.

## Turn it on and connect an assistant

Open Dory's Settings (Command-Comma) and choose **Assistants**.

1. Check **Let assistants read your meetings**. The sentence under it is the one above.
2. Under **Connect an assistant**, choose what it may read. Nothing is chosen for you.

   | Level | The assistant can read |
   | --- | --- |
   | Titles | Each meeting's id, title, date, duration, and language. Search matches titles only. |
   | Summaries | Adds summaries, speaker names, and action items. Search also matches summaries. |
   | Full transcripts | Adds every word said. Search also matches what was said. |

3. Check **May ask for changes** if it may ask to rename meetings, name speakers, or summarize again. Each change still waits for your click.
4. Pick the app under **Assistant app** (Claude Code, Claude Desktop, Cursor, or Codex) and click **Copy Setup**. Dory gives that app its own key and copies its setup with the key and the helper's path already in it. Then paste it as below for that app.

Copying again for the same assistant gives it a new key, and the old one stops working, so paste the new setup too. **Connected** lists each assistant with its level, its **May ask for changes** switch, and **Remove**. **Activity** shows the latest reads.

Dory keeps only a digest of each key, so a lost key cannot be shown again. Copy the setup again instead.

The same can be done from Terminal with `dory-cli`, while Dory is not running: `dory-cli assistants on`, then `dory-cli assistants add "Claude Code" --exposure summaries`, which prints the key once.

## Connect Claude Code

Paste the copied command into Terminal and press Return, then start a new Claude Code session. By hand, it is this, with your key in place of `<key>`. `--env` has to come before `--scope`, or Claude Code reads the key as the server's name.

```bash
claude mcp add --env DORY_MCP_KEY=<key> --scope user dory -- /Applications/Dory.app/Contents/MacOS/dory-mcp
```

Check it with `claude mcp list`, which should show `dory` as connected. Then ask Claude Code things like "what did we decide about the budget in last week's sync?".

## Connect Claude Desktop

Open Claude Desktop's settings, go to Developer, and choose Edit Config. That opens `~/Library/Application Support/Claude/claude_desktop_config.json`. Add the copied `dory` entry under `mcpServers`, keeping any servers already there. It looks like this.

```json
{
  "mcpServers": {
    "dory": {
      "command": "/Applications/Dory.app/Contents/MacOS/dory-mcp",
      "env": { "DORY_MCP_KEY": "<key>" }
    }
  }
}
```

Quit and reopen Claude Desktop. Dory's tools then appear in its tools menu.

## Connect Cursor

Add the copied `dory` entry under `mcpServers` in `~/.cursor/mcp.json`, keeping any servers already there, then restart Cursor. It looks like this.

```json
{
  "mcpServers": {
    "dory": {
      "type": "stdio",
      "command": "/Applications/Dory.app/Contents/MacOS/dory-mcp",
      "env": { "DORY_MCP_KEY": "<key>" }
    }
  }
}
```

## Connect Codex

Add the copied table to `~/.codex/config.toml`, then start a new Codex session. It looks like this.

```toml
[mcp_servers.dory]
command = "/Applications/Dory.app/Contents/MacOS/dory-mcp"
env = { DORY_MCP_KEY = "<key>" }
```

## What an assistant can do

| Tool | What it returns |
| --- | --- |
| `search_recordings` | Meetings matching words, newest first, the way Dory's search box matches them: any word order, word starts, case and accents folded, quoted phrases in order, and one typo corrected when nothing matches. Each result says whether it matched in the title or the conversation, and carries a snippet with the matched words marked and, for a transcript match, the turn's speaker, time, and a `get_transcript` cursor. It searches only the text the assistant's level may read. Takes `query`, `since` and `until` as UTC days (`YYYY-MM-DD`), `limit` up to 25, and a `cursor` for the next page. |
| `get_recording` | One meeting's title, date, duration, whether it has a transcript and summary, and its speakers at the `summaries` level. |
| `get_summary` | One meeting's summary: key points, decisions, and action items. Needs `summaries`. |
| `list_action_items` | The action items of one meeting, or of recent meetings since a day. Needs `summaries`. |
| `get_transcript` | One meeting's transcript as speaker turns with times, a page at a time. Needs `transcripts`. |

Dory also offers two prompts and three kinds of resources. In Claude Code, prompts show up as `/mcp__dory__prepare_one_on_one` and `/mcp__dory__draft_follow_up_email`, and you attach a resource by typing `@dory:` and picking it. Both need the `summaries` level.

| Prompt or resource | What it does |
| --- | --- |
| `prepare_one_on_one` | Takes a person's name, finds your most recent meetings that mention them, and asks the assistant for an agenda from their summaries. |
| `draft_follow_up_email` | Takes a meeting id and asks the assistant to draft a short follow-up with decisions and action items. |
| `dory://recordings/<id>` | One meeting's details. |
| `dory://recordings/<id>/summary` | One meeting's summary. |
| `dory://recordings/<id>/transcript` | One meeting's whole transcript. Long meetings are large, so the `get_transcript` tool pages them instead. |

A prompt contains only Dory's instruction and links to the meetings, never meeting text, so nothing said in a meeting reaches the assistant as if you had typed it.

Every tool only reads. Meeting text comes back inside a marked block that tells the assistant it is recorded speech to treat as data, never as instructions, because anyone in a meeting can say something that sounds like an instruction.

## Let an assistant ask for changes

An assistant can also ask to rename a meeting (`set_title`), name a speaker (`rename_speaker`), or summarize a meeting again with an instruction such as "focus on the budget" (`resummarize`). It cannot start or stop a recording. A new summary runs on your Mac with your summary model, like any other. Edits are off for every assistant until you turn them on.

```bash
dory-cli assistants edits "Claude Code" ask
```

Each request then shows an alert in Dory's window with the exact change, for example "Claude Code wants to rename a meeting. From: Two hour planning, To: Q3 planning". Don't Allow is the default button. Nothing changes unless you click Allow. If you don't answer within 90 seconds, or Dory's window is closed, or Dory isn't running, the assistant is told nothing changed. If the assistant read meeting text in the last five minutes, the alert says so, because meeting text can carry instructions from other people.

## Change, hide, and review

- Change an assistant's level with `dory-cli assistants exposure "Claude Code" transcripts`. It applies on the assistant's next request.
- Remove an assistant with `dory-cli assistants remove "Claude Code"`. Its key stops working on its next request.
- Bring back the summary a re-summarize replaced with `dory-cli summary restore <id>`. Running it again swaps back. Dory keeps one earlier summary per meeting, and the instruction stays with the meeting for later summaries.
- Hide a recording from every assistant with `dory-cli private <id> on`, and show it again with `off`. A hidden recording answers exactly like one that does not exist. You still see it in Dory.
- Turn access off with `dory-cli assistants off`.
- See every read with `dory-cli assistants activity`, newest first, for the last 30 days, or `--days N`. Each line says which assistant read what and when, never the meeting text. Dory deletes activity older than 30 days each time it opens, and `dory-cli assistants prune` does it on demand.
- List assistants and whether access is on with `dory-cli assistants list`.

Settings > Assistants does all of this while Dory runs. The `dory-cli` commands that change access need Dory's data folder to themselves, so quit Dory before using them; `list` and `activity` work while Dory runs.

## When something is wrong

The helper answers every request, even when it cannot read, with a sentence the assistant can pass on. "Assistant access to Dory is off" means access is off. "Dory does not know this assistant's key" means the key was removed or mistyped. "Open Dory to update this library" means Dory was updated and has not opened since. When Dory updates while an assistant is connected, the helper exits after its next request so the assistant restarts it from the new version.

For tests and development, set `DORY_DATA_DIR` to a throwaway folder in the assistant's config `env`, next to the key, and the helper reads that folder instead of your library.
