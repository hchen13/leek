mod auth;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "leek", version, about = "L.E.E.K — Logic-Enhanced Equity Kernel")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 启动 gateway HTTP / SSE server
    Serve {
        #[arg(long, default_value_t = 8964)]
        port: u16,
        #[arg(long, default_value = "./vault.db")]
        vault: String,
    },

    /// LLM provider 认证管理
    Auth {
        #[command(subcommand)]
        provider: AuthProvider,
    },
}

#[derive(Subcommand)]
enum AuthProvider {
    /// Codex OAuth (ChatGPT subscription)
    Codex {
        /// 从 ~/.codex/auth.json 一次性导入 token（注意：之后 codex CLI 若 refresh 会让 leek 失效）
        #[arg(long)]
        import_from_codex_cli: bool,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "leek_gateway=info,info".into()),
        )
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Serve { port, vault } => {
            tracing::info!(port, vault, "leek serve (placeholder — Task #48)");
            anyhow::bail!("serve 还没接入，等待 gateway HTTP/SSE 切片完成");
        }
        Command::Auth { provider } => match provider {
            AuthProvider::Codex {
                import_from_codex_cli,
            } => run_auth_codex(import_from_codex_cli).await,
        },
    }
}

async fn run_auth_codex(import_from_codex_cli: bool) -> anyhow::Result<()> {
    if import_from_codex_cli {
        anyhow::bail!("--import-from-codex-cli 等 vault 持久化切片完成（Task #50）");
    }

    println!("Signing in to OpenAI Codex...");
    println!("(leek runs its own device flow — won't interfere with codex CLI / VS Code)");

    let tokens = auth::codex::device_flow_login().await?;

    println!();
    println!("\x1b[92m✓ Login successful\x1b[0m");
    println!("  expires at:    {}", tokens.expires_at.to_rfc3339());
    println!("  access_token:  {}…", &tokens.access_token[..tokens.access_token.len().min(20)]);
    println!("  refresh_token: {}…", &tokens.refresh_token[..tokens.refresh_token.len().min(20)]);
    println!();
    println!("\x1b[93mNote: token persistence to vault.db is wired up in next slice (Task #50).\x1b[0m");

    Ok(())
}
