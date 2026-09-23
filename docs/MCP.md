# Diskray for coding agents (MCP)

`diskray mcp` is a [Model Context Protocol](https://modelcontextprotocol.io)
server over stdio. It gives an agent the same measured, read-only tools the
on-device model uses, so the agent answers "why is my disk full?" from real
measurements instead of improvising shell commands.

Nothing in the MCP server can delete, move, or signal anything. The one tool
that isn't purely read-only, `propose_cleanup`, only saves a proposal. You
review it, and every target is measured and checked again, in `diskray review`.

## Set up

Claude Code:

```bash
claude mcp add --scope user diskray -- diskray mcp
```

Cursor (`~/.cursor/mcp.json`) and other clients that use the common JSON format:

```json
{
  "mcpServers": {
    "diskray": { "command": "diskray", "args": ["mcp"] }
  }
}
```

Codex (`~/.codex/config.toml`):

```toml
[mcp_servers.diskray]
command = "diskray"
args = ["mcp"]
```

By default the server measures the startup volume and accepts paths inside your
home folder. `diskray mcp --root /Volumes/Work` measures that volume instead
and also accepts paths inside it.

## Tools

| Tool | What it returns |
| --- | --- |
| `disk_overview` | Capacity, used and free space, space the folder walk cannot see, local snapshots, largest folders, ready quick wins |
| `findings` | Measured findings with status, tradeoff, and whether each can be proposed |
| `list_children` | The largest items in a folder, with handles (`n2`) for subfolders |
| `folder_age` | How much of a folder changed recently versus long ago |
| `open_handles` | Processes holding files open inside a folder |
| `identify_owner` | The app or tool that most likely owns a folder, and when it was last opened |
| `cleanup_rules` | Whether a cleanup rule covers a folder, its status, and its tradeoff |
| `growth` | What grew or shrank since an earlier complete assessment |
| `top_processes` | The processes using the most CPU or memory |
| `memory_state` | Memory pressure, swap activity, and the largest memory users |
| `process_details` | One process's identity, state, memory, and parent (never its arguments) |
| `stale_artifacts` | Build output (`node_modules`, `target`, `.venv`, …) in projects untouched for 60+ days; report only |
| `propose_cleanup` | Saves up to 10 eligible cleanup-rule targets for your review; deletes nothing |

The first tool call starts a full assessment in the background. Until it
finishes (usually a minute or two for a startup volume), tools other than
`stale_artifacts` say so and ask
the agent to try again. The result is cached for five minutes; call
`disk_overview` with `refresh: true` to start over.

## Safety model

- **Paths.** The agent may name a folder with an absolute path, `~/path`, or a
  handle from an earlier result. Paths containing `..`, paths through symlinks,
  and paths outside your home folder (or `--root`) are refused.
- **Private folders.** For Keychains, Messages, Mail, Cookies, Safari, the TCC
  database, `~/.ssh`, and `~/.gnupg`, only an aggregate size is reported.
- **Proposals, never actions.** `propose_cleanup` accepts only targets of
  cleanup rules that are ready or optional right now. Review-only data,
  in-use caches, and process signals cannot be proposed. The proposal is saved
  privately (mode 0600) and expires after 24 hours.
- **`diskray review`** opens the normal interface, re-measures everything, adds
  only the actions that are still eligible, and waits for the usual typed
  confirmation.
- **Where the data goes.** Tool results are sent to your agent's model, which
  may run in the cloud. The on-device Apple Intelligence features in the
  `diskray` app never leave your Mac; the MCP server is for agents you choose to
  connect.

## Protocol

The server is dual-era. It answers the legacy `initialize` handshake
(2025-11-25, 2025-06-18, 2025-03-26) and modern per-request versioning
(2026-07-28), including `server/discover`, `resultType`, and cache hints on
`tools/list`. An unsupported version gets error `-32022` with the supported
list. Stdout carries protocol messages only, and requests larger than 1 MiB
are rejected.

`python3 scripts/check_mcp.py target/release/diskray` exercises both eras,
the tools, path escapes, and malformed input against a disposable fixture.
