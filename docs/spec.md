# Spec: bia, a workflow-as-data engine

## Problem Statement

I want cron jobs, durable queues and long-running, reliable workflows (in the spirit of Cloudflare's Cron Triggers, Queues and Workflows) on infrastructure I control. Existing durable-execution tools (Temporal, Restate, Inngest, Cloudflare Workflows) make every pipeline **code**: someone has to write, compile and deploy a program, and deterministic replay constrains what that code may do. Low-code tools (n8n and similar) are easy to click together but are not agent-friendly, are weak on durability semantics, and do not expose a clean, schema-validated contract.

That makes the pipelines hard for an AI agent to own. Generating code that compiles and deploys is fragile, the feedback on mistakes is unstructured, and nothing limits what an agent's pipeline can do once it runs. I want an agent, a UI or a plain YAML file in git to be able to create, validate, try out and operate pipelines through one contract, while keeping the durability guarantees (retries, timeouts, sleeping for days, waiting for humans or external events, surviving crashes) that make long-running automation trustworthy.

## Solution

bia (Background Integrations & Automations) is a self-hosted engine, written in Rust and backed only by Postgres, where every pipeline is a **Definition**: a JSON (or YAML) document written in a small DSL and validated against a published JSON Schema. An interpreter executes Definitions as a state machine, checkpointing every transition in Postgres, so a crashed Run resumes from its Checkpoint with no code replay.

A single binary, `bia`, provides everything:
- `bia serve` runs the HTTP API, the MCP server, webhooks, Workers and the scheduler, in any combination of Roles.
- The rest of the CLI operates the system through the API.

A static web UI shows Runs and their Timelines, edits Definitions with live Diagnostics and Dry Runs, and later builds them visually.

Agents are first-class citizens:
- They get the full schema and Action catalogue.
- Validation returns structured Diagnostics they can fix themselves.
- They can Dry Run with Mocks.
- They can author Drafts, but publishing a Version and activating it stays a human decision, enforced by Token Scopes.

The first milestone that proves the idea end to end is the **anchor scenario**: an agent, via MCP, authors an uptime-check Workflow, fixes a Diagnostic, dry-runs it, and asks a human to publish it. Cron then runs it, and the agent inspects and retries Runs. The roadmap then grows the full control-flow vocabulary, triggers, a visual editor, a broad Action catalogue, extensibility (Action Servers, WASM, sandboxed exec/containers) and production hardening.

## User Stories

### Authoring Definitions

1. As a workflow author, I want to write a Definition in JSON, so that I have one canonical, schema-validated format.
2. As a workflow author, I want to write the same Definition in YAML through the CLI, so that hand-authoring and code review are pleasant.
3. As a workflow author, I want Steps to be an ordered list with control-flow Blocks containing nested Steps, so that I never wire Steps together by id or create cycles by accident.
4. As a workflow author, I want each Step to declare an Action, Params, a retry policy and a timeout, so that every unit of work has explicit reliability settings.
5. As a workflow author, I want to reference the Run's input and earlier Steps' Outputs in Params with `{{ … }}` Templates, so that data flows between Steps without code.
6. As a workflow author, I want a Template that is a single expression to keep its type (object, number, list), so that I can pass structured data, not just strings.
7. As a workflow author, I want conditions (`when`, `until`, `if`) to be plain CEL, so that branching logic is typed, side-effect free and predictable.
8. As a workflow author, I want an optional `if` on any Step that records it as skipped when false, so that simple conditional Steps don't need a whole Block.
9. As a workflow author, I want to declare an input schema for my Workflow, so that every Run's input is validated before anything executes.
10. As a workflow author, I want durations written as `30s`, `5m` or `3d`, so that timeouts, sleeps and backoff read naturally.
11. As a workflow author, I want to reference Secrets as `secrets.NAME` in Params, so that credentials never appear in Definitions.
12. As a workflow author, I want every Definition to carry a `schema_version`, so that the DSL can evolve without breaking stored Workflows.
13. As a workflow author, I want Step ids to be unique across the whole Definition, nested Blocks included, so that `steps.<id>.output` is never ambiguous.

### Validation and Diagnostics

14. As a workflow author, I want validation to report every problem at once as Diagnostics with a JSON path, a stable code, a severity, a message and a suggestion, so that I can fix everything in one pass.
15. As an AI agent, I want Diagnostic codes to be stable identifiers, so that I can match on them and self-correct reliably.
16. As a workflow author, I want validation to catch duplicate ids, references to Steps that haven't run yet, Expressions that don't compile, and Params that don't match the Action's schema, so that mistakes surface before a Run starts.
17. As a workflow author, I want a reference to an unknown Queue or Workflow to be a warning on a Draft and an error at publish time, so that I can draft incrementally but never publish something broken.
18. As a workflow author, I want "did you mean…?" suggestions on typos, so that fixing them is instant.
19. As a workflow author, I want validation to reject Definitions that exceed configured limits (iterations, nesting, Steps), so that a Workflow can't run away.
20. As an operator, I want publishing to fail if a Version uses an Action disabled on this server, so that dangerous capabilities can't sneak in.

### Versions and lifecycle

21. As a workflow author, I want saving a Workflow to create a new immutable Version, so that I get history, diffs and audit for free.
22. As a workflow author, I want Versions to be Drafts until published, so that I can iterate without anything running.
23. As an operator, I want to choose which Published Version is the Active Version, so that I control exactly what Triggers run.
24. As an operator, I want to roll back by pointing the Active Version at an older Version, so that recovering from a bad change is one action.
25. As an operator, I want a Run to stay on the Version it started with, so that activating a new Version never corrupts in-flight Runs.
26. As a workflow author, I want to diff two Versions, so that I can review what changed before publishing.

### Triggers

27. As a workflow author, I want cron Triggers with timezone and optional jitter, so that schedules run when I mean them to.
28. As an operator, I want a cron overlap policy (`skip` by default, `queue`, `allow`), so that a slow Run doesn't pile up duplicates.
29. As an operator, I want a catch-up policy (`latest` by default, `none`, `all` bounded), so that downtime is handled the way I choose.
30. As an operator, I want to run several scheduler replicas without double firing, so that scheduling survives a node loss.
31. As a workflow author, I want a manual Trigger, so that I can start a Run from the UI, CLI, API or an agent with a given input.
32. As a workflow author, I want webhook Triggers at `/hooks/{workflow_id}/{trigger_id}` with a per-Trigger secret and optional HMAC verification, so that external systems can start Runs safely.
33. As a workflow author, I want queue Triggers that start Runs from batches of Messages, so that load is absorbed and processed at a controlled pace.
34. As a workflow author, I want event Triggers, so that one system's Event can start another Workflow.
35. As a workflow author, I want a `workflow_completed` Trigger, so that I can chain Workflows.
36. As an operator, I want a per-Workflow `max_concurrent_runs` limit, so that one Workflow can't starve the others.

### Durable execution

37. As an operator, I want every Step transition checkpointed in Postgres in the same transaction as the Job bookkeeping, so that a crash never loses or half-applies progress.
38. As an operator, I want a crashed Worker's in-flight Steps to be picked up by another Worker once its Lease expires, so that Runs always make progress.
39. As a workflow author, I want per-Step retries with fixed or exponential backoff applied only to retryable errors, so that transient failures heal and permanent ones fail fast.
40. As a workflow author, I want per-Attempt timeouts that cancel the Action, so that hung calls don't block a Run forever.
41. As an integrator, I want each Step execution to carry a stable Idempotency Key forwarded to external systems, so that at-least-once delivery doesn't double-charge or double-send.
42. As a workflow author, I want `flow.sleep` and `flow.sleep_until` to wait for days without holding a Worker, so that long waits cost nothing.
43. As an operator, I want to cancel a Run and have in-flight Actions receive cancellation, so that stopping something actually stops it.
44. As an operator, I want to resume a failed Run from the failed Step on its own Version, so that I don't redo completed work.
45. As an operator, I want to retry a Run as a brand-new Run, so that I can start clean when needed.
46. As an operator, I want an Output larger than the configured maximum to fail its Step clearly, so that the database never bloats silently.

### Control flow

47. As a workflow author, I want `flow.branch` (if/else) and `flow.switch` (cases), so that Runs take different paths.
48. As a workflow author, I want `flow.parallel` with `all`, `any` or `n_of_m` joins, so that independent work runs concurrently.
49. As a workflow author, I want parallel and foreach to fail fast by default and to support `all_settled`, so that I choose between quick failure and collecting every result.
50. As a workflow author, I want `flow.foreach` with `concurrency`, `max_iterations` and `item`/`index` in scope, so that I can process lists safely.
51. As a workflow author, I want `flow.loop_until` with an iteration limit, so that I can poll or iterate until a condition holds.
52. As a workflow author, I want `flow.try` with `catch`, so that I can handle a failure locally and continue.
53. As a workflow author, I want a Run-level `on_error` list of compensation Steps, so that I can notify or undo when a Run fails.
54. As a workflow author, I want `flow.fail` and `flow.succeed`, so that I can end a Run explicitly with a clear status.
55. As a workflow author, I want `flow.subworkflow` in sync or fire-and-forget mode with a depth limit, so that I can compose Workflows.
56. As a workflow author, I want rate-limit, debounce and dedupe Steps, so that I can control throughput and suppress duplicates (semantics to be refined).

### Events and humans

57. As a workflow author, I want `flow.wait_for_event` with a timeout, so that a Run can pause until something external happens.
58. As an operator, I want to send an Event directly to one Run, so that I can unblock it from the UI or CLI.
59. As an integrator, I want to send an Event by name and Correlation Key without knowing the Run id, so that my system can resume the right Runs.
60. As a workflow author, I want `event.emit`, so that Workflows can signal each other.
61. As a workflow author, I want `human.approval`, `human.form` and `human.choice`, so that people can make decisions inside a Run.
62. As an operator, I want an inbox of pending Human Tasks in the UI, so that I can answer them in one place.

### Queues

63. As an operator, I want to create Queues explicitly with retry, visibility timeout, batch size and Dead-Letter Queue settings, so that every Queue is configured on purpose.
64. As a workflow author, I want `queue.publish` and `queue.publish_batch` with optional delay, so that Workflows can hand work to other consumers.
65. As an operator, I want exhausted Messages to land in the Dead-Letter Queue, so that failures are kept for inspection instead of lost.
66. As an operator, I want to browse Messages and replay or purge dead letters in the UI, so that I can recover from incidents.
67. As an operator, I want to declare Queues in YAML alongside Workflows, so that the whole setup lives in git.

### Agent interface

68. As an AI agent, I want `GET /schema` to return the DSL JSON Schema plus the full Action catalogue (names, descriptions, param and output schemas, examples), so that I can author valid Definitions without guessing.
69. As an AI agent, I want an MCP server exposing tools that mirror the API, so that I can operate bia from any MCP client.
70. As an AI agent, I want to create and replace Drafts, validate, Dry Run, trigger Runs, read Runs and Timelines, list failures, cancel, retry and send Events, so that I can own a Workflow day to day.
71. As an operator, I want agent Tokens to be unable to publish Versions or change the Active Version, so that an agent's mistakes can't reach production without me.
72. As an AI agent, I want a Dry Run where pure Actions execute for real, side-effecting Actions return Mocks I supply (or examples), and waits resolve instantly, so that I can test a Definition safely and quickly.
73. As an AI agent, I want the Dry Run result to look exactly like a real Timeline, so that I read both the same way.
74. As an AI agent, I want the Timeline to show every Attempt's inputs, Output, error and duration, so that I can diagnose failures myself.
75. As a local user, I want `bia mcp` as a stdio MCP server proxying to a running bia, so that Claude Code or Claude Desktop can use it directly.

### CLI and operations

76. As an operator, I want one binary with `bia serve --role api|worker|scheduler|all`, so that I can run everything on one box or scale Roles independently.
77. As an operator, I want `bia migrate`, so that schema upgrades are an explicit step.
78. As an operator, I want `bia workflow apply -f` to accept files or directories of JSON/YAML, so that GitOps is natural.
79. As an operator, I want CLI commands to validate, dry-run, publish, activate, diff, trigger, inspect, cancel and retry, so that everything the UI does is scriptable.
80. As an operator, I want readable Diagnostics in the terminal and a `--json` flag everywhere, so that both humans and scripts can use the CLI.
81. As an operator, I want configuration via environment variables and a config file, so that bia fits any deployment.
82. As an operator, I want graceful shutdown that releases Leases, so that deploys don't stall Runs.
83. As an operator, I want finished Runs pruned after 90 days by default, with separate retention for failures and dead letters, so that the database stays bounded.
84. As an operator, I want global limits (`max_steps_per_run`, `max_iterations`, `max_subworkflow_depth`, `max_output_bytes`) to be configurable, so that I can tune safety for my workloads.

### Auth and Secrets

85. As an operator, I want to set up the first admin from a one-time Setup Link or the CLI, so that a fresh install is usable and secure.
86. As an operator, I want Tokens with `admin`, `agent` and `webhook` Scopes, so that each caller gets only what it needs.
87. As an operator, I want to manage Secrets via API, CLI and UI without ever being able to read their values back, so that leaks through the interface are impossible.
88. As an operator, I want Secrets encrypted at rest with a master key from the environment, so that a database dump doesn't expose them.
89. As an operator, I want resolved Secret values masked in logs and kept out of Outputs and Timelines, so that observability never leaks credentials.
90. As an operator, I want to route Secret names to external managers (env, AWS Secrets Manager, later Vault, Infisical, 1Password) with the internal store as fallback, so that bia fits my existing secret management.
91. As an operator, I want CORS origins configurable, so that a UI hosted elsewhere can call the API.

### UI

92. As an operator, I want a UI that is a static site deployable to Cloudflare Pages or S3 + CloudFront, or embedded in the binary, so that hosting it is trivial.
93. As an operator, I want the UI to read its API URL from a runtime config file, so that one build works in every environment.
94. As an operator, I want to log into the UI with a Token, so that it uses the same authorisation as everything else.
95. As an operator, I want to list Workflows, see their Versions, diff them, and publish, activate or roll back, so that I manage lifecycle visually.
96. As an operator, I want a filterable list of Runs and a Timeline view with Attempts, inputs, Outputs, errors and durations, so that I understand what happened.
97. As an operator, I want to trigger, cancel, resume and retry Runs from the UI, so that I can operate without the CLI.
98. As an operator, I want Run status to update live, polling first and server-sent events later, so that I can watch Runs progress.
99. As a workflow author, I want a JSON/YAML editor with schema-aware completion and server Diagnostics shown inline, so that I author confidently in the browser.
100. As a workflow author, I want to Dry Run from the editor with input and per-Step Mocks, so that I can test before saving.
101. As an operator, I want screens to manage Secrets and Tokens, with Token values shown once, so that administration is complete in the UI.
102. As a workflow author, I want to see a Version as a graph with a Run's Step statuses painted on it, so that complex Workflows are easy to follow.
103. As a workflow author, I want to build and edit Workflows visually, kept in sync both ways with the text Definition, so that I can choose the editing style per task.
104. As a workflow author, I want forms for Step Params generated from each Action's schema, so that I don't need to memorise Params.
105. As a workflow author, I want scope-aware autocomplete for Expressions, so that writing CEL is easy (to be refined).

### Action catalogue

106. As a workflow author, I want `http.request` with method, headers, query, body, auth, timeout and status-based error classification, so that most integrations need nothing else.
107. As a workflow author, I want `transform.*` Actions for collections, JSON, text formats, regex, hashing, encoding, ids, dates and maths, so that I can reshape data without code.
108. As a workflow author, I want `llm.complete`, `llm.chat` and `llm.structured` across Anthropic, OpenAI-compatible and Ollama providers, defaulting to current Claude models, so that Workflows can use LLMs.
109. As a workflow author, I want `llm.classify`, `extract`, `summarize`, `translate` and `embed`, so that common LLM tasks are one Step.
110. As a workflow author, I want `llm.tool_loop` that runs an agent loop inside one Step with an allowlist of Actions as tools and an iteration cap, so that agentic steps are bounded and observable.
111. As a workflow author, I want `mcp.call_tool`, so that Workflows can use external MCP servers.
112. As a workflow author, I want `sql.*`, `kv.*`, `email.send`, `notify.*`, `storage.*`, `fs.*`/`archive.*`, `log.*`/`metric.*`/`audit.*`, HTTP extras, `net.*` and `web.*` Actions, so that most automation needs are covered out of the box.
113. As a workflow author, I want connectors for popular services, plus document, media, multimodal-AI and extra data-store Actions, so that the catalogue keeps growing (each to be refined).
114. As an operator, I want heavy Action categories behind build features, so that I compile only what I use.

### Extensibility and dangerous Actions

115. As an Action developer, I want to register Actions from an external HTTP Action Server in any language, so that I can extend bia without writing Rust (contract to be refined).
116. As an Action developer, I want a small reference SDK for Action Servers, so that writing one is quick (to be refined).
117. As an Action developer, I want sandboxed WASM Actions, so that extensions can run in-process safely (to be refined).
118. As an operator, I want to enable or disable Action categories and set allowlists in server config, so that I decide what my bia may do.
119. As an operator, I want Worker pools restricted to certain Actions, so that risky Actions only run on hardened hosts.
120. As an operator, I want `exec.*`, `container.*` and `ssh.*` disabled by default, policy-gated when enabled, and publishable only by admin, so that powerful Actions can't be abused by agents.

### Observability and hardening

121. As an operator, I want structured `tracing` logs correlated by Run id from day one, so that I can debug in production.
122. As an operator, I want OpenTelemetry traces and Prometheus metrics, so that bia plugs into my monitoring.
123. As an operator, I want proof that killing Workers, expiring Leases and failing over the scheduler loses no Runs, so that I can trust multi-replica deployments.
124. As an operator, I want large Outputs offloaded to blob storage by reference, so that big payloads don't break the database (to be refined).
125. As an operator, I want stored Definitions migrated across `schema_version`s, so that upgrades never strand old Workflows.
126. As an operator, I want versioned release binaries and a container image, so that installing bia is easy.

### Contributors

127. As a contributor (human or agent), I want one issue to map to one PR with outcomes and acceptance criteria rather than steps, so that I can choose the best implementation.
128. As a contributor, I want a glossary, an architecture overview, a DSL reference and ADRs, so that I build in the project's language and respect past decisions.
129. As a contributor, I want hard-to-reverse decisions I make during an issue recorded as ADRs in the same PR, so that the reasoning survives.
130. As a contributor, I want CI to enforce formatting, clippy, tests and dependency policy, so that quality doesn't depend on discipline.

## Implementation Decisions

### Architecture

- **Definitions are data, interpreted from Checkpoints.** There is no code replay. Durability comes from transactional state transitions (ADR-0001).
- **Postgres is the only store.** It holds the registry, Run state, Timelines, Queues, Jobs, Schedules, Events, Secrets and Tokens. Claims use `SKIP LOCKED`, Leases a `locked_until`, scheduler leadership an advisory lock, and wake-ups `LISTEN/NOTIFY` (ADR-0002).
- **One binary with Roles,** correct with N replicas of any Role from the first commit. Production hardening of that comes later (ADR-0005).
- **Actions run in-process in Workers** behind a transport-agnostic `Action` trait. A documented evolution path leads to Action Servers, WASM, restricted Worker pools and process/container isolation (ADR-0006).
- **The HTTP API is the only entry point.** The UI, CLI and MCP all go through it. MCP is served over streamable HTTP, and `bia mcp` is a stdio proxy. Only `serve` and `migrate` touch Postgres (ADR-0007).
- **Time comes from an injected `Clock`.** Queries compare against Clock-supplied time, not Postgres `now()` (ADR-0022).

### Modules (Cargo workspace, ADR-0013)

- **`bia-core`** (no IO): DSL types, JSON Schema generation, JSON/YAML parsing, CEL Expressions and Templates, layered validation into Diagnostics, the `Action` trait and registry interface, and the `Clock` trait.
- **`bia-store`**: migrations, repositories, and Queue and Job primitives (claim, Lease, ack, retry with backoff, delay, dead-letter, notify).
- **`bia-engine`**: the interpreter (advance a Run from its Checkpoint and Cursor), the Worker loop, and the scheduler (cron, catch-up, retention, internal jobs). It receives the Action Registry by injection.
- **`bia-actions`**: builtin Actions, one module per category, with heavy categories behind Cargo features. It depends only on `bia-core`.
- **`bia-server`**: the axum API, OpenAPI, Scope enforcement, webhooks, MCP, and the optional embedded UI.
- **`bia-client`**: the typed API client.
- **`bia-cli`**: the `bia` binary, which wires the Action Registry.
- **`ui/`**: a static SPA (Vite, React, TypeScript, TanStack Router and Query, Tailwind + shadcn/ui, Monaco, React Flow). TS types are generated from OpenAPI, and the API URL comes from a runtime config file.

### DSL

- **Tree of nested Blocks**, never id references between Steps. The Cursor is a path through Blocks (ADR-0003).
- **CEL is the only Expression language.** Condition fields take raw CEL. Params use Templates, which are type-preserving when the whole string is one expression (ADR-0004).
- **Expression scope** is `input`, `steps.<id>.output` (only for Steps that already ran), `trigger`, `run`, `item`/`index` and `error`. `secrets.NAME` is allowed in Params only. There is no mutable run context (ADR-0011).
- **Step ids** are unique across the whole Definition.
- **JSON is canonical.** The server accepts only JSON, and the CLI converts YAML.
- **Actions are named `category.operation` with no per-Action version.** Breaking changes go through `schema_version` migrations, and param-schema changes are additive during 0.x (ADR-0016).
- The target shape is documented in the DSL reference doc, which must be kept in sync with the generated schema.

### Lifecycle and access

- **Versions are immutable** and either Draft or Published. A Workflow has one Active Version, Runs are pinned to their Version, and rollback means changing the Active Version (ADR-0009).
- **Tokens are hashed and carry a Scope:** `admin`, `agent` or `webhook`. `agent` cannot publish or activate. There is a single user, with no multi-tenancy or namespaces (ADR-0008).
- **Dangerous Actions** are disabled by default, policy-gated when enabled, and admin-only to publish (ADR-0014).

### Execution semantics

- **Delivery is at-least-once everywhere.** The Idempotency Key is `run_id:step_id:iteration_path`. A Step's Checkpoint, its Job completion and the next Job's enqueue commit in one transaction (ADR-0010).
- **Action errors are classified `retryable` or `fatal`,** and only `retryable` consumes retries. Retries are per Step. `flow.try`/`catch` handles errors locally, and `on_error` runs saga-style compensation.
- **Parallel and foreach** fail fast by default, with `all_settled` available. Joins are `all`, `any` or `n_of_m`.
- **Global limits** are configurable, and `max_output_bytes` defaults to 256 KiB (ADR-0012).
- **Sleeps and backoff** are delayed Jobs, so no Worker is held while waiting.
- **Cancellation** is cooperative, through a token passed to Actions.
- **Cron:** overlap defaults to `skip`, catch-up to `latest`. A per-Workflow `max_concurrent_runs` limit is available.
- **Retention:** 90 days by default for finished Runs, with an independent retention for failures and dead letters (ADR-0021).

### Triggers, Queues and Events

- **Queues are explicit resources.** An unknown-Queue reference is a warning on a Draft and an error at publish. Webhooks have fixed paths with per-Trigger secrets and optional HMAC (ADR-0015).
- **Events are delivered two ways:** directly to one Run, or by name plus Correlation Key to every waiting Run. Early Events are dropped for now (ADR-0018).
- **Human Tasks** are built on `wait_for_event`, so there is no separate approval primitive.

### Agent contracts

- **`GET /schema`** returns the DSL schema plus the full Action catalogue.
- **Diagnostics** contain path, code, severity, message and suggestion. Codes are a public contract: they may be added, and renaming or removing one requires an ADR.
- **MCP tools** mirror the API, with descriptions written for agents. Publishing and activation stay admin-only.
- **Dry Run** executes pure Actions for real and Mocks side-effecting ones, using caller-supplied Mocks or the Action's examples. Waits resolve instantly. The result is an unpersisted Timeline marked as a Dry Run.

### Secrets

- A `SecretProvider` trait. The internal store uses XChaCha20-Poly1305 under a master key from the environment.
- Prefix routing to external providers. Values are never returned by the API and are masked everywhere (ADR-0017).

### Specific components

- **`llm.tool_loop`** runs its loop inside one Step, records each iteration as a Timeline sub-entry, and is checkpointed per Step. Providers sit behind a trait (ADR-0019).
- **Extensibility order:** HTTP Action Servers first, WASM afterwards (ADR-0020).

## Testing Decisions

A good test exercises **observable behaviour through a public interface** and survives refactors of everything behind it. Assert on API responses, Timelines, Diagnostics and Action Outputs, never on private functions, table layouts or call sequences. Choose the highest seam that can reach the behaviour. There are five seams:

1. **HTTP API (default).** An in-process server against the Postgres started by `docker compose`, driven through `bia-client`. It covers everything that crosses crates: lifecycle, Runs, Timelines, Events, Queues, webhooks, Scopes, Dry Runs and the anchor scenario end to end. MCP tests check only that each tool maps to its endpoint and enforces Scopes.
2. **`bia-core` public API.** Validation, Diagnostics and Expressions, which have too many combinations to drive over HTTP. Diagnostics and the generated JSON Schema are `insta` snapshots.
3. **`Action` trait.** Each builtin Action in isolation, with `wiremock` standing in for external services. This is the existing injection seam.
4. **Store/engine concurrency.** `#[sqlx::test]` for races the API cannot reproduce deterministically: many claimers with no double-processing, Lease expiry and recovery, single scheduler firing across replicas.
5. **UI.** Playwright against a real server for critical flows. Vitest for pure TypeScript such as the Definition ⇄ graph mapping, including round-trip property tests.

Time-dependent behaviour (sleeps, cron, catch-up, Lease expiry, retention) is tested by advancing the manual test `Clock`, never with real sleeps.

There is no prior art yet, since this is a greenfield repository. The first issues establish the patterns that later issues follow.

## Out of Scope

- Multi-user accounts, SSO, multi-tenancy and namespaces.
- SQLite or any non-Postgres backing store; Redis, NATS or Kafka as internal infrastructure (they may appear only as Actions).
- Deterministic code replay or Workflow-as-code SDKs.
- Server-side rendering for the UI.
- Per-Action versioning.
- Mutable run context (`ctx.set`/`ctx.get`).
- Buffering of Events that arrive before a waiter (a recorded gap, revisited later).
- Automatic reverse compensation for sagas.
- Items marked "to be refined" in the user stories: their direction is set here, but their contracts are decided in a later grilling before implementation.

## Further Notes

- Name: **bia**, Background Integrations & Automations; tagline _Bia Isn't Airflow_. License: MIT OR Apache-2.0.
- Everything in the repository is written in English.
- Delivery is organised in milestones M0–M8. The issue-by-issue plan, with dependencies, lives in the backlog doc, and Linear becomes the source of truth once the issues exist. M2 closes when the anchor scenario passes end to end.
- Issues labelled `needs-refinement` still carry rich context (problem, constraints, options, open questions) so that their grilling session can start immediately.
