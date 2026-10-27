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
| `retry` | `max_attempts` (required, counts the first Attempt), `backoff` (`fixed` \| `exponential`, default `exponential`), `initial` (default `1s`), `max` (default `1h`). Applies to `retryable` errors only; without it a Step has one Attempt. |
| `timeout` | Per Attempt, default `5m`. An Attempt past it is cancelled and fails with a `retryable` error. |
| `if` | Optional CEL condition. When false, the Step is recorded as `skipped`. |
| `description` | Free text shown in the UI and Timeline. |

Durations are strings: a whole number followed by `ms`, `s`, `m`, `h` or `d` (`500ms`, `30s`, `5m`, `2h`, `3d`). Units cannot be combined (`1h30m` is invalid; write `90m`).

Ids and names (Workflow, Step, Trigger, Queue, Action) are at most 64 characters.

## Blocks (control flow)

Blocks hold nested Step lists. They never reference other Steps by id (ADR-0003). The set of Blocks is closed and typed in the schema: their nested fields sit next to the common Step fields, and they take no `params` (ADR-0024). Required fields are in **bold**.

| Action | Nested fields | Notes |
|---|---|---|
| `flow.branch` | **`when`**, **`then`**, `else` | Output is `{ "arm": "then" \| "else" }`. |
| `flow.switch` | **`value`**, **`cases: [{ equals, steps }]`**, `default` | |
| `flow.parallel` | **`branches: [[steps], …]`**, `join: all\|any\|n_of_m`, `n`, `mode: fail_fast\|all_settled` | Output is the list of branch Outputs. |
| `flow.foreach` | **`items`** (CEL), **`body`**, `concurrency`, `max_iterations`, `mode` | `item` and `index` in scope; Output is the list of each iteration's last Output. |
| `flow.loop_until` | **`body`**, **`until`** (CEL), `max_iterations` | `until` sees the iteration's Outputs. |
| `flow.try` | **`body`**, **`catch`** | `catch` sees `error`. |

Every other Action is an ordinary Step that takes `params`, including these flow Actions: `flow.subworkflow` (params: `workflow`, `input`, `wait: true|false`; bounded by `max_subworkflow_depth`), `flow.sleep` (`duration`), `flow.sleep_until` (`timestamp`, RFC 3339), `flow.wait_for_event` (`event`, optional `match` Correlation Key, `timeout`), `flow.fail` (`message`), `flow.succeed` (ends the whole Run, from any depth), `flow.rate_limit`, `flow.debounce`, `flow.dedupe`.

Human Tasks: `human.approval`, `human.form` (fields from a JSON Schema), `human.choice`. All are built on `wait_for_event` (ADR-0018).

## Expressions and Templates

- Condition fields (`when`, `until`, `if`, `items`, `value`) take raw CEL.
- `params` values can be Templates. `"{{ expr }}"` alone, with nothing around it (not even whitespace), keeps the expression's type; `"text {{ expr }}"` renders a string, inserting strings as they are and any other value as compact JSON.
- Inside `{{ }}` the text is trimmed. The closing `}}` is the first one outside CEL string literals and braces the expression opened, so `{{ {'a': {'b': 1}} }}` is one map. A literal `{{` is written `{{ '{{' }}`.
- Scope: `input`, `steps.<id>.output` (only Steps that already ran), `trigger`, `run` (`id`, `version`, `attempt`, `started_at`), `item`, `index`, `error` (inside `catch`). `secrets.NAME` is allowed in `params` only (ADR-0011).
- Functions: the CEL standard library (including `timestamp`, `duration`, `matches`), the `strings` extension (`lowerAscii`, `split`, `join`, `replace`, `format`, …), `math` (`math.abs`, `math.ceil`, `math.round`, bit operations, …; not `math.greatest`/`math.least`) and `base64.encode`/`base64.decode` (ADR-0026).
- JSON numbers without a fractional part are `int`; other numbers are `double`. Results become JSON again, with `bytes` as base64, `timestamp` as RFC 3339 and `duration` as `"1.5s"`.
- Limits: an expression is at most 8 KiB, at most 32 levels deep, and spends at most 100,000 cost units per evaluation. Each `map`/`filter`/`all`/`exists`/`exists_one` iteration costs 1 plus the size of what it produces.

