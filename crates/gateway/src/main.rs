mod agent;
mod api;
mod auth;
mod corpus;
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
#[command(
    name = "leek",
    version,
    about = "L.E.E.K — Logic-Enhanced Equity Kernel"
)]
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

    /// Corpus 工具
    Corpus {
        #[command(subcommand)]
        action: CorpusAction,
    },
}

#[derive(Subcommand)]
enum CorpusAction {
    /// 扫描 corpus/ 生成 corpus.graph.json
    RebuildGraph {
        /// corpus 仓库根目录
        #[arg(long, default_value = "./corpus")]
        root: PathBuf,
        /// 输出 JSON 文件路径
        #[arg(long, default_value = "./crates/gateway/assets/corpus.graph.json")]
        output: PathBuf,
    },

    /// 把 corpus/wikis/principles 蒸馏成 system-prompt 可嵌入的单一 markdown blob
    Distill {
        /// corpus 仓库根目录
        #[arg(long, default_value = "./corpus")]
        root: PathBuf,
        /// 输出 markdown 文件路径
        #[arg(long, default_value = "./crates/gateway/assets/corpus_distilled.md")]
        output: PathBuf,
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
    // Load .env from the directory containing the binary, then the CWD,
    // then the workspace root — whichever exists first wins per-variable.
    for candidate in [
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join(".env"))),
        std::env::current_dir().ok().map(|d| d.join(".env")),
    ]
    .into_iter()
    .flatten()
    {
        if let Ok(content) = std::fs::read_to_string(&candidate) {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((k, v)) = line.split_once('=') {
                    let k = k.trim();
                    let v = v.trim().trim_matches('"').trim_matches('\'');
                    if std::env::var(k).is_err() {
                        std::env::set_var(k, v);
                    }
                }
            }
            break;
        }
    }

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
        Command::Corpus { action } => match action {
            CorpusAction::RebuildGraph { root, output } => run_corpus_rebuild(&root, &output),
            CorpusAction::Distill { root, output } => run_corpus_distill(&root, &output),
        },
    }
}

fn run_corpus_distill(root: &Path, output: &Path) -> Result<()> {
    let (blob, report) = corpus::distill::distill(root)?;
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(output, &blob).with_context(|| format!("writing {}", output.display()))?;
    println!(
        "\x1b[92m✓\x1b[0m distilled {} pages → {} ({} bytes)",
        report.pages_in,
        output.display(),
        report.bytes_out
    );
    println!("    input_hash {}", &report.input_hash[..16]);
    Ok(())
}

fn run_corpus_rebuild(root: &Path, output: &Path) -> Result<()> {
    let graph = corpus::build::build_graph(root)?;
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(&graph).context("serializing corpus graph")?;
    std::fs::write(output, json).with_context(|| format!("writing {}", output.display()))?;

    let mut by_cluster: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for n in &graph.nodes {
        *by_cluster.entry(n.cluster.as_str()).or_insert(0) += 1;
    }

    println!(
        "\x1b[92m✓\x1b[0m {} nodes · {} edges → {}",
        graph.nodes.len(),
        graph.edges.len(),
        output.display()
    );
    let mut keys: Vec<&&str> = by_cluster.keys().collect();
    keys.sort();
    for k in keys {
        println!("    · {:<22} {:>3}", k, by_cluster[k]);
    }
    if let Some(commit) = &graph.corpus_commit {
        println!("    corpus@{}", &commit[..commit.len().min(12)]);
    }
    Ok(())
}

