# Let an assistant read your meetings

Dory can let an AI assistant such as Claude Desktop or Claude Code search and read your recorded meetings. The assistant runs `dory-mcp`, a small helper inside Dory.app, which speaks the Model Context Protocol (MCP) over stdio. The helper only reads. It works whether or not Dory is open, and it never changes your library.

Access is off until you turn it on. Each assistant you add gets its own key and its own level. You can hide any recording from every assistant.

## What leaves your Mac

Anything an assistant reads from Dory is sent to that assistant's provider, like anything else you paste into it. Dory itself sends nothing. A cloud assistant such as Claude sends what it reads to its provider under that provider's retention settings, and Dory cannot see or limit what happens after that.

## Turn it on and add an assistant

1. Turn access on. The command prints the sentence above first.

   ```bash
   dory-cli assistants on
   ```

2. Add an assistant with the level it may read. There is no default level.

   ```bash
   dory-cli assistants add "Claude Code" --exposure summaries
   ```

   The levels are these.

   | Level | The assistant can read |
   | --- | --- |
   | `titles` | Each meeting's id, title, date, duration, and language. Search matches titles only. |
   | `summaries` | Adds summaries, speaker names, and action items. Search also matches summaries. |
   | `transcripts` | Adds full transcripts. Search also matches what was said. |

3. Copy the key the command prints. It starts with `dk_` and is shown once. Dory keeps only a digest of it, so if you lose it, remove the assistant and add it again.

## Connect Claude Code

Run this once, with your key in place of `<key>`. `--env` has to come before `--scope`, or Claude Code reads the key as the server's name.

```bash
claude mcp add --env DORY_MCP_KEY=<key> --scope user dory -- /Applications/Dory.app/Contents/MacOS/dory-mcp
```

Check it with `claude mcp list`, which should show `dory` as connected. Then ask Claude Code things like "what did we decide about the budget in last week's sync?".

## Connect Claude Desktop

Open Claude Desktop's settings, go to Developer, and choose Edit Config. That opens `~/Library/Application Support/Claude/claude_desktop_config.json`. Add a `dory` entry under `mcpServers`, keeping any servers already there.

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

## What an assistant can do

| Tool | What it returns |
| --- | --- |
| `search_recordings` | Meetings matching words, newest first, with a short snippet around the match. Takes `query`, `since` and `until` as UTC days (`YYYY-MM-DD`), `limit` up to 25, and a `cursor` for the next page. |
| `get_recording` | One meeting's title, date, duration, whether it has a transcript and summary, and its speakers at the `summaries` level. |
| `get_summary` | One meeting's summary: key points, decisions, and action items. Needs `summaries`. |
| `list_action_items` | The action items of one meeting, or of recent meetings since a day. Needs `summaries`. |
| `get_transcript` | One meeting's transcript as speaker turns with times, a page at a time. Needs `transcripts`. |

Every tool only reads. Meeting text comes back inside a marked block that tells the assistant it is recorded speech to treat as data, never as instructions, because anyone in a meeting can say something that sounds like an instruction.

## Change, hide, and review

- Change an assistant's level with `dory-cli assistants exposure "Claude Code" transcripts`. It applies on the assistant's next request.
- Remove an assistant with `dory-cli assistants remove "Claude Code"`. Its key stops working on its next request.
- Hide a recording from every assistant with `dory-cli private <id> on`, and show it again with `off`. A hidden recording answers exactly like one that does not exist. You still see it in Dory.
- Turn access off with `dory-cli assistants off`.
- See every read with `dory-cli assistants activity`, newest first, for the last 30 days, or `--days N`. Each line says which assistant read what and when, never the meeting text. Dory deletes activity older than 30 days each time it opens, and `dory-cli assistants prune` does it on demand.
- List assistants and whether access is on with `dory-cli assistants list`.

Changing access needs Dory's data folder to itself, so quit Dory first. `list` and `activity` work while Dory runs.

## When something is wrong

The helper answers every request, even when it cannot read, with a sentence the assistant can pass on. "Assistant access to Dory is off" means access is off. "Dory does not know this assistant's key" means the key was removed or mistyped. "Open Dory to update this library" means Dory was updated and has not opened since. When Dory updates while an assistant is connected, the helper exits after its next request so the assistant restarts it from the new version.

For tests and development, set `DORY_DATA_DIR` to a throwaway folder in the assistant's config `env`, next to the key, and the helper reads that folder instead of your library.
