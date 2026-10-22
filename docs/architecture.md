# Architecture

How bia's pieces fit together. Terms are defined in [`GLOSSARY.md`](../GLOSSARY.md), and the reasons behind each choice live in [`docs/adr/`](adr/).

```
 UI (static SPA) ──────┐
 Agent (MCP) ──────────┼─▶ bia-server (axum: REST + OpenAPI + /mcp + /hooks)
 CLI (bia-client) ─────┘          │
                                  ▼
                       bia-core (DSL, schema, CEL, validation)
                                  │
       ┌──────────────── bia-store (Postgres) ────────────────┐
       │ workflows · versions · runs · timeline · jobs ·       │
       │ queues · messages · schedules · events · secrets ·    │
       │ tokens                                                │
       └──────────────────────────┬────────────────────────────┘
                                  │ SKIP LOCKED claims, Leases, LISTEN/NOTIFY
               bia-engine (Workers: interpreter + Checkpoints;
                           scheduler: cron, catch-up, retention)
                                  │
                       Action Registry (bia-actions, later Action Servers / WASM)
```

## Crates

| Crate | Responsibility | May depend on |
|---|---|---|
| `bia-core` | DSL types, JSON Schema (`schemars`), JSON/YAML parsing, CEL Expressions and Templates, validation → Diagnostics, `Action` trait and registry interface. **No IO.** | — |
| `bia-store` | `sqlx` + migrations, repositories, Queue and Job primitives (claim, ack, retry, dead-letter, Leases, notify). | core |
| `bia-engine` | Interpreter (advance a Run from its Checkpoint), Worker loop, scheduler loop, Trigger dispatch. Receives the Action Registry by injection. | core, store |
| `bia-actions` | Builtin Actions, one module per category; heavy categories behind Cargo features (`actions-llm`, `actions-sql`, …). | core |
| `bia-server` | axum HTTP API, OpenAPI (`utoipa`), auth/Scopes, webhooks, MCP over streamable HTTP (`rmcp`), optional embedded UI (`ui` feature). | core, store, engine |
| `bia-client` | Typed HTTP client for the API. | core |
| `bia-cli` | The `bia` binary (`clap`): `serve`, `migrate`, `mcp`, `workflow …`, `run …`, `queue …`, `secret …`, `token …`. Wires the Action Registry. | everything |

The dependency rule keeps rebuilds small: an edit to an Action recompiles `bia-actions` and the binary only. `deny.toml` enforces these edges: `just deny` fails when a crate depends on one it may not. A crate is split only when it earns it, and the split is recorded in an ADR.

## Time

Engine, scheduler and store read time from an injected `Clock` (ADR-0022), and wait on it for in-process timers such as Attempt timeouts (ADR-0034), so tests drive sleeps, timeouts, cron and Lease expiry deterministically. The trait and the manual test Clock (behind `bia-core`'s `testing` feature) live in `bia-core`; the system Clock lives in `bia-store`. Store functions take `now` as a parameter rather than a Clock.

## Processes

- `bia serve --role all` (dev) or any mix of `api`, `worker` and `scheduler` replicas (ADR-0005), e.g. `--role api,worker`. Roles not built yet start and do nothing.
- `bia migrate` applies migrations.
- Both read their configuration from a TOML file under `BIA_*` environment variables (ADR-0037, [`configuration.md`](configuration.md)).
- A Worker runs claimed Jobs as concurrent tasks, heartbeats their Leases, and on SIGTERM stops claiming, waits a grace period, then releases the Leases of what did not finish (ADR-0036).
- `bia mcp` is a stdio → HTTP proxy for local MCP clients.
- Every other subcommand is an API client (ADR-0007).

## Run lifecycle

1. A Trigger fires (Schedule, Message, webhook, manual, Event), and the server creates a Run pinned to the Active Version, plus a Job to advance it.
2. A Worker claims the Job under a Lease, loads the Checkpoint and executes the Step at the Cursor: it resolves Templates and Secrets, calls the Action with the Idempotency Key and enforces the timeout.
3. In one transaction, the Worker writes the Attempt to the Timeline, updates the Checkpoint and Cursor, completes the Job and enqueues the next Job (or a delayed Job for `sleep`/retry backoff, or nothing while waiting for an Event). Each Job makes one transition: starting the Run (checking its input against the `input_schema`) or one Attempt of one Step (ADR-0032). The Timeline entry shape is ADR-0033.
4. Run statuses: `pending → running → (sleeping | waiting) → running → succeeded | failed | cancelled`.

Cancellation sets a flag on the Run that Postgres announces to Workers, which also re-read it on every Lease heartbeat. The Worker running the Run's Attempt fires its cancellation token, the in-flight Action stops cooperatively, and the Run ends `cancelled` without a later Step running; a sleeping Run's wake-up is brought forward so it ends at once (ADR-0036).

## Interfaces for agents

- `GET /schema`: DSL JSON Schema plus the Action catalogue (name, description, param schema, output schema, examples).
- MCP tools mirror the API: `list_actions`, `get_schema`, `validate_workflow`, `create_draft`, `dry_run`, `diff_versions`, `trigger_run`, `get_run`, `list_runs`, `list_failures`, `retry_run`, `cancel_run`, `send_event`, `list_queues`. Publishing and activation are admin-only (ADR-0008).

## UI

A static SPA in `ui/`, deployed to any static host or embedded in the binary (ADR-0013). It is built in phases: Run observation and operation → schema-aware JSON/YAML editor with Diagnostics and Dry Run → visual editor (React Flow) synced both ways with the Definition → param forms generated from Action schemas.

## Where it evolves

- Action isolation path: ADR-0006.
- Multi-replica hardening, OpenTelemetry export, blob storage for large Outputs, external Secret Providers: hardening milestone.