async fn run_serve(vault_path: &Path, port: u16) -> Result<()> {
    let vault = Vault::open(vault_path).await?;
    let provider: Arc<dyn LlmProvider> = Arc::new(llm::codex_oauth::CodexOauthProvider::new(
        vault.pool.clone(),
        vault::LOCAL_USER_ID,
    )?);
    let event_bus = events::EventBus::new();

    match agent::harness::corpus_prompt_status() {
        agent::harness::CorpusPromptStatus::Loaded { path, bytes } => {
            tracing::info!(path = %path.display(), bytes, "corpus prompt loaded");
        }
        agent::harness::CorpusPromptStatus::Placeholder { path } => {
            tracing::warn!(
                path = %path.display(),
                "corpus prompt is a placeholder — system prompt will run without the \
                 distilled principles kernel. Run: `leek corpus distill --root ./corpus`"
            );
        }
        agent::harness::CorpusPromptStatus::Missing => {
            tracing::warn!(
                "corpus prompt file not found in any candidate path — system prompt will \
                 run without the distilled principles kernel. Run: `leek corpus distill --root ./corpus`"
            );
        }
    }

    // Phase 0 minimal toolset:
    //   - generic: ask_user_question, web_fetch, update_plan
    //   - corpus:  corpus_search, corpus_read
    //   - market:  tradingview_quote (= market_quote), get_candlesticks
    //   - A-share fundamentals: get_financials (Tushare for A-share, SEC for
    //                          US until that path is consolidated),
    //                          get_company_info, get_capital_flow
    // Removed in the rebuild slice: critic-driven decision_draft pipeline
    // (`record_investment_action`, `record_research_note`), 4-persona
    // subagent (`delegate_research`), skill-as-a-tool (`use_skill`,
    // replaced by static skill injection in build_system_prompt), US-only
    // filings tool (`sec_filing_fetch`), and crypto tools (`get_funding_rate`,
    // `get_crypto_market`) since the active vertical is A-shares.
    let tools = agent::tools::ToolRegistry::builder()
        .register(Arc::new(
            agent::tools::ask_user_question::AskUserQuestionTool::new(),
        ))
        .register(Arc::new(agent::tools::web_fetch::WebFetchTool::new()?))
        .register(Arc::new(agent::tools::update_plan::UpdatePlanTool::new()))
        .register(Arc::new(
            agent::tools::corpus_search::CorpusSearchTool::new(),
        ))
        .register(Arc::new(agent::tools::corpus_read::CorpusReadTool::new()))
        .register(Arc::new(
            agent::tools::tradingview_quote::TradingViewQuoteTool::new()?,
        ))
        .register(Arc::new(
            agent::tools::get_candlesticks::GetCandlesticksTool::new()?,
        ))
        .register(Arc::new(
            agent::tools::get_financials::GetFinancialsTool::new()?,
        ))
        .register(Arc::new(
            agent::tools::get_company_info::GetCompanyInfoTool::new()?,
        ))
        .register(Arc::new(
            agent::tools::get_capital_flow::GetCapitalFlowTool::new()?,
        ))
        .build();

    // mandates/<user_id>.md sits next to the vault sqlite. Users edit this
    // file directly (markdown over Obsidian / vim / etc); we re-read it on
    // every chat turn so edits take effect without restart.
    let mandate_path = vault_path.parent().map(|dir| {
        dir.join("mandates")
            .join(format!("{}.md", vault::LOCAL_USER_ID))
    });

    // Initial tuning: load persisted user_settings if any, otherwise defaults.
    let tuning_initial = vault::user_settings::load_tuning(&vault.pool, vault::LOCAL_USER_ID)
        .await
        .unwrap_or_else(|err| {
            tracing::warn!(?err, "failed to load user_settings; using built-in defaults");
            llm::LlmTuning::defaults()
        });

    let state = api::AppState {
        pool: vault.pool.clone(),
        provider,
        event_bus,
        user_id: vault::LOCAL_USER_ID.to_string(),
        active_replies: std::sync::Arc::new(tokio::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        tools,
        mandate_path,
        tuning: std::sync::Arc::new(std::sync::RwLock::new(tuning_initial)),
    };

    let app = api::router(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("binding {addr}"))?;

    tracing::info!(%addr, vault = %vault_path.display(), "leek serve listening");
    axum::serve(listener, app).await.context("axum serve")?;
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
        // Enable codex's built-in web_search so this CLI path doubles as a
        // smoke-test for the tool-on-request wiring (search/open_page events
        // print as `[web_search] ...` lines in stderr).
        tools: vec![llm::ToolSpec::WebSearch {
            external_web_access: true,
        }],
        additional_inputs: Vec::new(),
        reasoning_effort: None,
        verbosity: None,
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
            llm::LlmEvent::WebSearchCall { status, action } => {
                eprintln!("\x1b[90m[web_search] {status} {action:?}\x1b[0m");
            }
            llm::LlmEvent::FunctionCall {
                call_id,
                name,
                arguments,
            } => {
                eprintln!(
                    "\x1b[90m[function_call] id={call_id} name={name} args={}\x1b[0m",
                    arguments.chars().take(80).collect::<String>()
                );
            }
        }
    }
    Ok(())
}

async fn run_auth_codex(vault_path: &Path, import_from_codex_cli: bool) -> Result<()> {
    let vault = Vault::open(vault_path).await?;

    let tokens = if import_from_codex_cli {
        let imported =
            read_codex_cli_auth()?.ok_or_else(|| anyhow!("~/.codex/auth.json not found"))?;
        println!("Imported tokens from ~/.codex/auth.json");
        println!();
        println!(
            "\x1b[93m⚠  WARNING\x1b[0m: codex CLI / VS Code 扩展接下来若 refresh 会让 leek 失效。"
        );
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

    let content =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let payload: serde_json::Value =
        serde_json::from_str(&content).with_context(|| format!("parsing {}", path.display()))?;
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

    let expires_at =
        auth::jwt::decode_exp(&access_token).context("decoding exp from imported access_token")?;

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
