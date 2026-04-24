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

/// read-remote モードでのみ利用可能なツール名 (read-only では除外)
const READ_REMOTE_ONLY_TOOLS: &[&str] = &["fetch", "remote_list"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ServerMode {
    #[default]
    Full,
    ReadOnly,
    ReadRemote,
}

/// Heartbeat 発火間隔 (秒)。debugging 目的で短めに固定。
const HEARTBEAT_INTERVAL_SECS: u64 = 30;

pub async fn run(mode: ServerMode) -> anyhow::Result<()> {
    let server = GitWorkflowServer::new(mode);
    let session_id = server.session_id.clone();

    // Heartbeat task: 起動直後に 1 発打ち、以降 30s interval で "alive" ログを出す。
    // shutdown 時は abort で殺す (クリーンアップ不要の単純 logging loop)。
    let heartbeat_handle = tokio::spawn(heartbeat_loop(session_id.clone()));

    let service = server.serve(stdio()).await?;

    // Shutdown reason を 5 種に分類して log 出力する。
    let (reason, err_detail) = wait_for_shutdown(service).await;

    heartbeat_handle.abort();

    match err_detail {
        Some(e) => tracing::info!(reason, err = %e, "shutting down"),
        None => tracing::info!(reason, "shutting down"),
    }

    Ok(())
}

/// 30 秒おきに `alive pid=N sid=...` を発火する heartbeat loop。
///
/// 初回は即時発火 (起動直後の last-alive timestamp 確保)、以降 interval tick ごと。
/// logging 経路の失敗は tracing が swallow する (appender buffer 溢れ等)。
async fn heartbeat_loop(session_id: SessionId) {
    let start = std::time::Instant::now();
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
    // interval の 1 発目は即時 fire。default がそうなっているがカラッと明示。
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;
        let elapsed_s = start.elapsed().as_secs();
        tracing::info!(
            pid = std::process::id(),
            sid = %session_id,
            elapsed_s,
            "alive"
        );
    }
}

/// Shutdown trigger を 5 branch で多重監視する。
///
/// 戻り値は `(reason, err_detail)`。err_detail は `service_error` 時のみ `Some`。
async fn wait_for_shutdown<S>(service: S) -> (&'static str, Option<String>)
where
    S: ServiceWaiting + Send + 'static,
{
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        // signal stream 作成失敗は startup 時点で panic しないよう Option 扱い。
        // 失敗時はその branch が発火しなくなるだけ (他 4 branch で拾える)。
        let mut sigterm = signal(SignalKind::terminate()).ok();
        let mut sigpipe = signal(SignalKind::pipe()).ok();

        let waiting = service.waiting();
        tokio::pin!(waiting);

        tokio::select! {
            r = &mut waiting => match r {
                Ok(()) => ("stdin_eof", None),
                Err(e) => ("service_error", Some(e)),
            },
            _ = tokio::signal::ctrl_c() => ("ctrl_c", None),
            _ = async {
                match sigterm.as_mut() {
                    Some(s) => { s.recv().await; }
                    None => std::future::pending::<()>().await,
                }
            } => ("sigterm", None),
            _ = async {
                match sigpipe.as_mut() {
                    Some(s) => { s.recv().await; }
                    None => std::future::pending::<()>().await,
                }
            } => ("sigpipe", None),
        }
    }

    #[cfg(not(unix))]
    {
        // 非 Unix (Windows) では SIGTERM/SIGPIPE 相当が無いので ctrl_c と waiting のみ。
        let waiting = service.waiting();
        tokio::pin!(waiting);

        tokio::select! {
            r = &mut waiting => match r {
                Ok(()) => ("stdin_eof", None),
                Err(e) => ("service_error", Some(e)),
            },
            _ = tokio::signal::ctrl_c() => ("ctrl_c", None),
        }
    }
}

/// `service.waiting()` を抽象化する trait。rmcp の具体型に直接依存せず test 可能にする。
trait ServiceWaiting {
    fn waiting(self) -> impl std::future::Future<Output = Result<(), String>> + Send;
}

impl<S> ServiceWaiting for rmcp::service::RunningService<RoleServer, S>
where
    S: ServerHandler,
{
    async fn waiting(self) -> Result<(), String> {
        // rmcp 0.15: `waiting()` は `QuitReason` を返す。Ok は正常終了 (EOF 等)、
        // 内部 error は Err(ServiceError) で返る。to_string でフラット化。
        match self.waiting().await {
            Ok(_) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
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

        let instructions = match self.mode {
            ServerMode::ReadOnly => format!(
                "{base_instructions}\n\n\
                 NOTE: This server is running in read-only mode. Write operations are disabled."
            ),
            ServerMode::ReadRemote => format!(
                "{base_instructions}\n\n\
                 NOTE: This server is running in read-remote mode. Write operations are disabled; \
                 fetch and remote_list are available for remote sync (no push)."
            ),
            ServerMode::Full => base_instructions.to_string(),
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
        match self.mode {
            ServerMode::ReadOnly => {
                tools.retain(|t| {
                    !WRITE_TOOLS.contains(&t.name.as_ref())
                        && !READ_REMOTE_ONLY_TOOLS.contains(&t.name.as_ref())
                });
            }
            ServerMode::ReadRemote => {
                tools.retain(|t| !WRITE_TOOLS.contains(&t.name.as_ref()));
            }
            ServerMode::Full => {}
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
        use futures::FutureExt;
        use std::panic::AssertUnwindSafe;

        match self.mode {
            ServerMode::ReadOnly => {
                if WRITE_TOOLS.contains(&request.name.as_ref())
                    || READ_REMOTE_ONLY_TOOLS.contains(&request.name.as_ref())
                {
                    return Err(McpError::invalid_params(
                        format!("tool '{}' is not available in read-only mode", request.name),
                        None,
                    ));
                }
            }
            ServerMode::ReadRemote => {
                if WRITE_TOOLS.contains(&request.name.as_ref()) {
                    return Err(McpError::invalid_params(
                        format!(
                            "tool '{}' is not available in read-remote mode",
                            request.name
                        ),
                        None,
                    ));
                }
            }
            ServerMode::Full => {}
        }
        let tool_name = request.name.to_string();
        let tool_ctx = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);

        // tool handler 本体を catch_unwind で囲む。単一 handler の panic で stdio transport を
        // 死なせず、McpError::internal_error として client に返す。panic message は best-effort
        // 抽出 (&str / String) する。
        let result = AssertUnwindSafe(self.tool_router.call(tool_ctx))
            .catch_unwind()
            .await;

        match result {
            Ok(r) => r,
            Err(panic) => {
                let msg = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "<non-string panic payload>".to_string());
                tracing::error!(
                    tool = %tool_name,
                    panic_msg = %msg,
                    "tool handler panicked"
                );
                Err(McpError::internal_error(
                    format!("tool '{tool_name}' panicked: {msg}"),
                    None,
                ))
            }
        }
    }
}
