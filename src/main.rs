mod auth;
mod cache;
mod config;
mod dav;
mod error;
mod ics;
mod server;

use rmcp::ServiceExt;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // All logging must go to stderr — stdout is reserved for MCP JSON-RPC
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let config = config::AppConfig::load()?;
    tracing::info!("loaded {} account(s)", config.accounts.len());

    let server = server::CalDavServer::new(&config);
    let transport = rmcp::transport::io::stdio();

    let service = server.serve(transport).await?;
    service.waiting().await?;

    Ok(())
}
