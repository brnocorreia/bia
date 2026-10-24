//! Configuration for `bia serve` and `bia migrate`: defaults, an optional TOML file, then the
//! environment. Naming and precedence are ADR-0037; the keys are listed in
//! `docs/configuration.md`, which must follow any change to them.

use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, anyhow, bail};
use bia_core::dsl::Duration;
use bia_engine::{InterpreterConfig, RunLimits, SchedulerConfig, WorkerConfig};
use chrono::TimeDelta;
use figment::Figment;
use figment::providers::{Env, Format, Toml};
use figment::value::{Uncased, UncasedStr};
use serde::Deserialize;
use sqlx::postgres::PgConnectOptions;
use tracing_subscriber::EnvFilter;

const ENV_PREFIX: &str = "BIA_";
const SECTIONS: [&str; 4] = ["worker", "scheduler", "log", "limits"];
/// Every key the environment may set, as key paths.
const KEYS: [&str; 13] = [
    "database_url",
    "worker.id",
    "worker.concurrency",
    "worker.lease",
    "worker.poll_interval",
    "worker.shutdown_grace",
    "scheduler.poll_interval",
    "log.format",
    "log.filter",
    "limits.max_steps_per_run",
    "limits.max_iterations",
    "limits.max_subworkflow_depth",
    "limits.max_output_bytes",
];

