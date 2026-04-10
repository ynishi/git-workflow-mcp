mod domain;
mod infra;
mod interface;

use clap::Parser;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "git-workflow-mcp", version)]
struct Cli {
    /// Server mode: "full" (all tools) or "read-only" (read tools only)
    #[arg(long, default_value = "full")]
    mode: CliMode,

    /// Log level filter (e.g. "info", "debug", "git_workflow_mcp=trace")
    #[arg(long, env = "RUST_LOG", default_value = "warn")]
    log_level: String,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum CliMode {
    Full,
    ReadOnly,
}

impl From<CliMode> for interface::mcp::ServerMode {
    fn from(m: CliMode) -> Self {
        match m {
            CliMode::Full => interface::mcp::ServerMode::Full,
            CliMode::ReadOnly => interface::mcp::ServerMode::ReadOnly,
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(&cli.log_level))
        .with_writer(std::io::stderr)
        .init();

    interface::mcp::run(cli.mode.into()).await
}
