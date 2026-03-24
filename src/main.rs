mod domain;
mod infra;
mod interface;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    interface::mcp::run().await
}
