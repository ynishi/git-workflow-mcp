use rmcp::{handler::server::wrapper::Parameters, model::CallToolResult, tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use super::GitWorkflowServer;
use crate::infra::git;

// ─── Request types ───────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
struct SessionStartRequest {
    /// Git repository root path (absolute path to the repo)
    repo_root: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct WorktreeAddRequest {
    /// Worktree name (used as directory name under .worktrees/)
    name: String,
    /// Branch name to create (e.g. "task/my-feature")
    branch: String,
    /// Base branch to branch from (e.g. "topic/foo"). If omitted, branches from repo HEAD.
    base_branch: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct WorktreeRemoveRequest {
    /// Worktree name to remove
    name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct BranchDeleteRequest {
    /// Branch name to delete
    branch: String,
    /// Working directory to run `git branch -d` in (e.g. a worktree path whose HEAD
    /// contains the branch's commits). If omitted, uses repo root. Required when the
    /// branch is only merged into a non-default HEAD such as a topic worktree, because
    /// `git branch -d`'s merge safety check is evaluated against the running directory's HEAD.
    working_dir: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CommitRequest {
    /// Working directory path (worktree or repo root)
    working_dir: String,
    /// Commit message
    message: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct MergeRequest {
    /// Branch to merge from
    branch: String,
    /// Branch to merge into (must be current branch)
    into_branch: String,
    /// Working directory to run merge in (e.g. a worktree path). If omitted, uses repo root.
    working_dir: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct StatusRequest {
    /// Working directory path (worktree or repo root)
    working_dir: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DiffRequest {
    /// Working directory path (worktree or repo root)
    working_dir: String,
    /// Show staged changes only (default: false). Cannot be used with commit_range.
    staged: Option<bool>,
    /// Commit range for comparison (e.g. "main..HEAD", "abc123..def456", "HEAD"). Cannot be used with staged.
    commit_range: Option<String>,
    /// Limit diff to specific file or directory paths
    paths: Option<Vec<String>>,
    /// Show only changed file names (no patch content)
    name_only: Option<bool>,
    /// Maximum number of diff output lines (truncates if exceeded)
    max_lines: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct LogRequest {
    /// Working directory path (worktree or repo root)
    working_dir: String,
    /// Commit range (e.g. "main..HEAD"). If omitted, shows recent commits.
    range: Option<String>,
    /// Maximum number of commits to show (default: 20)
    max_count: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SafeResetRequest {
    /// Working directory path (worktree or repo root)
    working_dir: String,
    /// Reset mode: "soft" (move HEAD only, keep staged and working tree) or "mixed" (move HEAD, unstage changes, keep working tree). Default: "mixed"
    mode: Option<String>,
    /// Target commit to reset to (e.g. "HEAD~1", "abc1234", "main"). Default: "HEAD~1"
    target: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SessionReleaseRequest {
    /// Worktree name to release ownership of
    name: String,
}

// ─── Tools ───────────────────────────────────────────────

#[tool_router(vis = "pub(super)")]
impl GitWorkflowServer {
    #[tracing::instrument(
        skip(self, req),
        fields(session_id = %self.session_id, tool = "session_start"),
        err
    )]
    #[tool(
        name = "session_start",
        description = "Initialize the session with a git repository root. Must be called before using any repository-scoped tools (worktree_*, branch_delete, merge). Returns the session ID.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn session_start(
        &self,
        Parameters(req): Parameters<SessionStartRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let path = std::path::PathBuf::from(&req.repo_root);
        if !path.join(".git").exists() && !path.join("HEAD").exists() {
            return Err(rmcp::ErrorData::internal_error(
                format!("not a git repository: {}", req.repo_root),
                None,
            ));
        }

        *self.repo_root.write().await = Some(path);

        Ok(CallToolResult::success(vec![rmcp::model::Content::text(
            format!(
                "Session started.\n- Session ID: {}\n- Repo root: {}",
                self.session_id, req.repo_root
            ),
        )]))
    }

    #[tracing::instrument(
        skip(self, req),
        fields(session_id = %self.session_id, tool = "worktree_add"),
        err
    )]
    #[tool(
        name = "worktree_add",
        description = "Create a new git worktree under .worktrees/ with a new branch. Requires session_start. Registers the worktree to this session for ownership tracking.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn worktree_add(
        &self,
        Parameters(req): Parameters<WorktreeAddRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let repo_root = self.repo_root().await?;
        let path = git::worktree_add(
            &repo_root,
            &req.name,
            &req.branch,
            req.base_branch.as_deref(),
        )
        .map_err(Self::to_mcp_error)?;

        let store = self.session_store().await?;
        store
            .register(&req.name, &req.branch, &self.session_id)
            .map_err(Self::to_mcp_error)?;

        Ok(CallToolResult::success(vec![rmcp::model::Content::text(
            format!(
                "Worktree created.\n- Path: {path}\n- Branch: {}\n- Session: {}",
                req.branch, self.session_id
            ),
        )]))
    }

    #[tracing::instrument(
        skip(self, req),
        fields(session_id = %self.session_id, tool = "worktree_remove"),
        err
    )]
    #[tool(
        name = "worktree_remove",
        description = "Remove a worktree. Only the session that created it can remove it. Requires session_start.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
    )]
    async fn worktree_remove(
        &self,
        Parameters(req): Parameters<WorktreeRemoveRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let repo_root = self.repo_root().await?;
        let store = self.session_store().await?;

        store
            .verify_owner(&req.name, &self.session_id)
            .map_err(Self::to_mcp_error)?;

        git::worktree_remove(&repo_root, &req.name).map_err(Self::to_mcp_error)?;

        store.unregister(&req.name).map_err(Self::to_mcp_error)?;

        Ok(CallToolResult::success(vec![rmcp::model::Content::text(
            format!("Worktree '{}' removed.", req.name),
        )]))
    }

    #[tracing::instrument(
        skip(self, req),
        fields(session_id = %self.session_id, tool = "session_release"),
        err
    )]
    #[tool(
        name = "session_release",
        description = "Release session ownership of a worktree, allowing another session to perform cleanup (merge / worktree_remove). Useful when the original session ended and a new session needs to complete cleanup. Requires session_start.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn session_release(
        &self,
        Parameters(req): Parameters<SessionReleaseRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let store = self.session_store().await?;

        store.unregister(&req.name).map_err(Self::to_mcp_error)?;

        Ok(CallToolResult::success(vec![rmcp::model::Content::text(
            format!("Session ownership released for worktree '{}'.", req.name),
        )]))
    }

    #[tracing::instrument(
        skip(self),
        fields(session_id = %self.session_id, tool = "worktree_list"),
        err
    )]
    #[tool(
        name = "worktree_list",
        description = "List all git worktrees with their session ownership info. Requires session_start.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn worktree_list(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        let repo_root = self.repo_root().await?;
        let store = self.session_store().await?;

        let worktrees = git::worktree_list(&repo_root).map_err(Self::to_mcp_error)?;
        let sessions = store.load().unwrap_or_default();

        let mut lines = Vec::new();
        for wt in &worktrees {
            if wt.is_bare {
                continue;
            }
            let branch = wt.branch.as_deref().unwrap_or("(detached)");
            let owner = sessions
                .iter()
                .find(|s| wt.path.ends_with(&s.worktree_name))
                .map(|s| {
                    let mine = if s.session_id == self.session_id {
                        " (this session)"
                    } else {
                        ""
                    };
                    format!("session:{}{mine}", s.session_id)
                })
                .unwrap_or_else(|| "untracked".to_string());
            lines.push(format!("- {} [{}] owner={}", wt.path, branch, owner));
        }

        let output = if lines.is_empty() {
            "No worktrees (besides main).".to_string()
        } else {
            lines.join("\n")
        };

        Ok(CallToolResult::success(vec![rmcp::model::Content::text(
            output,
        )]))
    }

    #[tracing::instrument(
        skip(self, req),
        fields(session_id = %self.session_id, tool = "branch_delete"),
        err
    )]
    #[tool(
        name = "branch_delete",
        description = "Delete a merged branch. Only allowed if the branch's worktree was created by this session. Requires session_start.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
    )]
    async fn branch_delete(
        &self,
        Parameters(req): Parameters<BranchDeleteRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let repo_root = self.repo_root().await?;
        let store = self.session_store().await?;

        let sessions = store.load().unwrap_or_default();
        if let Some(session) = sessions.iter().find(|s| s.branch == req.branch)
            && session.session_id != self.session_id
        {
            return Err(Self::to_mcp_error(
                crate::domain::error::DomainError::SessionMismatch {
                    worktree: session.worktree_name.clone(),
                    owner: session.session_id.to_string(),
                    caller: self.session_id.to_string(),
                },
            ));
        }

        let working_dir = req.working_dir.as_deref().map(std::path::Path::new);
        git::branch_delete(&repo_root, &req.branch, working_dir).map_err(Self::to_mcp_error)?;

        Ok(CallToolResult::success(vec![rmcp::model::Content::text(
            format!("Branch '{}' deleted.", req.branch),
        )]))
    }

    #[tracing::instrument(
        skip(self, req),
        fields(session_id = %self.session_id, tool = "commit"),
        err
    )]
    #[tool(
        name = "commit",
        description = "Stage all changes and create a commit in the specified working directory. Does not require session_start.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn commit(
        &self,
        Parameters(req): Parameters<CommitRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let working_dir = std::path::Path::new(&req.working_dir);
        let result = git::commit(working_dir, &req.message).map_err(Self::to_mcp_error)?;

        Ok(CallToolResult::success(vec![rmcp::model::Content::text(
            format!(
                "Committed.\n- Hash: {}\n- Message: {}\n- Files changed: {}",
                result.hash, result.message, result.files_changed
            ),
        )]))
    }

    #[tracing::instrument(
        skip(self, req),
        fields(session_id = %self.session_id, tool = "merge"),
        err
    )]
    #[tool(
        name = "merge",
        description = "Merge a branch into the target branch (must be current branch). Session-guarded. Requires session_start.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
    )]
    async fn merge(
        &self,
        Parameters(req): Parameters<MergeRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let repo_root = self.repo_root().await?;
        let store = self.session_store().await?;

        let sessions = store.load().unwrap_or_default();
        if let Some(session) = sessions.iter().find(|s| s.branch == req.branch)
            && session.session_id != self.session_id
        {
            return Err(Self::to_mcp_error(
                crate::domain::error::DomainError::SessionMismatch {
                    worktree: session.worktree_name.clone(),
                    owner: session.session_id.to_string(),
                    caller: self.session_id.to_string(),
                },
            ));
        }

        let working_dir = req.working_dir.as_deref().map(std::path::Path::new);
        let result = git::merge(&repo_root, &req.branch, &req.into_branch, working_dir)
            .map_err(Self::to_mcp_error)?;

        Ok(CallToolResult::success(vec![rmcp::model::Content::text(
            format!(
                "Merged '{}' into '{}'.\n{}",
                result.merged_branch, result.into_branch, result.summary
            ),
        )]))
    }

    #[tracing::instrument(
        skip(self, req),
        fields(session_id = %self.session_id, tool = "status"),
        err
    )]
    #[tool(
        name = "status",
        description = "Show git status (branch, changed files) for a working directory. Does not require session_start.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn status(
        &self,
        Parameters(req): Parameters<StatusRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let working_dir = std::path::Path::new(&req.working_dir);
        let result = git::status(working_dir).map_err(Self::to_mcp_error)?;

        let mut output = format!("Branch: {}\nClean: {}\n", result.branch, result.clean);
        if !result.changed_files.is_empty() {
            output.push_str(&format!(
                "{} changed file(s):\n",
                result.changed_files.len()
            ));
            for f in &result.changed_files {
                output.push_str(&format!("  {f}\n"));
            }
        }

        Ok(CallToolResult::success(vec![rmcp::model::Content::text(
            output,
        )]))
    }

    #[tracing::instrument(
        skip(self, req),
        fields(session_id = %self.session_id, tool = "diff"),
        err
    )]
    #[tool(
        name = "diff",
        description = "Show git diff for a working directory. Supports staged, commit range comparison, path filtering, name-only, and line limit. Does not require session_start.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn diff(
        &self,
        Parameters(req): Parameters<DiffRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        // staged と commit_range の排他チェック
        if req.staged.unwrap_or(false) && req.commit_range.is_some() {
            return Err(rmcp::ErrorData::internal_error(
                "staged and commit_range cannot be used together".to_string(),
                None,
            ));
        }

        let working_dir = std::path::Path::new(&req.working_dir);
        let opts = git::DiffOptions {
            staged: req.staged.unwrap_or(false),
            commit_range: req.commit_range.clone(),
            paths: req.paths.clone(),
            name_only: req.name_only.unwrap_or(false),
        };
        let result = git::diff(working_dir, &opts).map_err(Self::to_mcp_error)?;

        // max_lines truncation（diff フィールドのみに適用）
        let diff_text = if let Some(max_lines) = req.max_lines {
            let max_lines = max_lines as usize;
            let lines: Vec<&str> = result.diff.lines().collect();
            let total = lines.len();
            if total > max_lines {
                let shown = lines[..max_lines].join("\n");
                format!("{shown}\n... (truncated, showing {max_lines}/{total} lines)")
            } else {
                result.diff.clone()
            }
        } else {
            result.diff.clone()
        };

        let output = if result.stat.is_empty() && diff_text.is_empty() {
            "No changes.".to_string()
        } else if result.stat.is_empty() {
            diff_text
        } else {
            format!("{}\n\n{}", result.stat, diff_text)
        };

        Ok(CallToolResult::success(vec![rmcp::model::Content::text(
            output,
        )]))
    }

    #[tracing::instrument(
        skip(self, req),
        fields(session_id = %self.session_id, tool = "log"),
        err
    )]
    #[tool(
        name = "log",
        description = "Show git log (commit history) for a working directory. Does not require session_start.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn log(
        &self,
        Parameters(req): Parameters<LogRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let working_dir = std::path::Path::new(&req.working_dir);
        let max_count = req.max_count.unwrap_or(20);
        let entries =
            git::log(working_dir, req.range.as_deref(), max_count).map_err(Self::to_mcp_error)?;

        let output = if entries.is_empty() {
            "No commits.".to_string()
        } else {
            entries
                .iter()
                .map(|e| format!("{} {}", e.hash, e.message))
                .collect::<Vec<_>>()
                .join("\n")
        };

        Ok(CallToolResult::success(vec![rmcp::model::Content::text(
            output,
        )]))
    }

    #[tracing::instrument(
        skip(self, req),
        fields(session_id = %self.session_id, tool = "safe_reset"),
        err
    )]
    #[tool(
        name = "safe_reset",
        description = "Safely reset HEAD to a target commit. Only supports 'soft' (move HEAD, keep staged and working tree) and 'mixed' (move HEAD, unstage changes, keep working tree) modes. Hard reset is intentionally not supported. Returns the previous HEAD hash for recovery. Does not require session_start.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn safe_reset(
        &self,
        Parameters(req): Parameters<SafeResetRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let working_dir = std::path::Path::new(&req.working_dir);
        let mode = req.mode.as_deref().unwrap_or("mixed");
        let target = req.target.as_deref().unwrap_or("HEAD~1");

        let result = git::reset(working_dir, mode, target).map_err(Self::to_mcp_error)?;

        Ok(CallToolResult::success(vec![rmcp::model::Content::text(
            format!(
                "Reset completed ({}).\n- Previous HEAD: {}\n- New HEAD: {}\n- Target: {}\nTo undo: git reset --soft {}",
                result.mode, result.previous_head, result.new_head, target, result.previous_head
            ),
        )]))
    }
}
