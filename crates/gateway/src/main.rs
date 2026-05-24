//! L.E.E.K gateway — M1: agent loop MVP.
//!
//! An HTTP + SQLite + SSE server fronting a real agent loop on the codex
//! backend. A posted message runs the model–tool cycle (M0's echo worker is
//! gone), bounded by the M1 guard set, with per-turn metrics recorded.

mod agent;
mod agents;
mod api;
mod bus;
mod config;
mod corpus;
mod hooks;
mod llm;
mod plugins;
mod skills;
mod vault;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
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

    /// Corpus root (M2.1). Resolution order: this flag → env
    /// `LEEK_CORPUS_DIR` → default `./corpus/`. A missing root is not a
    /// startup error — the gateway runs with an empty corpus instead.
    #[arg(long, global = true)]
    corpus: Option<PathBuf>,

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
    let corpus_root = resolve_corpus_root(cli.corpus.as_deref());
    match cli.command {
        Command::Serve { port } => serve(&cli.vault, &corpus_root, port).await,
        Command::Auth { command } => run_auth(&cli.vault, command).await,
    }
}

/// Resolve the corpus root: `--corpus <dir>` → `LEEK_CORPUS_DIR` →
/// default `./corpus/`. Returns the path without checking existence — a
/// missing path is handled downstream by `Corpus::load` (graceful empty).
fn resolve_corpus_root(cli_arg: Option<&Path>) -> PathBuf {
    if let Some(p) = cli_arg {
        return p.to_path_buf();
    }
    if let Ok(env) = std::env::var("LEEK_CORPUS_DIR") {
        if !env.is_empty() {
            return PathBuf::from(env);
        }
    }
    PathBuf::from("./corpus")
}

