//! The `bia` binary. `serve` and `migrate` touch Postgres directly; every other subcommand
//! is a client of the HTTP API (ADR-0005, ADR-0007).

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, bail};
use bia_client::{NewToken, Scope, WebhookBinding};
use bia_core::action::CancellationToken;
use bia_core::clock::Clock;
use bia_engine::{InternalSecrets, Interpreter, Retention, Scheduler, Worker};
use bia_server::{ApiState, SetupCode};
use bia_store::SystemClock;
use bia_store::secret::{PgSecretRepository, SecretRepository};
use bia_store::user::{PgUserRepository, UserRepository};
use clap::{Parser, Subcommand, ValueEnum};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tokio::net::TcpListener;

mod api;
mod client_config;
mod config;
mod mcp;
mod setup;

use client_config::ClientConfig;
use config::{Config, LogFormat};

#[derive(Debug, Parser)]
#[command(name = "bia", version, about = "Workflows as data, run from Postgres.")]
struct Cli {
    /// A TOML config file. Environment variables override what it sets.
    #[arg(long, global = true, env = "BIA_CONFIG")]
    config: Option<PathBuf>,
    /// Where the API is served, for the commands that call it. Defaults to `BIA_URL`, the
    /// config file's `url`, then http://127.0.0.1:8080.
    #[arg(long, global = true, value_name = "URL")]
    url: Option<String>,
    /// Prints JSON instead of text, errors included.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Runs the given Roles until SIGTERM or SIGINT.
    Serve {
        /// Which Roles this process takes on, comma-separated.
        #[arg(long, value_enum, value_delimiter = ',', default_value = "all")]
        role: Vec<Role>,
        /// Clears the User's password and revokes all their Tokens before serving, then logs
        /// a new Setup Link. For a lost password; needs the `api` Role.
        #[arg(long)]
        reset_admin: bool,
    },
    /// Applies the database migrations this release needs.
    Migrate,
    /// Sets up the User with the code from the Setup Link that `bia serve` logs, prompting for
    /// it and a password, and prints an admin Token.
    Setup,
    /// Saves, checks, dry-runs, publishes and activates Workflows.
    Workflow {
        #[command(subcommand)]
        command: api::WorkflowCommand,
    },
    /// Starts, shows, lists, cancels and retries Runs.
    Run {
        #[command(subcommand)]
        command: api::RunCommand,
    },
    /// Speaks MCP on stdin and stdout for a local client such as Claude Code, forwarding to the
    /// server's `/mcp` endpoint with the Token in `BIA_TOKEN`.
    Mcp,
    /// Creates, lists and deletes Queues, and lists their Messages.
    Queue {
        #[command(subcommand)]
        command: api::QueueCommand,
    },
    /// Sets, lists and deletes Secrets. No command shows a value.
    Secret {
        #[command(subcommand)]
        command: api::SecretCommand,
    },
    /// Manages API Tokens.
    Token {
        #[command(subcommand)]
        command: TokenCommand,
    },
}

