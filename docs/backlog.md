# Backlog plan

The plan the Linear project **bia** is created from (https://linear.app/brnocorreia/project/bia-e17fd941b01e). IDs here (`B01`…) are local placeholders that map to Linear ids at creation time. Once issues exist, Linear is the source of truth, and this file stays only as the historical plan.

## Conventions

- One issue = one PR. Issues describe **outcomes and constraints**, not implementation steps, and the implementing agent decides how.
- Issue body sections: **Context** (why, with links to ADRs/GLOSSARY/docs) · **Goal** · **Scope / Non-goals** · **Acceptance criteria** (behavioural, testable) · **Notes / open questions** · **Depends on**.
- Labels: `ready-for-agent` or `needs-refinement`; one `area:*` (`core`, `store`, `engine`, `actions`, `server`, `cli`, `ui`, `infra`, `docs`); `type:feature` or `type:chore`.
- `needs-refinement` issues still carry rich context: the problem, known constraints, candidate options, and the questions the next grilling must answer.
- Relations: *blocked by* only, with no estimates.

## Milestones

| | Milestone | Done when |
|---|---|---|
| M0 | Foundations | Repo, CI, DSL types + schema, Expressions, validation with Diagnostics exist. |
| M1 | Durable core | A Definition runs end to end from Postgres with retries, sleeps, branches and cron. |
| M2 | Agent interface | **Anchor scenario passes**: an agent, via MCP, authors and dry-runs an uptime-check Workflow, fixes a Diagnostic, a human publishes it, cron runs it, and the agent inspects and retries Runs. |
| M3 | UI: observe & author | Runs are observable and operable, and Definitions are editable with Diagnostics and Dry Run in the browser. |
| M4 | Control flow & triggers | The full Block set, Queues, webhooks, Events, Human Tasks and subworkflows work. |
| M5 | Visual editor | Definitions are built and edited as a graph, synced both ways with the document. |
| M6 | Action catalog | The broad builtin catalogue from the handoff exists. |
| M7 | Extensibility | Action Servers, WASM, policies, exec/container. |
| M8 | Hardening | OTel, multi-replica resilience, blob storage, external Secret Providers, releases. |

## M0 · Foundations

| ID | Linear | Title | Labels | Blocked by | Outcome |
|---|---|---|---|---|---|
| B01 | [BIA-5](https://linear.app/brnocorreia/issue/BIA-5) | Bootstrap repository, workspace and CI | infra, chore | — | `git init` with the existing foundation files as the first commit; public GitHub repo `brnocorreia/bia` with protected `main`; Cargo workspace with the seven crates (ADR-0013), `[workspace.lints]`, `rust-toolchain.toml`, edition 2024; `Justfile`; `docker compose` Postgres; CI running fmt, clippy `-D warnings`, tests with a Postgres service, and `cargo-deny`; Renovate or Dependabot. |
| B02 | [BIA-6](https://linear.app/brnocorreia/issue/BIA-6) | DSL types, JSON Schema and JSON/YAML parsing | core | B01 | `bia-core` models Definitions per `docs/dsl.md`; schema generated with `schemars` and snapshot-tested; YAML → JSON conversion; round-trip tests; `schema_version` present. |
| B03 | [BIA-7](https://linear.app/brnocorreia/issue/BIA-7) | CEL Expressions and Templates | core | B01 | Compile/evaluate CEL against the Run scope; type-preserving Templates; static extraction of referenced `steps.*`/`secrets.*` for validation; bounded evaluation. |
| B04 | [BIA-8](https://linear.app/brnocorreia/issue/BIA-8) | Action trait and registry interface | core | B02 | Transport-agnostic `Action` trait (ADR-0006): descriptor (name, category, description, param/output schema, examples, dangerous flag), classified errors, cancellation token, Idempotency Key; registry interface. |
| B05 | [BIA-9](https://linear.app/brnocorreia/issue/BIA-9) | Layered validation producing Diagnostics | core | B02, B03, B04 | Schema → semantics (unique ids, scope references only to earlier Steps, Expressions compile, params vs Action schema, resource-existence lookups via a trait, disabled Actions, limits) → Diagnostics with stable codes, JSON paths and suggestions; draft-vs-publish severity. |

## M1 · Durable core

| ID | Linear | Title | Labels | Blocked by | Outcome |
|---|---|---|---|---|---|
| B06 | [BIA-10](https://linear.app/brnocorreia/issue/BIA-10) | Store schema and repositories | store | B02 | Migrations and repositories for Workflows, Versions (Draft/Published, Active Version), Runs, Timeline entries, Jobs. `sqlx` offline data committed. |
| B07 | [BIA-11](https://linear.app/brnocorreia/issue/BIA-11) | Job queue primitives | store | B06 | All time read from the injected `Clock` (ADR-0022), with a manual test Clock. Claim with `SKIP LOCKED`, Leases with expiry, ack/fail/retry with backoff, delayed Jobs, `LISTEN/NOTIFY` wake-up; concurrency test with many claimers proving no double-processing. |
| B08 | [BIA-12](https://linear.app/brnocorreia/issue/BIA-12) | Interpreter: sequential Runs with Checkpoints | engine | B05, B07 | Advance a Run from its Checkpoint/Cursor; resolve Templates; enforce retry, timeout and error classification; Idempotency Key; Timeline writes; single-transaction transitions (ADR-0010); tested with fake Actions. |
| B09 | [BIA-13](https://linear.app/brnocorreia/issue/BIA-13) | Builtin Actions: `http.request`, `transform.expr` | actions | B04 | Two real Actions with schemas and examples; HTTP forwards the Idempotency Key; status-based error classification; `wiremock` tests. |
| B10 | [BIA-14](https://linear.app/brnocorreia/issue/BIA-14) | Basic flow: branch, sleep, sleep_until, fail, succeed, `if` | engine | B08 | Durable sleeps via delayed Jobs (no Worker held); `skipped` Steps recorded; Cursor works through nested Blocks. |
| B11 | [BIA-15](https://linear.app/brnocorreia/issue/BIA-15) | `bia serve` roles, Worker loop, config, cancellation | engine | B08, B09 | `bia serve --role …` and `bia migrate`; config from env and file; Worker pool claiming Jobs; graceful shutdown that releases Leases; Run cancellation reaches in-flight Actions. |
| B12 | [BIA-16](https://linear.app/brnocorreia/issue/BIA-16) | Scheduler and cron Triggers | engine | B11 | Schedules materialised from Active Versions; overlap and catch-up policies (ADR defaults); timezone and jitter; advisory-lock leadership; test with two schedulers firing exactly once. |
| B13 | [BIA-17](https://linear.app/brnocorreia/issue/BIA-17) | Global limits | engine | B08 | `max_steps_per_run`, `max_iterations`, `max_subworkflow_depth`, `max_output_bytes` enforced with clear Timeline errors; configurable. |
| B14 | [BIA-18](https://linear.app/brnocorreia/issue/BIA-18) | Run retention | engine | B12 | Internal scheduled cleanup (ADR-0021), configurable, tested. |

## M2 · Agent interface

| ID | Linear | Title | Labels | Blocked by | Outcome |
|---|---|---|---|---|---|
| B15 | [BIA-19](https://linear.app/brnocorreia/issue/BIA-19) | HTTP API foundation and `GET /schema` | server | B05, B11 | axum server in the `api` Role; OpenAPI via `utoipa` (spec served and committed); uniform error format embedding Diagnostics; health/readiness; `/schema` returns DSL schema + Action catalogue. |
| B16 | [BIA-20](https://linear.app/brnocorreia/issue/BIA-20) | Tokens, Scopes and CORS | server | B15 | Hashed Tokens with `admin`/`agent`/`webhook` Scopes; bootstrap of the first admin Token via `bia token create`; Scope enforcement middleware; `BIA_CORS_ORIGINS`. |
| B17 | [BIA-21](https://linear.app/brnocorreia/issue/BIA-21) | Workflow and Version API | server | B16, B06 | Create/replace Draft, validate, list, get, diff Versions, publish, activate, rollback; `agent` cannot publish or activate (ADR-0008). |
| B18 | [BIA-22](https://linear.app/brnocorreia/issue/BIA-22) | Run API | server | B17 | Manual trigger with input validation; list/filter Runs; get Run with Timeline; cancel; resume from failed Step; retry as new Run. |
| B19 | [BIA-23](https://linear.app/brnocorreia/issue/BIA-23) | Dry Run with Mocks | engine | B17, B10 | `POST …/dry-run` executes pure Actions for real and Mocks the rest (ADR semantics); sleeps and waits resolve instantly; unpersisted Timeline marked `dry_run`. |
| B20 | [BIA-24](https://linear.app/brnocorreia/issue/BIA-24) | `bia-client` and operational CLI | cli | B18, B19 | Typed client; `bia workflow apply -f` (JSON/YAML, files and directories), `validate`, `dry-run`, `publish`, `activate`, `diff`; `bia run trigger/get/list/cancel/retry`; Diagnostics rendered readably; `--json` output. |
| B21 | [BIA-25](https://linear.app/brnocorreia/issue/BIA-25) | MCP server | server | B19, B20 | `/mcp` over streamable HTTP (`rmcp`) and `bia mcp` stdio proxy; tools listed in `docs/architecture.md`; tool descriptions written for agents; Scope rules apply. |
| B22 | [BIA-26](https://linear.app/brnocorreia/issue/BIA-26) | Secrets: provider trait and internal store | server | B16, B08 | `SecretProvider` trait; encrypted internal store (ADR-0017); API/CLI CRUD that never returns values; runtime resolution in Params; masking in logs and Timeline. |
| B23 | [BIA-27](https://linear.app/brnocorreia/issue/BIA-27) | Anchor scenario end-to-end and quickstart guide | docs | B21, B12, B22 | Automated e2e test of the M2 anchor scenario through the public interfaces; `docs/quickstart.md` reproducing it by hand with an MCP client. |

## M3 · UI: observe & author

| ID | Linear | Title | Labels | Blocked by | Outcome |
|---|---|---|---|---|---|
| B24 | [BIA-28](https://linear.app/brnocorreia/issue/BIA-28) | UI scaffold | ui | B15 | Vite + React + TS, TanStack Router/Query, Tailwind + shadcn/ui, runtime `config.json`, Token login, types generated from OpenAPI, Vitest + Playwright, CI build. |
| B25 | [BIA-29](https://linear.app/brnocorreia/issue/BIA-29) | Embedded UI and static deploy | infra | B24 | `ui` Cargo feature embedding the build (rust-embed, SPA fallback); docs for Cloudflare Pages and S3 + CloudFront. |
| B26 | [BIA-30](https://linear.app/brnocorreia/issue/BIA-30) | Workflows and Versions screens | ui | B24, B17 | List, detail, Version history and diff, publish/activate/rollback (admin only). |
| B27 | [BIA-31](https://linear.app/brnocorreia/issue/BIA-31) | Runs list and Run Timeline | ui | B24, B18 | Filterable Runs list; Timeline with Attempts, inputs/Outputs, errors, durations; trigger, cancel, resume, retry; polling for live status. |
| B28 | [BIA-32](https://linear.app/brnocorreia/issue/BIA-32) | Definition editor with Diagnostics | ui | B26 | Monaco JSON/YAML with schema-aware completion; server Diagnostics shown inline by path; save as Draft. |
| B29 | [BIA-33](https://linear.app/brnocorreia/issue/BIA-33) | Dry Run from the editor | ui | B28, B19 | Run a Dry Run with input and per-Step Mocks; show the resulting Timeline. |
| B30 | [BIA-34](https://linear.app/brnocorreia/issue/BIA-34) | Secrets and Tokens screens | ui | B22, B24 | Manage Secrets (write-only values) and Tokens (shown once on creation). |

## M4 · Control flow & triggers

| ID | Linear | Title | Labels | Blocked by | Outcome |
|---|---|---|---|---|---|
| B31 | [BIA-35](https://linear.app/brnocorreia/issue/BIA-35) | Queues as resources | store | B07, B17 | Queue CRUD (API/CLI, `kind: queue` YAML); Messages with retry, visibility timeout, Dead-Letter Queue; `queue.publish`/`queue.publish_batch` with delay; unknown-Queue Diagnostics (ADR-0015). |
| B32 | [BIA-36](https://linear.app/brnocorreia/issue/BIA-36) | Queue Trigger | engine | B31, B12 | Consume batches into Runs with ack on Run completion semantics defined and tested. |
| B33 | [BIA-37](https://linear.app/brnocorreia/issue/BIA-37) | Webhook Triggers | server | B17, B16 | `/hooks/{workflow_id}/{trigger_id}`, per-Trigger secret, optional HMAC verification, payload → `input`. |
| B34 | [BIA-38](https://linear.app/brnocorreia/issue/BIA-38) | `flow.parallel` | engine | B10 | Branch execution, joins `all`/`any`/`n_of_m`, `fail_fast` cancels siblings, `all_settled`. |
| B35 | [BIA-39](https://linear.app/brnocorreia/issue/BIA-39) | `flow.foreach` | engine | B34 | `concurrency`, `max_iterations`, `item`/`index`, aggregated Output, iteration paths in Idempotency Keys. |
| B36 | [BIA-40](https://linear.app/brnocorreia/issue/BIA-40) | `flow.switch`, `flow.loop_until`, `flow.try`, `on_error` | engine | B10 | As specified in `docs/dsl.md` and ADR-0012. |
| B37 | [BIA-41](https://linear.app/brnocorreia/issue/BIA-41) | Events | engine | B18 | `flow.wait_for_event` with timeout; direct and correlated delivery endpoints; `event.emit`; event Trigger (ADR-0018). |
| B38 | [BIA-42](https://linear.app/brnocorreia/issue/BIA-42) | Human Tasks | engine | B37, B27 | `human.approval`/`form`/`choice`; pending Human Tasks API; UI inbox to answer them. |
| B39 | [BIA-43](https://linear.app/brnocorreia/issue/BIA-43) | `flow.subworkflow` and `workflow_completed` Trigger | engine | B18 | Sync and fire-and-forget, depth limit, parent/child links in Timeline. |
| B40 | [BIA-44](https://linear.app/brnocorreia/issue/BIA-44) | Per-Workflow concurrency limit | engine | B12 | `max_concurrent_runs` enforced across Triggers; interacts correctly with cron overlap `queue`. |
| B41 | [BIA-45](https://linear.app/brnocorreia/issue/BIA-45) | Queues and Dead-Letter UI | ui | B31, B24 | Browse Queues and Messages, replay or purge dead letters. |
| B42 | [BIA-46](https://linear.app/brnocorreia/issue/BIA-46) | `flow.rate_limit`, `flow.debounce`, `flow.dedupe` | engine | B36 | **needs-refinement**: durable semantics of debounce/dedupe windows, and whether limits are per Workflow or keyed. |

## M5 · Visual editor

| ID | Linear | Title | Labels | Blocked by | Outcome |
|---|---|---|---|---|---|
| B43 | [BIA-47](https://linear.app/brnocorreia/issue/BIA-47) | Definition ⇄ graph model | ui | B24 | Pure TS mapping between nested Blocks and a node/edge model with layout, and round-trip property tests. |
| B44 | [BIA-48](https://linear.app/brnocorreia/issue/BIA-48) | Read-only graph view with Run overlay | ui | B43, B27 | React Flow rendering of a Version; Step status of a Run painted on the graph. |
| B45 | [BIA-49](https://linear.app/brnocorreia/issue/BIA-49) | Visual editing | ui | B44, B28 | **needs-refinement**: add/move/delete Steps and Blocks, kept in sync with the text editor; UX for nested Blocks. |
| B46 | [BIA-50](https://linear.app/brnocorreia/issue/BIA-50) | Param forms from Action schemas | ui | B45 | Forms generated from `param_schema`, with Template-aware inputs. |
| B47 | [BIA-51](https://linear.app/brnocorreia/issue/BIA-51) | Expression authoring aid | ui | B28 | **needs-refinement**: scope-aware autocomplete and inline evaluation preview for CEL. |

## M6 · Action catalog

| ID | Linear | Title | Labels | Blocked by | Outcome |
|---|---|---|---|---|---|
| B48 | [BIA-52](https://linear.app/brnocorreia/issue/BIA-52) | `transform.*` collections and JSON | actions | B09 | map/filter/reduce/group_by/sort/unique/flatten, json_parse/stringify/patch/schema_validate. |
| B49 | [BIA-53](https://linear.app/brnocorreia/issue/BIA-53) | `transform.*` text and formats | actions | B09 | template (minijinja), csv/xml/yaml parse and write, regex_extract/replace. |
| B50 | [BIA-54](https://linear.app/brnocorreia/issue/BIA-54) | `transform.*` utilities | actions | B09 | hash, base64, uuid, random, datetime, math. |
| B51 | [BIA-55](https://linear.app/brnocorreia/issue/BIA-55) | LLM providers and `llm.complete`/`chat`/`structured` | actions | B09, B22 | Provider trait (Anthropic, OpenAI-compatible, Ollama), feature `actions-llm`, current Claude defaults, token usage in Output. |
| B52 | [BIA-56](https://linear.app/brnocorreia/issue/BIA-56) | `llm.classify`/`extract`/`summarize`/`translate`/`embed` | actions | B51 | Built on `llm.structured` where possible. |
| B53 | [BIA-57](https://linear.app/brnocorreia/issue/BIA-57) | `llm.tool_loop` | actions | B51 | ADR-0019: Action allowlist as tools, `max_iterations`, per-iteration Timeline sub-entries. |
| B54 | [BIA-58](https://linear.app/brnocorreia/issue/BIA-58) | `mcp.call_tool` | actions | B09 | Call a tool on an external MCP server configured by URL and Secret. |
| B55 | [BIA-59](https://linear.app/brnocorreia/issue/BIA-59) | `sql.query`/`execute`/`transaction` | actions | B09, B22 | Parameterised statements against Postgres/MySQL/SQLite connections referenced by Secret. |
| B56 | [BIA-60](https://linear.app/brnocorreia/issue/BIA-60) | `kv.*` | actions | B06 | get/set/delete/incr with TTL, stored in bia's Postgres. |
| B57 | [BIA-61](https://linear.app/brnocorreia/issue/BIA-61) | `email.send` | actions | B22 | SMTP plus at least one API provider; templates and attachments. |
| B58 | [BIA-62](https://linear.app/brnocorreia/issue/BIA-62) | `notify.*` | actions | B09 | Slack, Discord, Telegram, Teams webhooks; ntfy push. |
| B59 | [BIA-63](https://linear.app/brnocorreia/issue/BIA-63) | `storage.*` | actions | B22 | S3-compatible put/get/delete/list/presign. |
| B60 | [BIA-64](https://linear.app/brnocorreia/issue/BIA-64) | `fs.*` and `archive.*` with path policy | actions | B09 | File and archive Actions confined to configured roots; disabled by default. |
| B61 | [BIA-65](https://linear.app/brnocorreia/issue/BIA-65) | `log.write`, `metric.emit`, `audit.record` | actions | B09 | Structured entries linked to the Run. |
| B62 | [BIA-66](https://linear.app/brnocorreia/issue/BIA-66) | HTTP extras | actions | B09 | `http.graphql`, `http.download`/`upload` (streaming), `http.webhook_call` with HMAC. |
| B63 | [BIA-67](https://linear.app/brnocorreia/issue/BIA-67) | `net.*` checks | actions | B09 | dns_lookup, tcp_check, tls_cert_info (ping where unprivileged). |
| B64 | [BIA-68](https://linear.app/brnocorreia/issue/BIA-68) | `web.*` | actions | B09 | fetch_html, extract (CSS selectors), rss_read. |
| B65 | [BIA-69](https://linear.app/brnocorreia/issue/BIA-69) | Declarative connectors | actions | B62 | **needs-refinement**: connectors (GitHub, Linear, Notion, Stripe…) as declarative templates over `http.request`. |
| B66 | [BIA-70](https://linear.app/brnocorreia/issue/BIA-70) | Documents and media | actions | B59 | **needs-refinement**: pdf, image, ffmpeg; native libraries vs external tools. |
| B67 | [BIA-71](https://linear.app/brnocorreia/issue/BIA-71) | Multimodal AI | actions | B51 | **needs-refinement**: vision/OCR, transcription, TTS, image generation; provider coverage. |
| B68 | [BIA-72](https://linear.app/brnocorreia/issue/BIA-72) | Additional data stores | actions | B55 | **needs-refinement**: redis, mongo, vector (pgvector/Qdrant), feature-gated. |
| B69 | [BIA-73](https://linear.app/brnocorreia/issue/BIA-73) | git, sftp, grpc | actions | B60 | **needs-refinement**: scope and sandboxing. |

## M7 · Extensibility

| ID | Linear | Title | Labels | Blocked by | Outcome |
|---|---|---|---|---|---|
| B70 | [BIA-74](https://linear.app/brnocorreia/issue/BIA-74) | Action policy and Worker pools | engine | B11 | Server config enabling/disabling categories, allowlists; Workers started with an Action allowlist claim only matching Jobs (ADR-0006 step 3). |
| B71 | [BIA-75](https://linear.app/brnocorreia/issue/BIA-75) | Action Servers over HTTP | engine | B70, B17 | **needs-refinement**: registration, health, auth, request/response and error contract, timeouts and cancellation over HTTP (ADR-0020). |
| B72 | [BIA-76](https://linear.app/brnocorreia/issue/BIA-76) | Reference Action Server SDK | docs | B71 | **needs-refinement**: a small TypeScript or Python SDK plus example server. |
| B73 | [BIA-77](https://linear.app/brnocorreia/issue/BIA-77) | WASM Actions | engine | B70 | **needs-refinement**: wasmtime component model vs core modules, capabilities, fuel/memory limits, distribution. |
| B74 | [BIA-78](https://linear.app/brnocorreia/issue/BIA-78) | `exec.process` and `exec.shell` | actions | B70 | Policy-gated per ADR-0014; binary allowlist; no shell for `exec.process`; resource limits; admin-only publish. |
| B75 | [BIA-79](https://linear.app/brnocorreia/issue/BIA-79) | `container.run` | actions | B70 | **needs-refinement**: Docker vs Podman API, image policy, volumes, network isolation. |
| B76 | [BIA-80](https://linear.app/brnocorreia/issue/BIA-80) | `ssh.exec` and `k8s.*` | actions | B74 | **needs-refinement**: credentials, host allowlists, manifest policy. |

## M8 · Hardening

| ID | Linear | Title | Labels | Blocked by | Outcome |
|---|---|---|---|---|---|
| B77 | [BIA-81](https://linear.app/brnocorreia/issue/BIA-81) | OpenTelemetry and Prometheus | engine | B11 | OTLP traces (Run/Step spans) and `/metrics`; feature- or config-gated. |
| B78 | [BIA-82](https://linear.app/brnocorreia/issue/BIA-82) | Multi-replica resilience suite | engine | B12, B21 | Tests that kill Workers mid-Step, expire Leases, fail over the scheduler, and prove at-least-once with no lost Runs. |
| B79 | [BIA-83](https://linear.app/brnocorreia/issue/BIA-83) | DSL migrations framework | core | B02 | Upgrade stored Definitions across `schema_version`s, with a test fixture per version. |
| B80 | [BIA-84](https://linear.app/brnocorreia/issue/BIA-84) | Env and AWS Secrets Manager providers | server | B22 | Prefix routing to providers (ADR-0017). |
| B81 | [BIA-85](https://linear.app/brnocorreia/issue/BIA-85) | Vault, Infisical, 1Password providers | server | B80 | **needs-refinement**, low priority. |
| B82 | [BIA-86](https://linear.app/brnocorreia/issue/BIA-86) | Blob storage for large Outputs | engine | B59 | **needs-refinement**: threshold, references in Timeline, retention interplay. |
| B83 | [BIA-87](https://linear.app/brnocorreia/issue/BIA-87) | Early Event buffering | engine | B37 | **needs-refinement**: whether and how to buffer Events that arrive before a waiter (ADR-0018 gap). |
| B84 | [BIA-88](https://linear.app/brnocorreia/issue/BIA-88) | Live UI updates via SSE | server | B27 | Server-sent events for Run and Timeline changes; UI switches from polling. |
| B85 | [BIA-89](https://linear.app/brnocorreia/issue/BIA-89) | Release pipeline | infra | B23 | Versioned binaries for major platforms, container image, changelog from Conventional Commits. |

## Next steps

Linear is now the source of truth. The spec lives in the Linear document [Spec: bia, a workflow-as-data engine](https://linear.app/brnocorreia/document/spec-bia-a-workflow-as-data-engine-fb76a7e53255), and every issue above exists in the [bia project](https://linear.app/brnocorreia/project/bia-e17fd941b01e) with its milestone, labels and *blocked by* relations. This file stays only as the historical plan; change issues in Linear, not here.
