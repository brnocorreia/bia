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
| `cors_origins` | `BIA_CORS_ORIGINS` | none | Browser origins allowed to call the API, for a UI hosted elsewhere, written as the browser sends them (`https://bia.example.com`, `http://localhost:5173`): a TOML array, or comma-separated in the variable. `*` allows any origin. A preflight from any other origin fails. Callers that are not browsers are unaffected. |
| `bootstrap_key` | `BIA_BOOTSTRAP_KEY` | none | A secret of at least 32 characters that lets `bia token create --bootstrap` mint the first admin Token. It works only while no unrevoked admin Token exists; leave it unset to turn bootstrapping off. See [Tokens](#tokens). |
| `url` | `BIA_URL` | `http://127.0.0.1:8080` | Where the API commands (`bia workflow`, `bia run`, `bia token`) find the server. The global `--url` flag overrides it. `bia serve` ignores it. |
| `token` | `BIA_TOKEN` | none | The Token the API commands send. There is no flag, so it never appears in a process list. Keep a config file that holds it readable only by you. `bia serve` ignores it. |
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

## Tokens

Every API route except `GET /health`, `GET /ready`, `GET /openapi.json` and bootstrapping needs a Token, sent as `Authorization: Bearer <token>`. Its Scope decides what it may call: `admin` everything, `agent` everything but publishing, activation and administering Tokens, `webhook` only the webhook Trigger it is bound to (ADR-0008, ADR-0043). bia stores only a hash of each Token, so a lost value cannot be recovered, only replaced.

On a fresh install, start the server with a bootstrap key and mint the first admin Token with the same key:

```sh
export BIA_BOOTSTRAP_KEY="$(openssl rand -hex 32)"
bia serve &
export BIA_TOKEN="$(bia token create --bootstrap --name admin)"
```

Once an unrevoked admin Token exists, bootstrapping refuses (`already_bootstrapped`), so the key cannot mint another; create more Tokens with the admin Token instead, e.g. `bia token create --name claude-desktop --scope agent`. If every admin Token is revoked, bootstrapping works again, which is how to regain access; unset `BIA_BOOTSTRAP_KEY` to turn it off. `bia token create` talks to the API at `BIA_URL` (default `http://127.0.0.1:8080`) and reads credentials only from `BIA_TOKEN` (or `token` in the config file) and `BIA_BOOTSTRAP_KEY`, never from arguments. Token values and the bootstrap key are never logged.

## Example

```toml
database_url = "postgres://bia:bia@localhost:5432/bia"
cors_origins = ["https://bia.example.com"]

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

## Command line

The API commands need the server's `url` and a `token` (see [Tokens](#tokens)), from the config file or the environment. `--json` on any command prints JSON instead of text, errors included. Exit status is 0 on success and 1 on failure, on error Diagnostics and on a failed Dry Run (ADR-0047).

```sh
export BIA_URL=http://127.0.0.1:8080 BIA_TOKEN=...
bia workflow apply -f workflows/            # Drafts from JSON/YAML files and directories
bia workflow apply -f workflows/ --activate # also publish and activate (admin Token)
bia workflow validate -f price.yaml         # or: validate price 3 --target publish
bia workflow dry-run -f price.yaml --input input.json --mocks mocks.yaml
bia workflow publish price 3
bia workflow activate price 3
bia workflow diff price 2 3
bia run trigger price --input input.json
bia run get <id>
bia run list --workflow price --status failed
bia run cancel <id>
bia run retry <id>
```

## Signals

The first SIGTERM or SIGINT starts the graceful shutdown described under `worker.shutdown_grace`, and a leading scheduler hands leadership to another one at once. A second one exits at once, leaving unfinished Jobs to be claimed again once their Leases expire.
