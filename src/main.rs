mod domain;
mod infra;
mod interface;

use clap::Parser;

use infra::observability::{init_observability, resolve_log_dir, resolve_log_level};

#[derive(Parser)]
#[command(name = "git-workflow-mcp", version)]
struct Cli {
    /// Server mode: "full" (all tools) or "read-only" (read tools only)
    #[arg(long, default_value = "full")]
    mode: CliMode,

    /// Log level filter (e.g. "info", "debug", "git_workflow_mcp=trace").
    ///
    /// Priority: `--log-level` > `GIT_WORKFLOW_LOG_LEVEL` > `RUST_LOG` > `"warn"`.
    #[arg(long)]
    log_level: Option<String>,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum CliMode {
    Full,
    ReadOnly,
    ReadRemote,
}

impl From<CliMode> for interface::mcp::ServerMode {
    fn from(m: CliMode) -> Self {
        match m {
            CliMode::Full => interface::mcp::ServerMode::Full,
            CliMode::ReadOnly => interface::mcp::ServerMode::ReadOnly,
            CliMode::ReadRemote => interface::mcp::ServerMode::ReadRemote,
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let log_dir = resolve_log_dir();
    let level = resolve_log_level(cli.log_level.as_deref());

    // Observability 初期化。WorkerGuard を drop すると non_blocking writer が
    // flush 前に shutdown するため、`_guard` として main() scope 末尾まで保持する。
    let _guard = init_observability(&log_dir, &level)?;

    tracing::info!(
        log_dir = %log_dir.display(),
        level = %level,
        pid = %std::process::id(),
        "git-workflow-mcp starting"
    );

    interface::mcp::run(cli.mode.into()).await
}
