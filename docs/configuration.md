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
| `public_url` | `BIA_PUBLIC_URL` | `http://<api.bind>` | The URL people reach the API at, which links the server hands out are built from. The default uses the port the API listens on and writes a loopback or `0.0.0.0` address as `localhost`; set it when bia is behind a proxy or reached by another name, e.g. `https://bia.example.com`. It must be an `http` or `https` URL without credentials, query or fragment; an invalid one stops startup (ADR-0063). |
| `ui_url` | `BIA_UI_URL` | `public_url` in a build with the UI; unset in a build without it | The URL the UI is reached at, where the [Setup Link](#first-run-setup) points and where people approve OAuth clients and `bia login --device`. Set it when the UI is hosted apart from the API (it must then be same-site with it, see [`deploy-ui.md`](deploy-ui.md)). Same rules as `public_url`. A build without the UI leaves it unset, and then `/oauth/authorize` answers with a plain-text error naming `ui_url` until you set it; such an install uses manual Tokens (ADR-0067). |
| `master_key` | `BIA_MASTER_KEY` | none | The key Secrets are encrypted under: exactly 64 hex digits, e.g. from `openssl rand -hex 32`. Without it the server cannot store or read Secrets; it refuses to start when Secrets already exist. Back it up: losing it loses every Secret, and changing it makes the stored ones unreadable. See [Secrets](#secrets). |
| `url` | `BIA_URL` | `http://127.0.0.1:8080` | Where the API commands (`bia workflow`, `bia run`, `bia token`) find the server. The global `--url` flag overrides it. `bia serve` ignores it. |
| `token` | `BIA_TOKEN` | none | A manual Token the API commands send, for CI and scripts. It wins over a login from `bia login`. There is no flag, so it never appears in a process list. Keep a config file that holds it readable only by you. `bia serve` ignores it. |
| `api.bind` | `BIA_API_BIND` | `127.0.0.1:8080` | The address and port the `api` Role listens on. It defaults to loopback; set `0.0.0.0:8080` to accept connections from other hosts, such as in a container. |
| `worker.id` | `BIA_WORKER_ID` | `<HOSTNAME>-<pid>` | The name a Worker records on the Leases it holds, for diagnosis. It need not be unique. |
| `worker.concurrency` | `BIA_WORKER_CONCURRENCY` | `16` | How many Jobs a Worker runs at once, at least 1. The connection pool holds up to twice as many connections, plus one. |
| `worker.lease` | `BIA_WORKER_LEASE` | `30s` | How long a claim holds a Job, at least `1s`. A heartbeat extends it every third of its duration while the Job runs, so it bounds how long a crashed Worker's Jobs wait before another Worker takes them. |
| `worker.poll_interval` | `BIA_WORKER_POLL_INTERVAL` | `5s` | How often an idle Worker claims without a notification, which bounds how late a delayed Job, such as a sleep or a retry, starts. |
| `worker.shutdown_grace` | `BIA_WORKER_SHUTDOWN_GRACE` | `20s` | How long in-flight Jobs may keep running after SIGTERM or SIGINT. Jobs still running then are stopped and their Leases released to another Worker. Keep it below your orchestrator's kill timeout. |
| `scheduler.poll_interval` | `BIA_SCHEDULER_POLL_INTERVAL` | `1s` | How often a scheduler fires due cron Triggers and checks whether it leads. It bounds how late an occurrence fires and how long another scheduler takes over after the leader dies. |
| `run.retention_days` | `BIA_RUN_RETENTION_DAYS` | `90` | Days succeeded and cancelled Runs are kept after they finish, at least 1. Past it the leading scheduler deletes them with their Timelines and Jobs. Unfinished Runs are never deleted. |
| `run.failed_retention_days` | `BIA_RUN_FAILED_RETENTION_DAYS` | `90` | Days failed Runs are kept after they finish, at least 1, independently of `run.retention_days`. |
| `dead_letter.retention_days` | `BIA_DEAD_LETTER_RETENTION_DAYS` | `90` | Days a Message is kept once it is dead-lettered, at least 1. |
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

## First run setup

On a fresh install, `bia serve` (with the `api` Role) logs a Setup Link while no User can sign in:

```text
WARN no User can sign in yet; open the Setup Link, or run `bia setup` with its code setup_link=http://localhost:8080/setup?code=1f2e3d4c-…
```

Open it and choose a password of at least 12 characters, or run `bia setup` on a machine without the UI: it prompts for the code (or the whole link) and the password. Either way, setup creates the install's one User (ADR-0062). In the UI it also signs you in; `bia setup` returns no credential, so run `bia login` afterwards to sign the CLI in (ADR-0064, ADR-0068).

The code changes every time `bia serve` starts, so only a link from the current start works (`setup_code_expired` otherwise; a wrong code is `invalid_setup_code`), and once a User has a password setup refuses with `already_set_up`. The Setup Link is the one credential bia logs on purpose; the password is never logged. With several API processes, each logs its own link, which works only on that process.

To regain access after losing the password and the Tokens, restart with `bia serve --reset-admin`: it clears the User's password, deletes their passkeys, revokes every Token of the User (Sessions, Connected apps and manual Tokens, webhook Tokens included) and closes open sign-in approvals, then logs a new Setup Link. CLI logins and MCP clients have to be approved again.

## Access

Every API route except `GET /health`, `GET /ready`, `GET /openapi.json`, the discovery and `/oauth` routes that start a sign-in, `POST /v1/setup`, `POST /v1/session` and the passkey sign-in routes needs a credential, which is always a Token that acts for the User. Its Scope decides what it may call: `admin` everything, `agent` everything but publishing, activation and administering access, `webhook` only the webhook Trigger it is bound to (ADR-0008, ADR-0043). A Token's origin says how it was made, and the UI's Access screen lists all three, each revocable:

| Origin | How you get it | For |
|---|---|---|
| Session | Sign in to the UI with the password or a passkey. | You, in a browser. |
| Connected app | Approve an OAuth client (an MCP client, or `bia login`) on the consent screen. | Agents and the CLI. |
| Manual Token | Access, or `bia token create`. | CI, scripts and webhook senders. |

bia stores only a hash of each Token, so a lost value cannot be recovered, only replaced. Token values are never logged.

### Sessions

The UI signs in with the password (`POST /v1/session`) and keeps a Session in the `bia_session` cookie: `HttpOnly`, `SameSite=Lax`, `Secure` when `public_url` is HTTPS, never readable by scripts. A Session lasts 14 days without use and 90 days at most; signing out, `bia serve --reset-admin` or changing the password (`POST /v1/user/password`, which signs every other Session out but leaves Connected apps alone) ends it. After five failed sign-ins, each further attempt waits longer, up to a minute; nothing locks. The count is per User and peer address and held in memory by each API process, so behind a proxy the address is the proxy's (ADR-0064).

A request made with the cookie and an unsafe method must carry an `Origin` equal to the origin of `public_url` or `ui_url`, or it fails with `origin_mismatch`. A UI hosted apart from the API must be same-site with it (`ui.example.com` with `api.example.com`), or the browser drops the cookie; credentials are allowed for `ui_url`'s origin only, and `cors_origins` stay credential-less. `/mcp` ignores the cookie. See [`deploy-ui.md`](deploy-ui.md).

### Passkeys

Where the browser offers WebAuthn (HTTPS, or `localhost`), the user menu's Passkeys section registers passkeys, renames and removes them, and the sign-in screen offers "Sign in with a passkey". A passkey is an alternative to the password, not a second factor, and the password stays required to set up and recover the account. The WebAuthn RP ID is `public_url`'s host and the accepted origins are `public_url`'s and `ui_url`'s, so `public_url` must name a domain or `localhost`, not an IP address (the passkey routes answer `passkeys_unavailable` otherwise), and the UI must be served from the same host; a response from any other origin or RP ID is refused. Removing a passkey needs a Session that signed in within the last 10 minutes (`reauthentication_required` otherwise), and failed passkey sign-ins count towards the same throttle as passwords (ADR-0065).

### Connected apps

An MCP client pointed at `{public_url}/mcp` signs itself in: it reads the discovery documents, you approve it on the consent screen at `{ui_url}/oauth/consent`, and it gets a 1-hour access token it renews with a refresh token (rotated on every use, unused ones expire after 90 days). The consent screen shows the client's name, where it came from, the redirect address and the Scope it asks for (`agent` by default), which you may lower from `admin` to `agent`. Clients that identify themselves with an `https` URL show where they came from; clients that registered themselves through `POST /oauth/register` are marked "self-registered, unverified". Registrations and `bia login --device` requests are limited to 20 per peer address per hour, counted in memory per process. A token issued for `{public_url}/mcp` is accepted only at `/mcp`. Revoking the app in Access revokes every token it holds (ADR-0067).

### `bia login`

```sh
bia login               # opens the browser; approve "bia CLI (built in)"
bia login --device      # no browser here: enter the shown code at {ui_url}/device
bia logout              # revokes the login on the server and deletes the local copy
```

The login is kept in the OS keychain, keyed by the server URL, and in `credentials.json` (mode 0600, in a 0700 directory) in `$BIA_CONFIG_DIR`, else `$XDG_CONFIG_HOME/bia`, else `~/.config/bia` when there is no keychain. `BIA_KEYCHAIN=off` skips the keychain. Every API command and `bia mcp` use it and renew it on their own. `BIA_TOKEN` (or `token` in the config file) wins over a stored login, so CI is unaffected (ADR-0068, which amends ADR-0047). `bia login` asks for the `admin` Scope, which you may lower on the consent screen.

### Manual Tokens

For CI, scripts and webhook senders, create a Token in the Access screen, or with `bia token create --name ci --scope agent` (any login or Token with the `admin` Scope may). The value is shown once and does not expire. `bia token list` shows every credential with its origin, and `bia token revoke <id>` revokes one. These commands talk to the API at `BIA_URL`, and `bia token create` never takes a Token from arguments.

### Agents asking for publication

An `agent` cannot publish or activate. It calls the `request_approval` tool (or `POST /v1/approval-requests`), and you decide under Approvals in the UI or with `bia approval list|show|approve|reject` (ADR-0066).

## Secrets

Credentials a Step needs, such as an API token, are Secrets: set once, referenced in a Step's `params` as `{{ secrets.NAME }}`, resolved by the Worker just before the Action runs (ADR-0017, ADR-0049). Start the server with a master key, then set Secrets after `bia login`:

```sh
export BIA_MASTER_KEY="$(openssl rand -hex 32)"
bia serve &
printf '%s' "$UPTIME_API_TOKEN" | bia secret set API_TOKEN   # or: bia secret set API_TOKEN --file token.txt
bia secret list
bia secret delete API_TOKEN
```

The value is read from standard input or `--file` (one trailing newline is dropped), never from an argument, which `ps` would show. Setting a name again replaces its value. No API response, command output or MCP tool returns a value; `agent` credentials may list names, and only `admin` ones may set or delete. A Step that reads a Secret that does not exist fails with `unknown_secret`, naming it.

The Timeline shows a Step's `params` as written, so `Bearer {{ secrets.API_TOKEN }}` stays a Template there, and the interpreter replaces any resolved value with `***` in the Output and the error of the Attempt, which covers an Action that echoes its input. Masking matches the exact value, so an Action that encodes or splits it before echoing it is not covered. Dry Runs never read Secrets: `secrets.NAME` is a `<secret NAME>` placeholder there.

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

The API commands need the server's `url` and a credential: the login from `bia login`, or a manual `token` (see [Access](#access)), from the config file or the environment. `--json` on any command prints JSON instead of text, errors included. Exit status is 0 on success and 1 on failure, on error Diagnostics and on a failed Dry Run (ADR-0047).

```sh
bia login                                   # once; in CI set BIA_URL and BIA_TOKEN instead
bia workflow apply -f workflows/            # Drafts from JSON/YAML files and directories
bia workflow apply -f workflows/ --activate # also publish and activate (admin)
bia workflow validate -f price.yaml         # or: validate price 3 --target publish
bia workflow dry-run -f price.yaml --input input.json --mocks mocks.yaml
bia workflow publish price 3
bia workflow activate price 3
bia approval list
bia workflow diff price 2 3
bia run trigger price --input input.json
bia run get <id>
bia run list --workflow price --status failed
bia run cancel <id>
bia run retry <id>
```

## Signals

The first SIGTERM or SIGINT starts the graceful shutdown described under `worker.shutdown_grace`, and a leading scheduler hands leadership to another one at once. A second one exits at once, leaving unfinished Jobs to be claimed again once their Leases expire.