#[derive(Debug, Subcommand)]
enum TokenCommand {
    /// Creates a Token and prints its value, which is shown only this once. Needs an admin
    /// Token in BIA_TOKEN.
    Create {
        /// What the Token is for, e.g. `claude-desktop`.
        #[arg(long)]
        name: String,
        #[arg(long, value_enum, default_value = "agent")]
        scope: ScopeArg,
        /// The Workflow a `webhook` Token may start Runs of.
        #[arg(long, required_if_eq("scope", "webhook"))]
        workflow: Option<String>,
        /// The webhook Trigger a `webhook` Token may start Runs from.
        #[arg(long, required_if_eq("scope", "webhook"))]
        trigger: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ScopeArg {
    Admin,
    Agent,
    Webhook,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum Role {
    Api,
    Worker,
    Scheduler,
    All,
}

#[tokio::main]
async fn main() -> anyhow::Result<ExitCode> {
    let cli = Cli::parse();
    let server = |cli: &Cli| -> anyhow::Result<Config> {
        let config = Config::load(cli.config.as_deref())?;
        init_logging(&config);
        Ok(config)
    };
    let success = match cli.command {
        Command::Serve {
            ref role,
            reset_admin,
        } => {
            serve(&server(&cli)?, &roles(role), reset_admin).await?;
            true
        }
        Command::Migrate => {
            migrate(&server(&cli)?).await?;
            true
        }
        Command::Mcp => {
            let config = ClientConfig::load(cli.config.as_deref())?;
            mcp::proxy(config.url(cli.url.as_deref()), config.token()).await?;
            true
        }
        Command::Setup => {
            let config = ClientConfig::load(cli.config.as_deref())?;
            setup::run(&config.client(cli.url.as_deref())?).await?;
            true
        }
        Command::Token { ref command } => {
            token(&cli, command).await?;
            true
        }
        Command::Workflow { command } => {
            api::workflow(&api_context(&cli.config, &cli.url, cli.json)?, command).await
        }
        Command::Queue { command } => {
            api::queue(&api_context(&cli.config, &cli.url, cli.json)?, command).await
        }
        Command::Secret { command } => {
            api::secret(&api_context(&cli.config, &cli.url, cli.json)?, command).await
        }
        Command::Run { command } => {
            api::run(&api_context(&cli.config, &cli.url, cli.json)?, command).await
        }
    };
    Ok(if success {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
fn api_context(
    file: &Option<PathBuf>,
    url: &Option<String>,
    json: bool,
) -> anyhow::Result<api::Ctx> {
    let config = ClientConfig::load(file.as_deref())?;
    Ok(api::Ctx {
        client: config.client(url.as_deref())?,
        url: config.url(url.as_deref()).to_owned(),
        json,
    })
}

async fn token(cli: &Cli, command: &TokenCommand) -> anyhow::Result<()> {
    let TokenCommand::Create {
        name,
        scope,
        workflow,
        trigger,
    } = command;
    let config = ClientConfig::load(cli.config.as_deref())?;
    let client = config.client(cli.url.as_deref())?;
    if config.token().is_none() {
        bail!(
            "set BIA_TOKEN (or `token` in the config file) to an admin Token; on a fresh \
             install, `bia setup` prints one"
        );
    }
    let (scope, webhook) = match (scope, workflow, trigger) {
        (ScopeArg::Admin, ..) => (Scope::Admin, None),
        (ScopeArg::Agent, ..) => (Scope::Agent, None),
        (ScopeArg::Webhook, Some(workflow_id), Some(trigger_id)) => (
            Scope::Webhook,
            Some(WebhookBinding {
                workflow_id: workflow_id.clone(),
                trigger_id: trigger_id.clone(),
            }),
        ),
        (ScopeArg::Webhook, ..) => bail!("a `webhook` Token needs --workflow and --trigger"),
    };
    let new = NewToken {
        name: name.clone(),
        scope,
        webhook,
    };
    let created = client.create_token(&new).await?;
    eprintln!(
        "created {:?} Token `{}` ({}); its value is shown only now",
        created.token.scope, created.token.name, created.token.id
    );
    println!("{}", created.value);
    Ok(())
}

fn roles(given: &[Role]) -> BTreeSet<Role> {
    if given.contains(&Role::All) {
        return BTreeSet::from([Role::Api, Role::Worker, Role::Scheduler]);
    }
    given.iter().copied().collect()
}

fn init_logging(config: &Config) {
    // `Config::load` already checked the filter.
    let filter = tracing_subscriber::EnvFilter::new(&config.log.filter);
    let logs = tracing_subscriber::fmt().with_env_filter(filter);
    match config.log.format {
        LogFormat::Text => logs.init(),
        LogFormat::Json => logs.json().init(),
    }
}

async fn connect(config: &Config, max_connections: u32) -> anyhow::Result<PgPool> {
    PgPoolOptions::new()
        .max_connections(max_connections)
        .connect(config.database_url()?)
        .await
        .context("cannot connect to the database")
}

async fn migrate(config: &Config) -> anyhow::Result<()> {
    let pool = connect(config, 1).await?;
    bia_store::migrate(&pool)
        .await
        .context("cannot apply migrations")?;
    tracing::info!("migrations applied");
    Ok(())
}

async fn serve(config: &Config, roles: &BTreeSet<Role>, reset_admin: bool) -> anyhow::Result<()> {
    if reset_admin && !roles.contains(&Role::Api) {
        bail!("--reset-admin needs the `api` Role, which serves the new Setup Link");
    }
    // Each in-flight Job can hold a connection for its transition and one for its heartbeat,
    // and the Worker loop needs one to claim; the listener has its own. The scheduler fires
    // one Schedule at a time and holds its leader lock on a connection of its own. The API
    // gets a few more so requests do not queue behind Jobs.
    let concurrency = u32::try_from(config.worker.concurrency.get()).unwrap_or(u32::MAX);
    let pool = connect(config, concurrency.saturating_mul(2).saturating_add(6)).await?;
    let master_key = config.master_key()?;
    require_master_key_for_secrets(&pool, master_key.is_some()).await?;
    if reset_admin {
        reset_admin_password(&pool).await?;
    }
    let shutdown = CancellationToken::new();
    tokio::spawn(stop_on_signal(shutdown.clone()));
    let actions = Arc::new(bia_actions::builtin_registry().context("cannot build the Actions")?);

    let api = async {
        if !roles.contains(&Role::Api) {
            return Ok(());
        }
        let bind = config.api.bind;
        let listener = TcpListener::bind(bind)
            .await
            .with_context(|| format!("cannot listen on {bind}"))?;
        let listening = listener.local_addr()?;
        tracing::info!(address = %listening, "api listening");
        let (setup_code, code) = SetupCode::generate().context("cannot make a setup code")?;
        log_setup_link(&pool, &config.setup_link(&code, listening)?).await?;
        let api_config = bia_server::ApiConfig {
            setup_code: Some(setup_code),
            ..config.api_config()?
        };
        let state = ApiState::new(pool.clone(), actions.clone(), Arc::new(SystemClock))
            .with_config(api_config);
        let router = bia_server::router(state);
        bia_server::serve(listener, router, shutdown.clone())
            .await
            .context("the api failed")?;
        tracing::info!("api stopped");
        Ok(())
    };
    let worker = async {
        if !roles.contains(&Role::Worker) {
            return Ok(());
        }
        let interpreter = Interpreter::new(pool.clone(), actions.clone(), Arc::new(SystemClock))
            .with_config(config.interpreter_config())
            .with_secrets(Arc::new(InternalSecrets::new(
                pool.clone(),
                master_key.clone(),
            )));
        let worker = Worker::new(config.worker_id(), interpreter, config.worker_config());
        worker
            .run(shutdown.clone())
            .await
            .context("the worker failed")
    };
    let scheduler = async {
        if !roles.contains(&Role::Scheduler) {
            return Ok(());
        }
        let scheduler = Scheduler::new(
            config::process_name(),
            pool.clone(),
            Arc::new(SystemClock),
            config.scheduler_config(),
        )
        .with_chore(Arc::new(Retention::new(config.retention_config())));
        scheduler
            .run(shutdown.clone())
            .await
            .context("the scheduler failed")
    };
    tokio::try_join!(api, worker, scheduler, async {
        shutdown.cancelled().await;
        anyhow::Ok(())
    })?;
    tracing::info!("stopped");
    Ok(())
}

/// Secrets are encrypted, so a server without the key could neither read them nor tell the
/// operator why Steps fail: refuse to start instead.
async fn require_master_key_for_secrets(pool: &PgPool, has_key: bool) -> anyhow::Result<()> {
    if has_key {
        return Ok(());
    }
    let mut conn = pool
        .acquire()
        .await
        .context("cannot connect to the database")?;
    if PgSecretRepository.any(&mut conn).await? {
        bail!(
            "invalid configuration: Secrets exist but `master_key` is not set; set BIA_MASTER_KEY \
             to the key they were stored under"
        );
    }
    Ok(())
}

/// Clears the User's password and revokes their Tokens, so the Setup Link this start logs is
/// the way back in. `serve` does it itself so only `serve` and `migrate` touch Postgres
/// (ADR-0007).
async fn reset_admin_password(pool: &PgPool) -> anyhow::Result<()> {
    let mut conn = pool
        .acquire()
        .await
        .context("cannot connect to the database")?;
    match PgUserRepository.reset(&mut conn, SystemClock.now()).await? {
        Some(revoked) => {
            tracing::warn!(revoked_tokens = revoked, "admin reset: password cleared");
        }
        None => tracing::info!("admin reset: no User exists yet"),
    }
    Ok(())
}

/// Logs the Setup Link while no User can sign in. The link carries the setup code: logging it
/// is the point, since the operator reading the logs is who it is for.
async fn log_setup_link(pool: &PgPool, link: &str) -> anyhow::Result<()> {
    let mut conn = pool
        .acquire()
        .await
        .context("cannot connect to the database")?;
    if PgUserRepository.needs_setup(&mut conn).await? {
        tracing::warn!(
            setup_link = link,
            "no User can sign in yet; open the Setup Link, or run `bia setup` with its code"
        );
    }
    Ok(())
}

/// Starts the graceful shutdown on the first SIGTERM or SIGINT, and exits at once on a second.
async fn stop_on_signal(shutdown: CancellationToken) {
    if let Err(error) = signal().await {
        tracing::error!(%error, "cannot listen for signals; stop the process with SIGKILL");
        return;
    }
    tracing::info!("shutting down; signal again to exit at once");
    shutdown.cancel();
    if signal().await.is_ok() {
        std::process::exit(130);
    }
}

#[cfg(unix)]
async fn signal() -> std::io::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut terminate = signal(SignalKind::terminate())?;
    tokio::select! {
        interrupted = tokio::signal::ctrl_c() => interrupted,
        _ = terminate.recv() => Ok(()),
    }
}

#[cfg(not(unix))]
async fn signal() -> std::io::Result<()> {
    tokio::signal::ctrl_c().await
}
