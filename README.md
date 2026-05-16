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
| `branch_status` | Return ahead/behind counts and commit lists as typed fields (no string parsing required) | No |
| `unpushed_commits` | List commits on a local branch not yet on its remote tracking ref | No |
| `is_pushed` | Check whether a commit is reachable from any remote tracking ref | No |
| `tag_pushed` | Check whether a tag exists on a remote by querying remote refs directly | No |
| `reset_target` | Compute the target commit hash N steps back via first-parent walk (no actual reset) | No |
| `worktree_state` | Return a typed snapshot of clean/ahead/behind/tracking/uncommitted in one call | No |

## Modes

The server exposes different tool subsets depending on `--mode`:

| `--mode` | Exposed tools | Typical use |
|---|---|---|
| `full` (default) | All tools | Full worktree workflow (read + local write + remote) |
| `read-only` | `status`, `diff`, `log`, `worktree_list`, `session_start` | Local read-only inspection |
| `read-remote` | read-only tools + `fetch`, `remote_list` | Remote sync without local/remote write |

`read-remote` is a superset of `read-only`; `full` is a superset of `read-remote`.
No `push`, `clone`, or remote-configuration tools are exposed — those remain CLI-only.

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

## Structured Branch and Tag Inspection

Six read-only tools eliminate common AI git-parsing errors at BUMP/Publish moments.

### ahead/behind without direction errors

`branch_status` returns `ahead`, `behind`, and `up_to_date` as discrete typed integer fields using `git rev-list --left-right --count base...branch`. There is no formatted string for the caller to parse, so direction-misread errors ("1 ahead" vs "1 behind") are structurally impossible.

```
branch_status(working_dir, branch: "main", base: "origin/main")
→ { ahead: 1, behind: 0, up_to_date: false, ahead_commits: [...], behind_commits: [], common_ancestor: "abc123" }
```

`unpushed_commits` lists the commits absent from the remote tracking ref as a typed list. `is_pushed` checks whether a specific commit hash is reachable from any remote ref.

All three tools assume remote refs are already fetched. Call `fetch` first when the remote state may have changed.

### Tag remote visibility

`tag_pushed` queries the remote's tag refs directly via `git ls-remote --tags` and never inspects local tag metadata. This closes the gap where a tag created locally but not yet pushed appears "present" in local state.

```
tag_pushed(working_dir, tag: "v1.2.0", remote: "origin")
→ { pushed: true, remote_refs: ["refs/tags/v1.2.0"] }
```

### Rollback target without merge-commit boundary crossing

`reset_target` computes the target commit hash by walking the first-parent chain for N steps (`git log --first-parent`), never using `HEAD~N` arithmetic. `HEAD~N` silently crosses merge commits and lands on a second-parent commit, making the rollback target unreliable.

```
reset_target(working_dir, steps_back: 2, from: "HEAD")
→ { target_hash: "def456", target_subject: "feat: ...", linear: true }
```

`reset_target` only computes the hash — it does not execute an actual reset.

### Worktree state in one call

`worktree_state` combines `branch_status`, upstream tracking ref resolution, and uncommitted file count into a single typed response, giving a complete picture without multiple round-trips.

```
worktree_state(working_dir)
→ { clean: false, ahead: 1, behind: 0, tracking: "origin/main", uncommitted: 2 }
```

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