async fn serve(vault_path: &Path, corpus_root: &Path, port: u16) -> Result<()> {
    let vault = vault::Vault::open(vault_path).await?;

    let codex = llm::codex::CodexClient::new(vault.pool.clone(), vault::LOCAL_USER)?;
    let http = reqwest::Client::builder()
        .user_agent("leek-gateway/0.1")
        .connect_timeout(Duration::from_secs(15))
        .build()
        .context("building the shared HTTP client")?;
    // M2.6: load persisted user settings once at startup. The agent
    // re-runs `GuardConfig::resolve` at the start of every turn against
    // this `Arc<RwLock<Config>>`, so a `PATCH /api/v1/settings` reaches
    // the next turn without a restart.
    let cfg = config::Config::load();
    tracing::info!(
        path = %config::Config::path().display(),
        "loaded user settings"
    );
    let config = Arc::new(RwLock::new(cfg));

    // Provider-side web search (M1.9.4) is opt-in: the codex backend gates
    // web search behind its own config, so leek keeps the capability off
    // unless `LEEK_WEB_SEARCH` is set (ARCHITECTURE §12.3 — capability
    // opt-in).
    let web_search = matches!(
        std::env::var("LEEK_WEB_SEARCH").ok().as_deref(),
        Some("1" | "true" | "on" | "yes")
    );
    // M2.1: load the corpus once at startup. A missing root yields an
    // empty corpus + warn — the agent still serves generic answers.
    let corpus = corpus::Corpus::load(corpus_root)?;
    tracing::info!(
        docs = corpus.len(),
        root = %corpus_root.display(),
        "corpus loaded: {} docs from {}",
        corpus.len(),
        corpus_root.display()
    );
    // Build the Corpus Brain wiki graph (M2.1) — wiki nodes + shared-tag
    // edges (weight ≥ 2). One pass at startup; the corpus is not hot
    // reloaded, so the graph is fresh for the gateway's lifetime.
    let corpus_graph = corpus.build_graph();
    tracing::info!(
        nodes = corpus_graph.nodes.len(),
        edges = corpus_graph.edges.len(),
        "corpus brain graph built"
    );
    let corpus = Arc::new(corpus);
    let corpus_graph = Arc::new(corpus_graph);

    // ── M2.5: skills / hooks / plugins ─────────────────────────────────
    // Three skill layers (built-in / user / project) plus any skills
    // bundled by plugins (registered at the user layer). Hooks come from
    // the user config + any plugin hooks bundle. Failures are warn + skip
    // — a malformed skill or plugin must never block startup.
    let m25 = load_m25_runtime();
    tracing::info!(
        skills = m25.skill_count,
        plugins = m25.plugin_count,
        hook_events = m25.hook_event_count,
        "M2.5 runtime loaded: {} skills, {} plugins, hooks for {} events",
        m25.skill_count,
        m25.plugin_count,
        m25.hook_event_count
    );

    // ── M2.7: subagents ────────────────────────────────────────────────
    // Same three-layer discovery as skills, but a separate registry +
    // distinct AGENT.md files (ARCHITECTURE §4.2 — agents and skills
    // share frontmatter style only, never types).
    let agents_arc = load_agent_runtime();
    tracing::info!(
        agents = agents_arc.len(),
        "M2.7 runtime loaded: {} agents from {} / {} / .leek/agents",
        agents_arc.len(),
        agents::builtin_agents_dir().display(),
        agents::user_agents_dir().display(),
    );

    let state = api::AppState {
        pool: vault.pool,
        bus: bus::EventBus::new(),
        codex,
        http,
        config,
        web_search,
        corpus,
        corpus_graph,
        skills: m25.skills,
        hooks: m25.hooks,
        agents: agents_arc,
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

/// What the M2.5 loader produces at startup: a flat `Arc` per shared
/// runtime structure for the `AppState`.
struct M25Runtime {
    skills: Arc<skills::SkillRegistry>,
    hooks: Arc<hooks::HookEngine>,
    skill_count: usize,
    plugin_count: usize,
    hook_event_count: usize,
}

/// Resolve the three skill layers, load plugins from `~/.leek/plugins/`
/// (and `<cwd>/.leek/plugins/`), parse `~/.leek/config.json` hooks, then
/// merge everything into one registry + one hook engine.
///
/// Failures at any layer warn + skip — the gateway must boot even if the
/// user's plugin or config is malformed.
fn load_m25_runtime() -> M25Runtime {
    use skills::SkillLayer;

    // Plugins first: each plugin contributes a (skills dir, layer) slot
    // and a hooks file (loaded into the engine below).
    let plugin_roots = vec![
        skills::expand_home("~/.leek/plugins/"),
        std::path::PathBuf::from(".leek/plugins"),
    ];
    let (loaded_plugins, plugin_errs) = plugins::load_all(&plugin_roots);
    for e in &plugin_errs {
        tracing::warn!(error = %e, "plugin load error");
    }

    // Skill layers, low-priority first. Plugin skills register at the
    // user layer (after the user dir, so an explicit user override still
    // wins). The project layer comes last so it overrides everything.
    let builtin = skills::builtin_skills_dir();
    let user = skills::user_skills_dir();
    let project = std::path::PathBuf::from(".leek/skills");

    let mut layers: Vec<(&std::path::Path, SkillLayer)> = Vec::new();
    layers.push((builtin.as_path(), SkillLayer::Builtin));
    layers.push((user.as_path(), SkillLayer::User));
    for p in &loaded_plugins {
        if let Some(dir) = &p.skills_dir {
            layers.push((dir.as_path(), SkillLayer::User));
        }
    }
    layers.push((project.as_path(), SkillLayer::Project));

    let (skill_reg, skill_errs) = skills::load_layered(&layers);
    for e in &skill_errs {
        tracing::warn!(error = %e, "skill load error");
    }
    let skill_count = skill_reg.len();

    // Hooks: user config + each plugin's hooks bundle, in that order.
    let mut hook_set = hooks::HookSet::default();
    let user_config = skills::expand_home("~/.leek/config.json");
    match hooks::load_user_hooks(&user_config) {
        Ok(set) => hook_set.merge(set),
        Err(e) => tracing::warn!(error = %e, path = %user_config.display(), "user hook load error"),
    }
    for p in &loaded_plugins {
        if let Some(hooks_file) = &p.hooks_file {
            match hooks::load_hooks_file(hooks_file) {
                Ok(set) => hook_set.merge(set),
                Err(e) => tracing::warn!(error = %e, plugin = %p.name, "plugin hook load error"),
            }
        }
    }
    let hook_event_count = hook_set.event_count();
    let engine = hooks::HookEngine::new(hook_set);

    M25Runtime {
        skills: Arc::new(skill_reg),
        hooks: Arc::new(engine),
        skill_count,
        plugin_count: loaded_plugins.len(),
        hook_event_count,
    }
}

/// Resolve the three subagent layers and merge them into one
/// `AgentRegistry`. Failures at any layer warn + skip — the gateway must
/// boot even if a user's AGENT.md is malformed (M2.7 spec §C).
fn load_agent_runtime() -> Arc<agents::AgentRegistry> {
    use agents::AgentLayer;

    let builtin = agents::builtin_agents_dir();
    let user = agents::user_agents_dir();
    let project = std::path::PathBuf::from(".leek/agents");

    let layers: Vec<(&std::path::Path, AgentLayer)> = vec![
        (builtin.as_path(), AgentLayer::Builtin),
        (user.as_path(), AgentLayer::User),
        (project.as_path(), AgentLayer::Project),
    ];

    let (reg, errs) = agents::load_layered(&layers);
    for e in &errs {
        tracing::warn!(error = %e, "agent load error");
    }
    Arc::new(reg)
}
