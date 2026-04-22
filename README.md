# git-workflow-mcp

MCP server providing session-guarded git worktree management for AI agent pipelines.

## Overview

`git-workflow-mcp` is a [Model Context Protocol (MCP)](https://modelcontextprotocol.io/) server that exposes git operations as tools. It introduces **session-based ownership** — each MCP session gets a unique ID, and destructive operations (worktree removal, branch deletion, merge) are only allowed on resources created by the same session.

## Tools

| Tool | Description | Session required |
|---|---|---|
| `session_start` | Initialize session with a git repository root | — |
| `worktree_add` | Create a worktree under `.worktrees/` with a new branch | Yes |
| `worktree_remove` | Remove a worktree (owner session only) | Yes |
| `worktree_list` | List all worktrees with ownership info | Yes |
| `branch_delete` | Delete a merged branch (owner session only) | Yes |
| `merge` | Merge a branch into target (owner session only) | Yes |
| `session_release` | Release session ownership of an orphan worktree | Yes |
| `commit` | Stage all changes and commit | No |
| `status` | Show git status | No |
| `diff` | Show git diff (stat + patch) | No |
| `log` | Show git log | No |

## Workflow

```
session_start → worktree_add → (work) → commit → merge → worktree_remove → branch_delete
```

## Orphan Worktree Recovery

When an MCP session ends unexpectedly (e.g. a crash or timeout), the worktree it created remains registered under the original session ID. A new session cannot run `merge`, `worktree_remove`, or `branch_delete` on that worktree because session ownership does not match.

Use `session_release` to remove the ownership entry and unblock the new session:

```
# New session
session_start(repo_root)
session_release(name: "<worktree-name>")   # drops orphan ownership entry
worktree_remove / merge / branch_delete    # now succeeds
```

`session_release` is idempotent — calling it for an already-released or non-existent name always succeeds.

## Installation

```bash
cargo install --path .
```

## Configuration

Add to your MCP client configuration (e.g. Claude Code `settings.json`):

```json
{
  "mcpServers": {
    "git-workflow": {
      "command": "git-workflow-mcp",
      "args": ["--stdio"]
    }
  }
}
```

## Observability

The server writes structured logs to both stderr and a rolling file, useful for diagnosing disconnects or hangs after the fact.

- **Logs**: written to `$GIT_WORKFLOW_LOG_DIR` (default `~/.cache/git-workflow-mcp/`) as `mcp.YYYY-MM-DD.log` (daily rotation).
- **Panic backtrace**: on any thread panic, the backtrace is appended to `panic.log` in the same directory.
- **Log level**: `--log-level` CLI arg > `GIT_WORKFLOW_LOG_LEVEL` env > `RUST_LOG` env > `warn` default. Accepts [`EnvFilter`](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html) syntax (e.g. `git_workflow_mcp=debug`).
- **Heartbeat**: an `alive pid=... sid=... elapsed_s=...` info line every 30 seconds, so the last-alive timestamp is always within half a minute of a crash.
- **Shutdown reason**: on exit a single `shutting down reason=<kind>` line is emitted. Kinds: `stdin_eof` (transport closed normally), `service_error` (rmcp returned error), `sigterm`, `sigpipe`, `ctrl_c`.

## License

Licensed under either of

- [MIT License](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

at your option.
