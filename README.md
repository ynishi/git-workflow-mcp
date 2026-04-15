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

## License

Licensed under either of

- [MIT License](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

at your option.
