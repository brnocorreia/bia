# Configuration

`bia serve` and `bia migrate` read their configuration at startup and refuse to start when any of it is invalid, naming the key and where its value came from. The conventions are ADR-0037.

Each setting can come from these sources, and a later one overrides an earlier one:

1. The built-in default.
2. The TOML file named by `--config <path>` or `BIA_CONFIG`. Without one, no file is read.
3. `DATABASE_URL`, for `database_url` only.
4. The `BIA_*` variable.

A file key `section.name` is the variable `BIA_SECTION_NAME`. Durations are a whole number and a unit: `ms`, `s`, `m`, `h` or `d`.

| File key | Variable | Default | Meaning |
|---|---|---|---|
| `database_url` | `BIA_DATABASE_URL` (or `DATABASE_URL`) | none, required | The Postgres URL, `postgres://user:password@host:port/database`. |
| `worker.id` | `BIA_WORKER_ID` | `<HOSTNAME>-<pid>` | The name a Worker records on the Leases it holds, for diagnosis. It need not be unique. |
| `worker.concurrency` | `BIA_WORKER_CONCURRENCY` | `16` | How many Jobs a Worker runs at once, at least 1. The connection pool holds up to twice as many connections, plus one. |
| `worker.lease` | `BIA_WORKER_LEASE` | `30s` | How long a claim holds a Job, at least `1s`. A heartbeat extends it every third of its duration while the Job runs, so it bounds how long a crashed Worker's Jobs wait before another Worker takes them. |
| `worker.poll_interval` | `BIA_WORKER_POLL_INTERVAL` | `5s` | How often an idle Worker claims without a notification, which bounds how late a delayed Job, such as a sleep or a retry, starts. |
| `worker.shutdown_grace` | `BIA_WORKER_SHUTDOWN_GRACE` | `20s` | How long in-flight Jobs may keep running after SIGTERM or SIGINT. Jobs still running then are stopped and their Leases released to another Worker. Keep it below your orchestrator's kill timeout. |
| `log.format` | `BIA_LOG_FORMAT` | `text` | `text` for people, `json` for log pipelines. |
| `log.filter` | `BIA_LOG_FILTER` | `info` | A [`tracing` filter directive](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html), e.g. `info,bia_engine=debug`. |

Unknown keys in the file are errors. `BIA_*` variables that name no key are ignored.

Logs about a Run carry its `run_id`, and logs about a Job its `job_id`. Inputs, Outputs, errors and Secrets are never logged; read them on the Run's Timeline.

## Example

```toml
database_url = "postgres://bia:bia@localhost:5432/bia"

[worker]
concurrency = 32
lease = "1m"

[log]
format = "json"
```

```sh
bia migrate --config bia.toml
BIA_WORKER_CONCURRENCY=8 bia serve --config bia.toml --role worker
```

## Signals

The first SIGTERM or SIGINT starts the graceful shutdown described under `worker.shutdown_grace`. A second one exits at once, leaving unfinished Jobs to be claimed again once their Leases expire.
