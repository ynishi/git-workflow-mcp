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
    /// Show staged changes only (default: false)
    staged: Option<bool>,
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

// ─── Tools ───────────────────────────────────────────────

#[tool_router(vis = "pub(super)")]
impl GitWorkflowServer {
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
        let path =
            git::worktree_add(&repo_root, &req.name, &req.branch).map_err(Self::to_mcp_error)?;

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

        git::branch_delete(&repo_root, &req.branch).map_err(Self::to_mcp_error)?;

        Ok(CallToolResult::success(vec![rmcp::model::Content::text(
            format!("Branch '{}' deleted.", req.branch),
        )]))
    }

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

        let result =
            git::merge(&repo_root, &req.branch, &req.into_branch).map_err(Self::to_mcp_error)?;

        Ok(CallToolResult::success(vec![rmcp::model::Content::text(
            format!(
                "Merged '{}' into '{}'.\n{}",
                result.merged_branch, result.into_branch, result.summary
            ),
        )]))
    }

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

    #[tool(
        name = "diff",
        description = "Show git diff (stat + patch) for a working directory. Does not require session_start.",
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
        let working_dir = std::path::Path::new(&req.working_dir);
        let result =
            git::diff(working_dir, req.staged.unwrap_or(false)).map_err(Self::to_mcp_error)?;

        let output = if result.stat.is_empty() && result.diff.is_empty() {
            "No changes.".to_string()
        } else {
            format!("{}\n\n{}", result.stat, result.diff)
        };

        Ok(CallToolResult::success(vec![rmcp::model::Content::text(
            output,
        )]))
    }

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
