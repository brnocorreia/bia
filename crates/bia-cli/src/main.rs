//! The `bia` binary. `serve` and `migrate` touch Postgres directly; every other subcommand
//! will be a client of the HTTP API (ADR-0005, ADR-0007).

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use bia_core::action::CancellationToken;
use bia_engine::{Interpreter, Scheduler, Worker};
use bia_store::SystemClock;
use clap::{Parser, Subcommand, ValueEnum};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

mod config;

use config::{Config, LogFormat};

#[derive(Debug, Parser)]
#[command(name = "bia", version, about = "Workflows as data, run from Postgres.")]
struct Cli {
    /// A TOML config file. Environment variables override what it sets.
    #[arg(long, global = true, env = "BIA_CONFIG")]
    config: Option<PathBuf>,
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
    },
    /// Applies the database migrations this release needs.
    Migrate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum Role {
    Api,
    Worker,
    Scheduler,
    All,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config = Config::load(cli.config.as_deref())?;
    init_logging(&config);
    match cli.command {
        Command::Serve { role } => serve(&config, &roles(&role)).await,
        Command::Migrate => migrate(&config).await,
    }
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

async fn serve(config: &Config, roles: &BTreeSet<Role>) -> anyhow::Result<()> {
    // Each in-flight Job can hold a connection for its transition and one for its heartbeat,
    // and the Worker loop needs one to claim; the listener has its own. The scheduler fires
    // one Schedule at a time and holds its leader lock on a connection of its own.
    let concurrency = u32::try_from(config.worker.concurrency.get()).unwrap_or(u32::MAX);
    let pool = connect(config, concurrency.saturating_mul(2).saturating_add(2)).await?;
    let shutdown = CancellationToken::new();
    tokio::spawn(stop_on_signal(shutdown.clone()));

    if roles.contains(&Role::Api) {
        tracing::warn!(role = "api", "this Role does nothing yet");
    }
    let worker = async {
        if !roles.contains(&Role::Worker) {
            return Ok(());
        }
        let actions = bia_actions::builtin_registry().context("cannot build the Actions")?;
        let interpreter = Interpreter::new(pool.clone(), Arc::new(actions), Arc::new(SystemClock));
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
        );
        scheduler
            .run(shutdown.clone())
            .await
            .context("the scheduler failed")
    };
    tokio::try_join!(worker, scheduler, async {
        shutdown.cancelled().await;
        anyhow::Ok(())
    })?;
    tracing::info!("stopped");
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
