//! L.E.E.K gateway — M0 clean-room skeleton.
//!
//! A minimal HTTP + SQLite + SSE server. No agent loop, no LLM, no OAuth,
//! no tools — only the plumbing a session / message / event UI needs. The
//! "assistant" is a fixed echo so the end-to-end path is proven before M1
//! wires in the real agent loop.

mod api;
mod bus;
mod vault;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "leek", version, about = "L.E.E.K — Logic-Enhanced Equity Kernel")]
struct Cli {
    /// Vault SQLite path (per-user data store).
    #[arg(long, global = true, default_value = "./vault.db")]
    vault: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the gateway HTTP / SSE server.
    Serve {
        #[arg(long, default_value_t = 8964)]
        port: u16,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "leek_gateway=info,info".into()),
        )
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Serve { port } => serve(&cli.vault, port).await,
    }
}

async fn serve(vault_path: &Path, port: u16) -> Result<()> {
    let vault = vault::Vault::open(vault_path).await?;
    let state = api::AppState {
        pool: vault.pool,
        bus: bus::EventBus::new(),
    };

    let app = api::router(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;

    tracing::info!(%addr, vault = %vault_path.display(), "leek gateway listening");
    axum::serve(listener, app).await.context("axum serve")?;
    Ok(())
}
