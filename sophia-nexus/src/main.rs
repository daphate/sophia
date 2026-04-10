mod paths;
mod server;

use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // All logging to stderr — stdout is reserved for JSON-RPC
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let data_dir = paths::data_dir();
    tracing::info!("sophia-nexus starting, data_dir={}", data_dir.display());

    let server = server::SophiaNexus::new(data_dir);
    let service = server.serve(stdio()).await.inspect_err(|e| {
        tracing::error!("MCP serve error: {:?}", e);
    })?;

    service.waiting().await?;
    Ok(())
}
