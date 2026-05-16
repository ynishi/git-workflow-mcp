# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `branch_status` tool: compare a branch against a base ref and return `ahead`, `behind`, and `up_to_date` as discrete typed integer/boolean fields. Uses `git rev-list --left-right --count base...branch` to avoid direction-misread errors. Returns `ahead_commits` and `behind_commits` as typed `LogEntry` lists. Assumes remote refs are up-to-date (`fetch` first if needed).
- `unpushed_commits` tool: list commits present on a local branch but absent from its remote tracking ref. Returns `commits` (typed list), `count`, and `remote_head` as structured fields.
- `is_pushed` tool: check whether a specific commit is reachable from any remote tracking ref. Returns `pushed` (bool) and `refs` (list of remote ref names containing the commit).
- `tag_pushed` tool: check whether a tag is pushed to a remote by querying the remote's tag refs **directly** via `git ls-remote --tags` — never using local tag metadata. Returns `pushed` (bool) and `remote_refs`. Requires network access to remote.
- `reset_target` tool: compute the target commit hash N steps back from `from` using a strict first-parent walk (`git log --first-parent`), never `HEAD~N` arithmetic. Returns `target_hash`, `target_subject`, and `linear` (false if any merge commit was on the traversal path). Does not execute an actual reset.
- `worktree_state` tool: return a typed snapshot of a worktree's full state — `clean`, `ahead`, `behind`, `tracking`, and `uncommitted` — in a single call. Combines `branch_status`, upstream tracking ref resolution, and uncommitted file count.

## [0.1.2] - 2026-05-12

### Added

- `commit` tool: optional `paths` parameter for selective staging instead of `git add -A`.

## [0.1.1] - 2026-05-01

### Added

- `read-remote` server mode exposing `fetch` and `remote_list` tools for remote sync without local write operations.
- `session_release` tool for releasing orphan worktree ownership after an unexpected session end.

### Changed

- Observability retrofit: structured logs to rolling file, panic backtrace to `panic.log`, configurable log level, 30-second heartbeat, shutdown-reason line.

## [0.1.0] - 2026-04-01

### Added

- Initial release: session-guarded git worktree management via MCP.
- Tools: `session_start`, `worktree_add`, `worktree_remove`, `worktree_list`, `branch_delete`, `merge`, `commit`, `status`, `diff`, `log`.
- Three server modes: `full`, `read-only`, `read-remote`.
