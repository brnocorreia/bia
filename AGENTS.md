# AGENTS.md

bia runs cron schedules, durable queues and long-running workflows defined as **data** (JSON/YAML validated by a schema), executed by a Rust interpreter over Postgres, and operated through an HTTP API, a CLI, an MCP server and a static React UI.

## Read before working

- **Vocabulary**: [`GLOSSARY.md`](GLOSSARY.md). Name code, docs, Diagnostics and UI copy with its terms, and stay away from the words listed under _Avoid_.
- **Crate boundaries, processes, Run lifecycle**: [`docs/architecture.md`](docs/architecture.md). Read it before adding a crate, a dependency between crates, or a new table.
- **Definition format**: [`docs/dsl.md`](docs/dsl.md). Read it whenever you touch the DSL, the schema, validation or Expressions.
- **Decisions**: [`docs/adr/`](docs/adr/). Before making a design choice, check whether an ADR already settles it, and follow it.

## Working on an issue

Issues live in Linear (project **bia**). One issue is one PR.

1. Check the issue's label. An issue labelled `needs-refinement` is not ready. Stop and ask for a grilling session, using the issue's context as the starting point.
2. Branch as `<linear-id>-<slug>` (e.g. `BIA-12-queue-claim`).
3. Implement against the issue's acceptance criteria. Choosing the implementation is your job; the issue describes outcomes.
4. When you have to make a design decision the issue and the ADRs leave open, and it is hard to reverse, record it as a new ADR in the same PR (format: one short paragraph covering context, decision and why; numbered after the highest existing ADR).
5. Every behaviour change ships with tests (see Testing). Test-first is encouraged.
6. Before opening the PR, run the formatter, clippy and the full test suite. All must pass. Use the commands in `Justfile`/CI as the source of truth.
7. Write commits as Conventional Commits. The PR title carries the Linear id, and the body follows `.github/pull_request_template.md`.
8. If a term in `GLOSSARY.md` turned out wrong or missing, fix `GLOSSARY.md` in the same PR.

## Code rules

- **`bia-core` has no IO**: no database, network, filesystem or clock reads. Everything that touches the world lives in `bia-store`, `bia-engine`, `bia-actions` or `bia-server`.
- **Dependency direction**: `bia-actions` depends only on `bia-core`, and `bia-engine` receives the Action Registry by injection. Keep these edges as they are, because they are what keeps rebuilds small.
- **Errors**: `thiserror` in library crates; `anyhow` only in `bia-cli`. Action errors are classified `retryable` or `fatal`.
- **SQL**: `sqlx` compile-time-checked queries; commit the regenerated `.sqlx/` offline data with any query change. Migrations are append-only.
- **Transactions**: a Step's Checkpoint, Job completion and next-Job enqueue commit together (ADR-0010).
- **Time**: read time only from the injected `Clock`, and pass it into queries instead of using Postgres `now()` (ADR-0022).
- **Secrets**: resolved values stay out of logs, Outputs, Timelines, errors and test snapshots.
- **Diagnostic codes** are a public contract for agents: add new codes freely, and rename or remove one only with an ADR.
- **Dependencies**: use the mainstream, well-maintained crate or package for the job, and record why in the PR description when adding one.
- Everything in the repository is written in English.

## Testing

Test observable behaviour at the highest seam that reaches it. There are five seams; pick the first that fits:

1. **HTTP API** (default): an in-process server against the `docker compose` Postgres, driven through `bia-client`. Use it for anything crossing crates: Definitions, Runs, Timelines, Events, Queues, webhooks, Scopes. MCP tests only check that each tool maps to its endpoint and respects Scopes.
2. **`bia-core` public API**: validation, Diagnostics and Expressions, with `insta` snapshots for Diagnostics and the generated JSON Schema.
3. **`Action` trait**: one builtin Action at a time, with `wiremock` for outbound HTTP.
4. **Store/engine concurrency**: `#[sqlx::test]` for races the API cannot reproduce deterministically (claims, Lease expiry, scheduler leadership).
5. **UI**: Playwright against a real server; Vitest for pure TS such as the Definition ⇄ graph mapping.

Advance time with the test `Clock`, never with real sleeps.

## Agent skills

### Issue tracker

Linear, project **bia** (via Linear MCP). See `docs/agents/issue-tracker.md`.

### Triage labels

Default five roles plus `needs-refinement`. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: `GLOSSARY.md` + `docs/adr/` at the root. See `docs/agents/domain.md`.
