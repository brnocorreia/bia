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
| `api.bind` | `BIA_API_BIND` | `127.0.0.1:8080` | The address and port the `api` Role listens on. It defaults to loopback; set `0.0.0.0:8080` to accept connections from other hosts, such as in a container. |
| `worker.id` | `BIA_WORKER_ID` | `<HOSTNAME>-<pid>` | The name a Worker records on the Leases it holds, for diagnosis. It need not be unique. |
| `worker.concurrency` | `BIA_WORKER_CONCURRENCY` | `16` | How many Jobs a Worker runs at once, at least 1. The connection pool holds up to twice as many connections, plus one. |
| `worker.lease` | `BIA_WORKER_LEASE` | `30s` | How long a claim holds a Job, at least `1s`. A heartbeat extends it every third of its duration while the Job runs, so it bounds how long a crashed Worker's Jobs wait before another Worker takes them. |
| `worker.poll_interval` | `BIA_WORKER_POLL_INTERVAL` | `5s` | How often an idle Worker claims without a notification, which bounds how late a delayed Job, such as a sleep or a retry, starts. |
| `worker.shutdown_grace` | `BIA_WORKER_SHUTDOWN_GRACE` | `20s` | How long in-flight Jobs may keep running after SIGTERM or SIGINT. Jobs still running then are stopped and their Leases released to another Worker. Keep it below your orchestrator's kill timeout. |
| `scheduler.poll_interval` | `BIA_SCHEDULER_POLL_INTERVAL` | `1s` | How often a scheduler fires due cron Triggers and checks whether it leads. It bounds how late an occurrence fires and how long another scheduler takes over after the leader dies. |
| `run.retention_days` | `BIA_RUN_RETENTION_DAYS` | `90` | Days succeeded and cancelled Runs are kept after they finish, at least 1. Past it the leading scheduler deletes them with their Timelines and Jobs. Unfinished Runs are never deleted. |
| `run.failed_retention_days` | `BIA_RUN_FAILED_RETENTION_DAYS` | `90` | Days failed Runs are kept after they finish, at least 1, independently of `run.retention_days`. |
| `dead_letter.retention_days` | `BIA_DEAD_LETTER_RETENTION_DAYS` | `90` | Days Messages stay in a Dead-Letter Queue, at least 1. It takes effect once Queues exist. |
| `limits.max_steps_per_run` | `BIA_LIMITS_MAX_STEPS_PER_RUN` | `10000` | Most Steps one Run may take up, at least 1. A Step counts once each time the Run reaches it; retries do not count again. Past it the Run fails with `run_step_limit_exceeded`. |
| `limits.max_iterations` | `BIA_LIMITS_MAX_ITERATIONS` | `10000` | Most iterations one loop may run, `flow.foreach` items included, at least 1. Past it the loop's Step fails with `iteration_limit_exceeded`. |
| `limits.max_subworkflow_depth` | `BIA_LIMITS_MAX_SUBWORKFLOW_DEPTH` | `8` | How many levels of `flow.subworkflow` may nest below a Run; `0` forbids Subworkflows. Past it the Step fails with `subworkflow_depth_exceeded`. |
| `limits.max_output_bytes` | `BIA_LIMITS_MAX_OUTPUT_BYTES` | `262144` (256 KiB) | The largest Output a Step may produce, in bytes of compact JSON, at least 1. A larger one fails the Step with `output_limit_exceeded` and is not stored. |
| `log.format` | `BIA_LOG_FORMAT` | `text` | `text` for people, `json` for log pipelines. |
| `log.filter` | `BIA_LOG_FILTER` | `info` | A [`tracing` filter directive](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html), e.g. `info,bia_engine=debug`. |

Limit errors are `fatal`, so they never consume a Step's `retry`. A Definition's own `limits` and a loop's `max_iterations` can only lower these limits (ADR-0038).

Retention runs on the leading scheduler every 10 minutes, deleting up to 10,000 Runs of each retention per run in batches of 500, so a large backlog is cleared over several runs without holding up cron Triggers (ADR-0021, ADR-0041). A process without the `scheduler` Role never deletes Runs.

Unknown keys in the file are errors. `BIA_*` variables that name no key are ignored.

Logs about a Run carry its `run_id`, and logs about a Job its `job_id`. Inputs, Outputs, errors and Secrets are never logged; read them on the Run's Timeline.

## Example

```toml
database_url = "postgres://bia:bia@localhost:5432/bia"

[worker]
concurrency = 32
lease = "1m"

[scheduler]
poll_interval = "2s"

[run]
retention_days = 30
failed_retention_days = 180

[log]
format = "json"
```

```sh
bia migrate --config bia.toml
BIA_WORKER_CONCURRENCY=8 bia serve --config bia.toml --role worker
```

## Signals

The first SIGTERM or SIGINT starts the graceful shutdown described under `worker.shutdown_grace`, and a leading scheduler hands leadership to another one at once. A second one exits at once, leaving unfinished Jobs to be claimed again once their Leases expire.
