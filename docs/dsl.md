# DSL reference (draft)

Status: **draft**. The JSON Schema generated from `bia-core` (`bia_core::dsl::definition_schema`, snapshotted in `crates/bia-core/tests/snapshots/`) is the source of truth. When the two disagree, update this file in the same PR that changes the schema. Vocabulary follows [`GLOSSARY.md`](../GLOSSARY.md), and the reasons behind each choice live in [`docs/adr/`](adr/).

## Shape of a Definition

```yaml
schema_version: 1
id: onboarding-email            # Workflow id, stable across Versions
description: Welcome new signups and escalate VIPs
triggers:
  - { id: daily, type: cron, expr: "0 9 * * *", tz: America/Sao_Paulo, overlap: skip, catch_up: latest }
  - { id: signups, type: queue, queue: signups, batch_size: 10 }
  - { id: signup_hook, type: webhook, hmac: { header: X-Signature, secret: secrets.SIGNUP_HMAC } }
  - { id: by_hand, type: manual }
input_schema:
  type: object
  required: [email]
  properties: { email: { type: string, format: email } }
limits: { max_concurrent_runs: 5 }   # optional; global limits apply otherwise
steps:
  - id: fetch_user
    action: http.request
    params:
      method: GET
      url: "https://api.example.com/users/{{ input.email }}"
      headers: { Authorization: "Bearer {{ secrets.API_TOKEN }}" }
    retry: { max_attempts: 5, backoff: exponential, initial: 2s, max: 1m }
    timeout: 30s

  - id: is_vip
    action: flow.branch
    when: steps.fetch_user.output.body.plan == 'vip'
    then:
      - id: send_vip
        action: queue.publish
        params: { queue: emails, body: "{{ steps.fetch_user.output.body }}" }
    else:
      - id: send_basic
        action: queue.publish
        params: { queue: emails, body: { email: "{{ input.email }}", template: basic } }

  - id: wait
    action: flow.sleep
    params: { duration: 3d }

  - id: approval
    action: human.approval
    params: { title: "Upgrade {{ input.email }}?", timeout: 7d }

on_error:
  - id: notify_failure
    action: http.request
    params: { method: POST, url: "https://hooks.example.com/alerts", body: { run: "{{ run.id }}" } }
```

The canonical form is JSON, and the server only accepts JSON. The CLI also accepts YAML (1.2, so only `true` and `false` are booleans, and duplicate keys are an error) and converts it to JSON on the client.

Required top-level fields are `schema_version` (currently `1`), `id` and `steps`. A Workflow `id` is `[a-z][a-z0-9_-]*`. Every object in the DSL rejects unknown fields, and a `null` value counts as leaving the field out (ADR-0024).

## Common Step fields

| Field | Meaning |
|---|---|
| `id` | Unique across the whole Definition, including nested Blocks. `[a-z][a-z0-9_]*`. |
| `action` | Action name, `category.operation`. |
| `params` | Object validated against the Action's param schema after Templates are resolved. Ordinary Actions only; Blocks take their nested fields instead. |
| `retry` | `max_attempts` (required, counts the first Attempt), `backoff` (`fixed` \| `exponential`), `initial`, `max`. Applies to `retryable` errors only. |
| `timeout` | Per Attempt. |
| `if` | Optional CEL condition. When false, the Step is recorded as `skipped`. |
| `description` | Free text shown in the UI and Timeline. |

Durations are strings: a whole number followed by `ms`, `s`, `m`, `h` or `d` (`500ms`, `30s`, `5m`, `2h`, `3d`). Units cannot be combined (`1h30m` is invalid; write `90m`).

Ids and names (Workflow, Step, Trigger, Queue, Action) are at most 64 characters.

## Blocks (control flow)

Blocks hold nested Step lists. They never reference other Steps by id (ADR-0003). The set of Blocks is closed and typed in the schema: their nested fields sit next to the common Step fields, and they take no `params` (ADR-0024). Required fields are in **bold**.

| Action | Nested fields | Notes |
|---|---|---|
| `flow.branch` | **`when`**, **`then`**, `else` | |
| `flow.switch` | **`value`**, **`cases: [{ equals, steps }]`**, `default` | |
| `flow.parallel` | **`branches: [[steps], …]`**, `join: all\|any\|n_of_m`, `n`, `mode: fail_fast\|all_settled` | Output is the list of branch Outputs. |
| `flow.foreach` | **`items`** (CEL), **`body`**, `concurrency`, `max_iterations`, `mode` | `item` and `index` in scope; Output is the list of each iteration's last Output. |
| `flow.loop_until` | **`body`**, **`until`** (CEL), `max_iterations` | `until` sees the iteration's Outputs. |
| `flow.try` | **`body`**, **`catch`** | `catch` sees `error`. |

Every other Action is an ordinary Step that takes `params`, including these flow Actions: `flow.subworkflow` (params: `workflow`, `input`, `wait: true|false`; bounded by `max_subworkflow_depth`), `flow.sleep`, `flow.sleep_until`, `flow.wait_for_event` (`event`, optional `match` Correlation Key, `timeout`), `flow.fail`, `flow.succeed`, `flow.rate_limit`, `flow.debounce`, `flow.dedupe`.

Human Tasks: `human.approval`, `human.form` (fields from a JSON Schema), `human.choice`. All are built on `wait_for_event` (ADR-0018).

## Expressions and Templates

- Condition fields (`when`, `until`, `if`, `items`, `value`) take raw CEL.
- `params` values can be Templates. `"{{ expr }}"` alone keeps the expression's type; `"text {{ expr }}"` renders a string.
- Scope: `input`, `steps.<id>.output` (only Steps that already ran), `trigger`, `run` (`id`, `version`, `attempt`, `started_at`), `item`, `index`, `error` (inside `catch`). `secrets.NAME` is allowed in `params` only (ADR-0011).

## Triggers

| `type` | Fields |
|---|---|
| `cron` | **`expr`**, `tz`, `jitter` (Duration), `overlap: skip\|queue\|allow`, `catch_up: none\|latest\|all` |
| `queue` | **`queue`**, `batch_size` |
| `webhook` | `hmac: { header, secret }` (optional). Served at `/hooks/{workflow_id}/{trigger_id}` (ADR-0015). |
| `manual` | — |
| `event` | **`event`**, optional `match` Correlation Key |
| later | `workflow_completed`, `poll`, `fs_watch`, `email_received` |

Every Trigger has an `id` (`[a-z][a-z0-9_]*`) unique within the Definition, and only the fields of its `type`. The Trigger payload becomes `input` (validated against `input_schema`), and its metadata becomes `trigger`.

## Queue resources

Queues are declared separately from Workflows (ADR-0015). A resource file is told apart from a Definition by its top-level `kind`; Definitions never have one (ADR-0025). Only `kind` and `name` are required.

```yaml
kind: queue
name: emails
max_retries: 5
visibility_timeout: 30s
batch_size: 10
dead_letter: emails-dlq
```

## Diagnostics

Validation runs in layers: JSON Schema → semantics (unique ids, scope references, Expressions compile, params match the Action, referenced Queues and Workflows exist, disabled Actions, limits) → optional Dry Run. Every finding is a Diagnostic:

```json
{
  "path": "/steps/1/then/0/params/queue",
  "code": "unknown_queue",
  "severity": "error",
  "message": "Queue 'emals' does not exist",
  "suggestion": "Did you mean 'emails'?"
}
```

Diagnostic codes are stable identifiers that agents can match on. Message text can change.
