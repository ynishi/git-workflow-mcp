mod domain;
mod infra;
mod interface;

use clap::Parser;

#[derive(Parser)]
#[command(name = "git-workflow-mcp", version)]
struct Cli {
    /// Server mode: "full" (all tools) or "read-only" (read tools only)
    #[arg(long, default_value = "full")]
    mode: CliMode,
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
    interface::mcp::run(cli.mode.into()).await
}