// No `Debug`: the database URL usually carries a password.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Config {
    database_url: Option<String>,
    pub(crate) worker: Worker,
    pub(crate) scheduler: Scheduler,
    pub(crate) log: Log,
    limits: Limits,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Worker {
    /// Names the Worker in the Leases it holds; defaults to `<host>-<pid>`.
    id: Option<String>,
    pub(crate) concurrency: NonZeroUsize,
    lease: Duration,
    poll_interval: Duration,
    shutdown_grace: Duration,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Scheduler {
    poll_interval: Duration,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Log {
    pub(crate) format: LogFormat,
    /// A `tracing` filter directive, e.g. `info` or `info,bia_engine=debug`.
    pub(crate) filter: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Limits {
    max_steps_per_run: NonZeroU64,
    max_iterations: NonZeroU32,
    /// 0 forbids Subworkflows.
    max_subworkflow_depth: u32,
    max_output_bytes: NonZeroU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum LogFormat {
    Text,
    Json,
}

/// A default `TimeDelta` as a config Duration, in whole seconds.
fn duration(delta: TimeDelta) -> Duration {
    let seconds = u64::try_from(delta.num_seconds()).unwrap_or_default();
    Duration::new(seconds, bia_core::dsl::DurationUnit::Seconds)
        .unwrap_or_else(|| unreachable!("default durations are small"))
}

fn delta(duration: Duration) -> TimeDelta {
    TimeDelta::from_std(duration.to_std()).unwrap_or(TimeDelta::MAX)
}

impl Default for Worker {
    fn default() -> Self {
        let defaults = WorkerConfig::default();
        Self {
            id: None,
            concurrency: defaults.concurrency,
            lease: duration(defaults.lease),
            poll_interval: duration(defaults.poll_interval),
            shutdown_grace: duration(defaults.shutdown_grace),
        }
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self {
            poll_interval: duration(SchedulerConfig::default().poll_interval),
        }
    }
}

impl Default for Log {
    fn default() -> Self {
        Self {
            format: LogFormat::Text,
            filter: "info".to_owned(),
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        let defaults = RunLimits::default();
        Self {
            max_steps_per_run: NonZeroU64::new(defaults.max_steps_per_run)
                .unwrap_or(NonZeroU64::MIN),
            max_iterations: NonZeroU32::new(defaults.max_iterations).unwrap_or(NonZeroU32::MIN),
            max_subworkflow_depth: defaults.max_subworkflow_depth,
            max_output_bytes: NonZeroU64::new(defaults.max_output_bytes).unwrap_or(NonZeroU64::MIN),
        }
    }
}

impl Config {
    /// Reads the defaults, then `file` when given, then the environment, and checks the
    /// result. Errors name the key and where its value came from.
    pub(crate) fn load(file: Option<&Path>) -> anyhow::Result<Self> {
        let mut figment = Figment::new();
        if let Some(file) = file {
            if !file.is_file() {
                bail!("config file {} not found", file.display());
            }
            figment = figment.merge(Toml::file_exact(file));
        }
        let config: Self = figment
            .merge(Env::raw().only(&["database_url"]))
            .merge(
                Env::prefixed(ENV_PREFIX)
                    .map(key_path)
                    .filter(|key| KEYS.iter().any(|known| key == *known)),
            )
            .extract()
            .map_err(|errors| {
                let errors: Vec<String> = errors.into_iter().map(describe).collect();
                anyhow!("invalid configuration: {}", errors.join("; "))
            })?;
        config.check()?;
        Ok(config)
    }

    fn check(&self) -> anyhow::Result<()> {
        let url = self.database_url()?;
        // Neither message echoes the URL, which may hold a password.
        let not_postgres = "invalid configuration: `database_url` is not a Postgres URL";
        if !(url.starts_with("postgres://") || url.starts_with("postgresql://")) {
            bail!(not_postgres);
        }
        PgConnectOptions::from_str(url).context(not_postgres)?;
        if self.worker.lease.as_millis() < 1_000 {
            bail!("invalid configuration: `worker.lease` must be at least 1s");
        }
        if self.worker.poll_interval.as_millis() == 0 {
            bail!("invalid configuration: `worker.poll_interval` must be more than 0s");
        }
        if self.scheduler.poll_interval.as_millis() == 0 {
            bail!("invalid configuration: `scheduler.poll_interval` must be more than 0s");
        }
        EnvFilter::try_new(&self.log.filter).with_context(|| {
            format!(
                "invalid configuration: `log.filter` `{}` is not a tracing filter",
                self.log.filter
            )
        })?;
        Ok(())
    }

    pub(crate) fn database_url(&self) -> anyhow::Result<&str> {
        self.database_url.as_deref().ok_or_else(|| {
            anyhow!(
                "invalid configuration: no database URL; set BIA_DATABASE_URL or \
                 DATABASE_URL, or `database_url` in the config file"
            )
        })
    }

    pub(crate) fn worker_id(&self) -> String {
        self.worker.id.clone().unwrap_or_else(process_name)
    }

    pub(crate) fn worker_config(&self) -> WorkerConfig {
        WorkerConfig {
            concurrency: self.worker.concurrency,
            lease: delta(self.worker.lease),
            poll_interval: delta(self.worker.poll_interval),
            shutdown_grace: delta(self.worker.shutdown_grace),
            ..WorkerConfig::default()
        }
    }

    pub(crate) fn scheduler_config(&self) -> SchedulerConfig {
        SchedulerConfig {
            poll_interval: delta(self.scheduler.poll_interval),
        }
    }

    pub(crate) fn interpreter_config(&self) -> InterpreterConfig {
        let limits = &self.limits;
        InterpreterConfig {
            limits: RunLimits {
                max_steps_per_run: limits.max_steps_per_run.get(),
                max_iterations: limits.max_iterations.get(),
                max_subworkflow_depth: limits.max_subworkflow_depth,
                max_output_bytes: limits.max_output_bytes.get(),
            },
            ..InterpreterConfig::default()
        }
    }
}

/// `<HOSTNAME>-<pid>`, naming this process in Leases and logs.
pub(crate) fn process_name() -> String {
    let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "bia".to_owned());
    format!("{host}-{}", std::process::id())
}

/// One extraction error, naming the key and the file or variable its value came from.
fn describe(error: figment::Error) -> String {
    let key = error.path.join(".").to_ascii_lowercase();
    let from = match &error.metadata {
        Some(metadata) => match &metadata.source {
            Some(source) => format!(" in {source}"),
            None if metadata.name.contains(ENV_PREFIX) => {
                format!(
                    " in {ENV_PREFIX}{}",
                    key.replace('.', "_").to_ascii_uppercase()
                )
            }
            None if metadata.name.contains("environment") => {
                format!(" in {}", key.to_ascii_uppercase())
            }
            None => format!(" in {}", metadata.name),
        },
        None => String::new(),
    };
    format!("`{key}`{from}: {}", error.kind)
}

/// `worker_poll_interval` → `worker.poll_interval`; keys outside a section stay as they are.
fn key_path(key: &UncasedStr) -> Uncased<'_> {
    // An uncased key keeps the case it was written in.
    let key = key.as_str().to_ascii_lowercase();
    for section in SECTIONS {
        if let Some(name) = key
            .strip_prefix(section)
            .and_then(|rest| rest.strip_prefix('_'))
        {
            return format!("{section}.{name}").into();
        }
    }
    key.into()
}

#[cfg(test)]
// `Jail` closures return `figment::Error`, which is large; that is figment's choice.
#[allow(clippy::result_large_err)]
mod tests {
    use std::num::NonZeroUsize;
    use std::path::Path;

    use bia_engine::RunLimits;
    use chrono::TimeDelta;
    use figment::Jail;

    use super::{Config, LogFormat};

    const URL: &str = "postgres://bia:bia@localhost:5432/bia";

    fn load(jail: &Jail, file: Option<&str>) -> anyhow::Result<Config> {
        Config::load(file.map(|file| jail.directory().join(file)).as_deref())
    }

    fn error(jail: &Jail, file: Option<&str>) -> String {
        match load(jail, file) {
            Ok(_) => panic!("the configuration loaded"),
            Err(error) => format!("{error:#}"),
        }
    }

    #[test]
    fn the_environment_overrides_the_file_which_overrides_the_defaults() {
        Jail::expect_with(|jail| {
            jail.clear_env();
            jail.create_file(
                "bia.toml",
                &format!(
                    r#"
                    database_url = "{URL}"

                    [worker]
                    concurrency = 4
                    lease = "1m"

                    [log]
                    format = "json"
                    "#
                ),
            )?;
            jail.set_env("BIA_WORKER_CONCURRENCY", "32");
            jail.set_env("BIA_WORKER_POLL_INTERVAL", "250ms");
            jail.set_env("BIA_SCHEDULER_POLL_INTERVAL", "2s");

            let config = load(jail, Some("bia.toml")).unwrap();

            let worker = config.worker_config();
            assert_eq!(
                worker.concurrency,
                NonZeroUsize::new(32).unwrap(),
                "env wins"
            );
            assert_eq!(worker.lease, TimeDelta::minutes(1), "the file wins");
            assert_eq!(worker.poll_interval, TimeDelta::milliseconds(250));
            assert_eq!(worker.shutdown_grace, TimeDelta::seconds(20), "default");
            assert_eq!(
                config.scheduler_config().poll_interval,
                TimeDelta::seconds(2)
            );
            assert_eq!(config.log.format, LogFormat::Json);
            assert_eq!(config.log.filter, "info");
            Ok(())
        });
    }

    #[test]
    fn bia_database_url_wins_over_database_url_which_wins_over_the_file() {
        Jail::expect_with(|jail| {
            jail.clear_env();
            jail.create_file("bia.toml", r#"database_url = "postgres://file/bia""#)?;
            assert_eq!(
                load(jail, Some("bia.toml"))
                    .unwrap()
                    .database_url()
                    .unwrap(),
                "postgres://file/bia"
            );

            jail.set_env("DATABASE_URL", "postgres://plain/bia");
            assert_eq!(
                load(jail, Some("bia.toml"))
                    .unwrap()
                    .database_url()
                    .unwrap(),
                "postgres://plain/bia"
            );

            jail.set_env("BIA_DATABASE_URL", "postgres://prefixed/bia");
            assert_eq!(
                load(jail, Some("bia.toml"))
                    .unwrap()
                    .database_url()
                    .unwrap(),
                "postgres://prefixed/bia"
            );
            Ok(())
        });
    }

    #[test]
    fn an_invalid_value_names_its_key_and_where_it_came_from() {
        Jail::expect_with(|jail| {
            jail.clear_env();
            jail.set_env("BIA_DATABASE_URL", URL);
            jail.set_env("BIA_WORKER_CONCURRENCY", "0");

            let error = error(jail, None);

            assert!(error.contains("worker.concurrency"), "{error}");
            assert!(error.contains("BIA_WORKER_CONCURRENCY"), "{error}");
            Ok(())
        });
    }

    #[test]
    fn an_unknown_file_key_fails_but_an_unknown_bia_variable_is_ignored() {
        Jail::expect_with(|jail| {
            jail.clear_env();
            jail.set_env("BIA_DATABASE_URL", URL);
            jail.set_env("BIA_MASTER_KEY", "for a later release");
            assert!(load(jail, None).is_ok());

            jail.create_file("bia.toml", "[worker]\nconcurency = 4\n")?;
            let error = error(jail, Some("bia.toml"));
            assert!(error.contains("concurency"), "{error}");
            Ok(())
        });
    }

    #[test]
    fn startup_needs_a_valid_database_url_and_sane_durations() {
        Jail::expect_with(|jail| {
            jail.clear_env();
            assert!(error(jail, None).contains("BIA_DATABASE_URL"));

            jail.set_env("BIA_DATABASE_URL", "mysql://elsewhere");
            assert!(error(jail, None).contains("not a Postgres URL"));

            jail.set_env("BIA_DATABASE_URL", URL);
            jail.set_env("BIA_WORKER_LEASE", "10ms");
            assert!(error(jail, None).contains("`worker.lease` must be at least 1s"));

            jail.set_env("BIA_WORKER_LEASE", "30s");
            jail.set_env("BIA_WORKER_POLL_INTERVAL", "soon");
            assert!(error(jail, None).contains("BIA_WORKER_POLL_INTERVAL"));
            Ok(())
        });
    }

    #[test]
    fn global_limits_come_from_the_file_and_the_environment() {
        Jail::expect_with(|jail| {
            jail.clear_env();
            jail.set_env("BIA_DATABASE_URL", URL);
            assert_eq!(
                load(jail, None).unwrap().interpreter_config().limits,
                RunLimits::default()
            );

            jail.create_file(
                "bia.toml",
                "[limits]\nmax_steps_per_run = 50\nmax_subworkflow_depth = 0\n",
            )?;
            jail.set_env("BIA_LIMITS_MAX_OUTPUT_BYTES", "1024");

            let limits = load(jail, Some("bia.toml"))
                .unwrap()
                .interpreter_config()
                .limits;
            assert_eq!(
                limits,
                RunLimits {
                    max_steps_per_run: 50,
                    max_subworkflow_depth: 0,
                    max_output_bytes: 1024,
                    ..RunLimits::default()
                }
            );

            jail.set_env("BIA_LIMITS_MAX_ITERATIONS", "0");
            let error = error(jail, Some("bia.toml"));
            assert!(error.contains("limits.max_iterations"), "{error}");
            assert!(error.contains("BIA_LIMITS_MAX_ITERATIONS"), "{error}");
            Ok(())
        });
    }

    #[test]
    fn a_missing_config_file_fails() {
        let error = Config::load(Some(Path::new("/nonexistent/bia.toml")))
            .err()
            .unwrap();
        assert!(error.to_string().contains("not found"), "{error}");
    }
}
