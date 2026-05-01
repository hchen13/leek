mod agent;
mod api;
mod auth;
mod events;
mod llm;
mod vault;

use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use futures::StreamExt;

use auth::codex::CodexTokens;
use llm::LlmProvider;
use vault::Vault;

#[derive(Parser)]
#[command(name = "leek", version, about = "L.E.E.K — Logic-Enhanced Equity Kernel")]
struct Cli {
    /// Vault SQLite path (per-user data store)
    #[arg(long, global = true, default_value = "./vault.db")]
    vault: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 启动 gateway HTTP / SSE server
    Serve {
        #[arg(long, default_value_t = 8964)]
        port: u16,
    },

    /// LLM provider 认证管理
    Auth {
        #[command(subcommand)]
        provider: AuthProvider,
    },

    /// 一次性 chat 调用（开发期 verify codex_oauth provider 用）
    Chat {
        /// User prompt
        prompt: String,
        /// Model name
        #[arg(long, default_value = "gpt-5")]
        model: String,
    },
}

#[derive(Subcommand)]
enum AuthProvider {
    /// Codex OAuth (ChatGPT subscription)
    Codex {
        /// 从 ~/.codex/auth.json 一次性导入 token（之后 codex CLI 若 refresh 会让 leek 失效）
        #[arg(long)]
        import_from_codex_cli: bool,
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
        Command::Serve { port } => run_serve(&cli.vault, port).await,
        Command::Auth { provider } => match provider {
            AuthProvider::Codex {
                import_from_codex_cli,
            } => run_auth_codex(&cli.vault, import_from_codex_cli).await,
        },
        Command::Chat { prompt, model } => run_chat(&cli.vault, prompt, model).await,
    }
}

async fn run_serve(vault_path: &Path, port: u16) -> Result<()> {
    let vault = Vault::open(vault_path).await?;
    let provider: Arc<dyn LlmProvider> = Arc::new(llm::codex_oauth::CodexOauthProvider::new(
        vault.pool.clone(),
        vault::LOCAL_USER_ID,
    )?);
    let event_bus = events::EventBus::new();

    let state = api::AppState {
        pool: vault.pool.clone(),
        provider,
        event_bus,
        user_id: vault::LOCAL_USER_ID.to_string(),
        active_replies: std::sync::Arc::new(tokio::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
    };

    let app = api::router(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("binding {addr}"))?;

    tracing::info!(%addr, vault = %vault_path.display(), "leek serve listening");
    axum::serve(listener, app)
        .await
        .context("axum serve")?;
    Ok(())
}

async fn run_chat(vault_path: &Path, prompt: String, model: String) -> Result<()> {
    let vault = Vault::open(vault_path).await?;
    let provider =
        llm::codex_oauth::CodexOauthProvider::new(vault.pool.clone(), vault::LOCAL_USER_ID)?;

    let req = llm::ChatRequest {
        messages: vec![llm::ChatMessage {
            role: llm::Role::User,
            content: prompt,
        }],
        system: None,
        model,
        max_output_tokens: Some(2048),
    };

    let mut stream = provider.chat(req).await?;
    let mut total_chars: usize = 0;
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();

    while let Some(event) = stream.next().await {
        match event? {
            llm::LlmEvent::TextDelta { text } => {
                handle.write_all(text.as_bytes()).ok();
                handle.flush().ok();
                total_chars += text.chars().count();
            }
            llm::LlmEvent::Usage(u) => {
                eprintln!();
                eprintln!(
                    "\x1b[90m[usage] in={} out={} cache_read={}\x1b[0m",
                    u.input_tokens, u.output_tokens, u.cache_read_tokens
                );
            }
            llm::LlmEvent::MessageEnd { stop_reason } => {
                eprintln!(
                    "\x1b[90m[end] reason={:?} chars_streamed={}\x1b[0m",
                    stop_reason, total_chars
                );
            }
        }
    }
    Ok(())
}

async fn run_auth_codex(vault_path: &Path, import_from_codex_cli: bool) -> Result<()> {
    let vault = Vault::open(vault_path).await?;

    let tokens = if import_from_codex_cli {
        let imported = read_codex_cli_auth()?
            .ok_or_else(|| anyhow!("~/.codex/auth.json not found"))?;
        println!("Imported tokens from ~/.codex/auth.json");
        println!();
        println!("\x1b[93m⚠  WARNING\x1b[0m: codex CLI / VS Code 扩展接下来若 refresh 会让 leek 失效。");
        println!("    使用 leek 期间避免在终端跑 `codex` 或在 VS Code 用 codex 扩展。");
        println!();
        imported
    } else {
        println!("Signing in to OpenAI Codex...");
        println!("(leek runs its own device flow — won't interfere with codex CLI / VS Code)");
        auth::codex::device_flow_login().await?
    };

    vault::provider_configs::upsert_codex(&vault.pool, vault::LOCAL_USER_ID, &tokens).await?;

    println!();
    println!("\x1b[92m✓ Login successful\x1b[0m");
    println!("  vault:       {}", vault_path.display());
    println!("  expires_at:  {}", tokens.expires_at.to_rfc3339());
    println!(
        "  access:      {}…",
        &tokens.access_token[..tokens.access_token.len().min(20)]
    );
    println!(
        "  refresh:     {}…",
        &tokens.refresh_token[..tokens.refresh_token.len().min(20)]
    );

    Ok(())
}

/// Read tokens from `~/.codex/auth.json` (or `$CODEX_HOME/auth.json` if set).
/// Returns `Ok(None)` if file doesn't exist; bails on malformed / expired contents.
fn read_codex_cli_auth() -> Result<Option<CodexTokens>> {
    let codex_home: PathBuf = std::env::var("CODEX_HOME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".codex")))
        .ok_or_else(|| anyhow!("could not resolve CODEX_HOME nor ~/.codex/"))?;
    let path = codex_home.join("auth.json");
    if !path.is_file() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    let payload: serde_json::Value = serde_json::from_str(&content)
        .with_context(|| format!("parsing {}", path.display()))?;
    let tokens_obj = payload
        .get("tokens")
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow!("{}: missing 'tokens' object", path.display()))?;
    let access_token = tokens_obj
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("{}: missing access_token", path.display()))?
        .to_string();
    let refresh_token = tokens_obj
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("{}: missing refresh_token", path.display()))?
        .to_string();

    let expires_at = auth::jwt::decode_exp(&access_token)
        .context("decoding exp from imported access_token")?;

    if expires_at < chrono::Utc::now() {
        bail!(
            "imported codex CLI access_token already expired ({}). \
             Run `codex` to refresh, then retry.",
            expires_at.to_rfc3339()
        );
    }

    Ok(Some(CodexTokens {
        access_token,
        refresh_token,
        expires_at,
    }))
}
