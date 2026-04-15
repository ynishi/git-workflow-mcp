mod tools;

use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::tool::ToolRouter,
    model::{
        CallToolRequestParams, CallToolResult, Implementation, ListToolsResult,
        PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerInfo,
    },
    service::{RequestContext, RoleServer},
    transport::stdio,
};

use crate::domain::session::SessionId;
use crate::infra::session_store::SessionStore;

/// read-only モードでブロックするツール名
const WRITE_TOOLS: &[&str] = &[
    "commit",
    "merge",
    "session_release",
    "worktree_add",
    "worktree_remove",
    "branch_delete",
    "safe_reset",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ServerMode {
    #[default]
    Full,
    ReadOnly,
}

pub async fn run(mode: ServerMode) -> anyhow::Result<()> {
    let server = GitWorkflowServer::new(mode);
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

#[derive(Clone)]
pub(super) struct GitWorkflowServer {
    pub(super) repo_root: Arc<RwLock<Option<PathBuf>>>,
    pub(super) session_id: SessionId,
    tool_router: ToolRouter<Self>,
    mode: ServerMode,
}

impl GitWorkflowServer {
    fn new(mode: ServerMode) -> Self {
        Self {
            repo_root: Arc::new(RwLock::new(None)),
            session_id: SessionId::new(),
            tool_router: Self::tool_router(),
            mode,
        }
    }

    pub(super) async fn repo_root(&self) -> Result<PathBuf, McpError> {
        self.repo_root.read().await.clone().ok_or_else(|| {
            McpError::internal_error(
                "session not started: call session_start(repo_root) first",
                None,
            )
        })
    }

    pub(super) async fn session_store(&self) -> Result<SessionStore, McpError> {
        let root = self.repo_root().await?;
        Ok(SessionStore::new(&root))
    }

    pub(super) fn to_mcp_error(e: crate::domain::error::DomainError) -> McpError {
        McpError::internal_error(format!("{e}"), None)
    }
}

impl ServerHandler for GitWorkflowServer {
    fn get_info(&self) -> ServerInfo {
        let base_instructions = "Git workflow operations for agent pipelines.\n\
             \n\
             IMPORTANT: Call session_start(repo_root) first before using any \
             repository-scoped tools (worktree_*, branch_delete, merge).\n\
             \n\
             Session-based safety: each MCP session gets a unique ID. \
             Destructive operations (worktree_remove, branch_delete, merge) \
             only work on worktrees created by the same session.\n\
             \n\
             Workflow: session_start → worktree_add → (work) → commit → merge → worktree_remove → branch_delete\n\
             \n\
             session_release: take ownership of an orphan worktree from a previous session";

        let instructions = if self.mode == ServerMode::ReadOnly {
            format!(
                "{base_instructions}\n\n\
                 NOTE: This server is running in read-only mode. Write operations are disabled."
            )
        } else {
            base_instructions.to_string()
        };

        ServerInfo {
            protocol_version: ProtocolVersion::V_2025_03_26,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "git-workflow-mcp".to_string(),
                title: Some("Git Workflow MCP — Worktree & Merge Operations".to_string()),
                description: Some(
                    "Session-guarded git workflow operations: worktree management, \
                     commit, merge, branch cleanup. Prevents cross-session destructive actions."
                        .to_string(),
                ),
                version: env!("CARGO_PKG_VERSION").to_string(),
                icons: None,
                website_url: None,
            },
            instructions: Some(instructions),
        }
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let mut tools = self.tool_router.list_all();
        if self.mode == ServerMode::ReadOnly {
            tools.retain(|t| !WRITE_TOOLS.contains(&t.name.as_ref()));
        }
        Ok(ListToolsResult {
            tools,
            next_cursor: None,
            meta: None,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        if self.mode == ServerMode::ReadOnly && WRITE_TOOLS.contains(&request.name.as_ref()) {
            return Err(McpError::invalid_params(
                format!("tool '{}' is not available in read-only mode", request.name),
                None,
            ));
        }
        let tool_ctx = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        self.tool_router.call(tool_ctx).await
    }
}
