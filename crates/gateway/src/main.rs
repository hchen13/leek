//! L.E.E.K gateway — M1: agent loop MVP.
//!
//! An HTTP + SQLite + SSE server fronting a real agent loop on the codex
//! backend. A posted message runs the model–tool cycle (M0's echo worker is
//! gone), bounded by the M1 guard set, with per-turn metrics recorded.

mod agent;
mod api;
mod bus;
mod llm;
mod vault;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "leek",
    version,
    about = "L.E.E.K — Logic-Enhanced Equity Kernel"
)]
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
    /// Manage codex authentication.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
}

#[derive(Subcommand)]
enum AuthCommand {
    /// Authenticate via the codex device-authorization flow (interactive).
    Login,
    /// Import the codex CLI's current token from `~/.codex/auth.json`.
    Import,
    /// Show the stored codex token's status.
    Status,
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
        Command::Auth { command } => run_auth(&cli.vault, command).await,
    }
}

async fn serve(vault_path: &Path, port: u16) -> Result<()> {
    let vault = vault::Vault::open(vault_path).await?;

    let codex = llm::codex::CodexClient::new(vault.pool.clone(), vault::LOCAL_USER)?;
    let http = reqwest::Client::builder()
        .user_agent("leek-gateway/0.1")
        .connect_timeout(Duration::from_secs(15))
        .build()
        .context("building the shared HTTP client")?;
    let guards = agent::GuardConfig::from_env();

    let state = api::AppState {
        pool: vault.pool,
        bus: bus::EventBus::new(),
        codex,
        http,
        guards,
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

async fn run_auth(vault_path: &Path, command: AuthCommand) -> Result<()> {
    let vault = vault::Vault::open(vault_path).await?;

    match command {
        AuthCommand::Login => {
            let tokens = llm::oauth::device_flow_login().await?;
            store_tokens(&vault.pool, &tokens).await?;
            println!(
                "\n✓ codex authenticated via device flow (token expires {}).",
                tokens.expires_at.to_rfc3339()
            );
        }
        AuthCommand::Import => {
            let tokens = llm::oauth::import_from_codex_cli()?;
            store_tokens(&vault.pool, &tokens).await?;
            let path = llm::oauth::codex_cli_auth_path()?;
            println!("✓ imported codex token from {}.", path.display());
            println!(
                "  account: {}",
                tokens.account_id.as_deref().unwrap_or("(unknown)")
            );
            println!("  expires: {}", tokens.expires_at.to_rfc3339());
            println!();
            println!("  caveat: leek and the codex CLI now share one refresh token.");
            println!("  Whichever process refreshes first may invalidate the other's");
            println!("  copy. For a standalone setup, prefer `leek auth login`.");
        }
        AuthCommand::Status => {
            let codex = llm::codex::CodexClient::new(vault.pool.clone(), vault::LOCAL_USER)?;
            match codex.token_status().await? {
                None => {
                    println!("codex: not authenticated.");
                    println!("Run `leek auth login` (device flow) or `leek auth import`.");
                }
                Some(s) => {
                    println!("codex: authenticated.");
                    println!(
                        "  account: {}",
                        s.account_id.as_deref().unwrap_or("(unknown)")
                    );
                    let validity = if s.expired {
                        "EXPIRED — refreshes on next use"
                    } else {
                        "valid"
                    };
                    println!("  expires: {} ({validity})", s.expires_at);
                    println!("  updated: {}", s.updated_at);
                }
            }
        }
    }
    Ok(())
}

async fn store_tokens(pool: &sqlx::SqlitePool, tokens: &llm::oauth::CodexTokens) -> Result<()> {
    vault::auth_tokens::upsert(
        pool,
        vault::LOCAL_USER,
        &tokens.access_token,
        &tokens.refresh_token,
        tokens.account_id.as_deref(),
        &tokens.expires_at.to_rfc3339(),
    )
    .await
}