## Limits

A Run is held to the server's global limits (ADR-0012, ADR-0038; configured as in [`configuration.md`](configuration.md)). A Definition's top-level `limits` can lower, never raise, two of them; a higher value is capped at the global limit.

| Field | Meaning |
|---|---|
| `max_concurrent_runs` | Most Runs of this Workflow active at once. |
| `max_steps_per_run` | Most Steps one Run may take up (global default 10,000). A Step counts once each time the Run reaches it, also when skipped or failed; retries and waking from a sleep do not count again. |
| `max_output_bytes` | Largest Output a Step may produce, in bytes of compact JSON (global default 256 KiB). |

A loop's `max_iterations` lowers the global `max_iterations` (default 10,000) for that loop, and `flow.subworkflow` is bounded by `max_subworkflow_depth` (default 8). Going past any limit fails the Step with a `fatal` error, so it consumes no retries. Its Timeline `code` is `run_step_limit_exceeded`, `iteration_limit_exceeded`, `subworkflow_depth_exceeded` or `output_limit_exceeded`, and its `details` are `{ "limit", "max", "actual" }`. An Output over the limit is not stored.

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

### Cron

Only the cron Triggers of the Active Version fire. Activating another Version replaces them within a second; a Trigger whose `expr`, `tz` and `jitter` did not change keeps its next occurrence. Semantics: ADR-0040.

- `expr` has five fields: minute, hour, day of month, month, day of week (`0 9 * * 1-5`). `L`, `W`, `#`, names (`MON`, `JAN`) and aliases (`@hourly`, `@daily`, `@weekly`, `@monthly`, `@yearly`) work; seconds do not.
- `tz` is an IANA time zone, UTC by default. Occurrences follow its wall clock: `0 9 * * *` stays at 09:00 local across DST changes, a time skipped by a change fires right after the gap, and a time repeated by one fires once.
- `jitter` delays each firing by up to the Duration. Each occurrence gets its own delay, the same on every scheduler replica.
- `overlap` decides what happens while the previous Run this Trigger started is still active: `skip` (default) drops the occurrence, `queue` starts it once that Run finishes, holding at most 100 and skipping further ones, and `allow` starts it at once.
- `catch_up` decides what happens to occurrences missed while no scheduler ran, meaning reached more than a minute after they were due: `none` drops them, `latest` (default) fires the newest one, and `all` fires each, up to the 100 most recent. The fired occurrences then go through `overlap`, so `all` with `skip` fires only the first.
- A cron Run's input is `{}`, and its `trigger` is `{ "type": "cron", "id": "<trigger id>", "scheduled_at": "<occurrence, RFC 3339 UTC>" }`. `scheduled_at` is the occurrence before jitter, even when the Run started later from the queue or after downtime.

### Webhook

`POST /hooks/{workflow_id}/{trigger_id}` starts a Run of the Active Version; webhook Triggers of other Versions answer 404. Semantics: ADR-0058.

- Without `hmac`, the caller sends `Authorization: Bearer <token>`: the `webhook` Token bound to this Trigger (`POST /v1/tokens` with `webhook: { workflow_id, trigger_id }`) or an `admin` Token.
- With `hmac`, the caller sends the HMAC-SHA256 of the raw body, keyed by the Secret `hmac.secret` names, in the `hmac.header` header, as hex optionally prefixed `sha256=` (GitHub's `X-Hub-Signature-256` format). No Token is needed, but one that is sent must be valid for this Trigger.
- The JSON body is the Run's `input` (an empty body is `{}`), checked against `input_schema` before the Run is created. An accepted request answers 202 with `{ "run_id" }`.
- The Run's `trigger` is `{ "type": "webhook", "id": "<trigger id>", "received_at": "<RFC 3339 UTC>", "headers": { "<lowercased name>": "<value>" } }`. `headers` leaves out every header that may carry a credential: the `hmac.header`, and any whose name contains `auth`, `token`, `secret`, `signature`, `password`, `session`, `credential`, `cookie` or `key`.

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

Validation runs in layers: JSON Schema → semantics (unique ids, scope references, Expressions compile, params match the Action, referenced Queues and Workflows exist, disabled Actions, limits) → optional Dry Run (ADR-0046). The semantic layer reports every problem in one pass. A document that fails the schema layer has no typed form to check further, so it gets that layer's first Diagnostic only. Every finding is a Diagnostic:

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

`path` is a JSON Pointer into the Definition (empty for the root), and `suggestion` is `null` when there is none. Diagnostic codes are lower `snake_case` phrases naming the problem (ADR-0028).

| Code | Layer | Severity | Meaning |
|---|---|---|---|
| `invalid_syntax` | schema | error | The text is not well-formed JSON or YAML. |
| `missing_schema_version` | schema | error | The Definition has no `schema_version`. |
| `unsupported_schema_version` | schema | error | This server does not support the `schema_version`. |
| `unknown_kind` | schema | error | A document's `kind` names no known resource. |
| `unknown_field` | schema | error | An object has a field the DSL does not define. Suggests the closest field. |
| `missing_field` | schema | error | A required field is absent. |
| `field_not_allowed` | schema | error | The field exists, but not on this Action or Trigger `type` (e.g. `params` on a Block). |
| `invalid_value` | schema | error | A value has the wrong type or format: a malformed id, Duration or enum value. Suggests the closest enum value. |
| `duplicate_step_id` | semantic | error | Two Steps share an id, nested Blocks included. |
| `duplicate_trigger_id` | semantic | error | Two Triggers share an id. |
| `invalid_expression` | semantic | error | An Expression or Template does not compile. |
| `expression_limit_exceeded` | semantic | error | An Expression is longer or nests deeper than the limit. |
| `secret_outside_params` | semantic | error | `secrets.*` is read outside `params`. |
| `variable_out_of_scope` | semantic | error | `item`/`index` outside a `flow.foreach` body, or `error` outside a `catch`. |
| `unknown_step_reference` | semantic | error | `steps.<id>` names no Step. Suggests the closest id. |
| `forward_step_reference` | semantic | error | `steps.<id>` names a Step that has not finished at this point: a later Step, the Step itself or a Block around it. |
| `step_may_not_have_run` | semantic | error | `steps.<id>` names an earlier Step that does not run on every path to this point. |
| `unknown_action` | semantic | error | The Action is not in the Action Registry. Suggests the closest name. |
| `disabled_action` | semantic | warning on a Draft, error at publish | The Action is disabled on this server (ADR-0014). |
| `invalid_params` | semantic | error | Params break the Action's param schema. |
| `unknown_queue` | semantic | warning on a Draft, error at publish | A referenced Queue does not exist (ADR-0015). Suggests the closest name. |
| `unknown_workflow` | semantic | warning on a Draft, error at publish | A referenced Workflow does not exist. Suggests the closest id. |
| `step_limit_exceeded` | semantic | error | The Definition has more Steps than `max_steps` (default 500). |
| `nesting_limit_exceeded` | semantic | error | Steps sit inside more Blocks than `max_nesting_depth` (default 16). |
| `iteration_limit_exceeded` | semantic | error | A loop's `max_iterations` exceeds the limit (default 10,000). |

### Which Steps an Expression can read

`steps.<id>` is valid only where Step `<id>` has finished on every path that reaches the Expression:

- Earlier Steps in the same list, and in every list around it, are readable. A Step skipped by its `if` is still recorded, so it counts.
- A Block's nested Steps stay out of scope after the Block: branch and switch arms, `flow.try` bodies and catches, and loop bodies may run partly or not at all. Read the Block's own Output instead. The one exception is `flow.parallel` with `join: all`, whose branches have all finished once it completes.
- Inside a `flow.foreach` or `flow.loop_until` body, Steps see the earlier Steps of the same iteration. `until` sees the whole iteration.
- `catch` sees what its `flow.try` saw, plus `error`, but not the body.
- `on_error` sees none of `steps`, because the Run may have failed anywhere.
- Reads by a computed id, such as `steps[name]`, can't be checked. Fields of `input`, `trigger` and `run` are not checked against `input_schema`.

### Params that name resources

An Action marks a param as naming a Queue or a Workflow with `"x-bia-resource": "queue"` or `"workflow"` in its param schema, on a property reached through `properties` from the schema root (ADR-0029). Validation checks literal names against the server's Queues and Workflows; names written as Templates are only known at execution time. A Definition always knows its own Workflow id.
