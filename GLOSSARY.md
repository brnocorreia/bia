# bia

bia (Background Integrations & Automations) runs cron schedules, durable queues and long-running workflows where every pipeline is **data**: a document validated against a published schema, authored by humans, a UI or an AI agent, and executed by an interpreter.

## Language

### Definitions

**Workflow**:
A named, stable identity for a pipeline (e.g. `onboarding-email`). It owns a sequence of Versions and points at one Active Version.
_Avoid_: pipeline, flow, job, DAG

**Version**:
One immutable snapshot of a Workflow's Definition, numbered 1, 2, 3… A Version is either a Draft or Published.
_Avoid_: revision, release

**Definition**:
The document (JSON, or YAML compiled to JSON) that describes a Version: triggers, input schema, steps, error handling.
_Avoid_: spec, config, manifest

**Draft**:
A Version that can be edited by replacement, validated and dry-run, but never runs from a Trigger.

**Published**:
A Version that has passed full validation and can become the Active Version.

**Active Version**:
The single Published Version a Workflow's Triggers start Runs on. Rollback means pointing it at an older Version.
_Avoid_: current version, live version

**DSL**:
The restricted vocabulary a Definition is written in, versioned by `schema_version`.

### Steps and actions

**Step**:
One node of a Definition, identified by an `id` unique across the whole Definition, including nested Blocks.
_Avoid_: task, node, activity

**Block**:
A Step whose Action is one of the control-flow Actions that contain nested lists of Steps (branch arms, parallel branches, a loop body): `flow.branch`, `flow.switch`, `flow.parallel`, `flow.foreach`, `flow.loop_until`, `flow.try`. Other `flow.*` and `human.*` Actions are ordinary Steps with Params.
_Avoid_: container, group

**Action**:
The kind of work a Step performs, named `category.operation` (e.g. `http.request`), with a declared param schema and output schema.
_Avoid_: operator, plugin, function, step type

**Action Registry**:
The set of Actions a running bia knows about: builtin ones plus those served by Action Servers.

**Action Server**:
An external HTTP service that registers Actions with bia and executes them on request.
_Avoid_: remote worker, plugin host

**Params**:
The input a Step passes to its Action, possibly containing Templates.

**Output**:
The immutable value a Step produces once it succeeds, readable by later Steps as `steps.<id>.output`.
_Avoid_: result, return value

### Expressions

**Expression**:
A side-effect-free CEL program evaluated against the Run's scope (`input`, `steps`, `trigger`, `run`, `item`, `index`).

**Template**:
A string in Params containing `{{ expression }}`. A Template that is exactly one expression keeps the expression's type; otherwise it renders to a string.
_Avoid_: interpolation, placeholder

### Triggers and runs

**Trigger**:
A declaration in a Definition of what starts a Run: cron, queue, webhook, manual, event, and later others.
_Avoid_: hook, listener

**Schedule**:
The scheduler's materialised state for a cron Trigger: when it fires next, its Overlap Policy and Catch-up Policy.

**Overlap Policy**:
What a cron Trigger does when the previous Run is still active: `skip`, `queue` or `allow`.

**Catch-up Policy**:
What a cron Trigger does with firings missed while bia was down: `none`, `latest` or `all`.

**Run**:
One execution of a specific Version, started by a Trigger, pinned to that Version for its whole life.
_Avoid_: execution, instance, job

**Attempt**:
One try at executing a Step. A Step with a retry policy can have several Attempts that share an Idempotency Key.

**Idempotency Key**:
A key stable across a Step's Attempts (`run_id:step_id:iteration_path`) that Actions forward to external systems.

**Checkpoint**:
The persisted state of a Run after a transition. Resuming a Run means reading its Checkpoint and continuing; code is never replayed.
_Avoid_: snapshot, savepoint

**Cursor**:
The position inside the Definition that a Run's Checkpoint points at, as a path through nested Blocks.

**Timeline**:
The ordered, human- and agent-readable record of a Run: each Step's Attempts, inputs, Outputs, errors and durations.
_Avoid_: history, log, trace

**Dry Run**:
An unpersisted Run of a Version where pure Actions execute for real and side-effecting Actions return Mocks.
_Avoid_: simulation, test run

**Mock**:
A stand-in Output for a Step during a Dry Run, supplied by the caller or derived from the Action's examples.

### Events and humans

**Event**:
A named signal with a payload that resumes Runs waiting in `flow.wait_for_event`, delivered either to one Run directly or by Correlation Key.
_Avoid_: signal, message, callback

**Correlation Key**:
The value a waiting Step declares so that an Event carrying the same name and key reaches it, without the sender knowing the Run.

**Human Task**:
A pending request for a person's input (`human.approval`, `human.form`, `human.choice`); answering it emits an Event.
_Avoid_: approval request, manual step

### Queues

**Queue**:
A named, explicitly created, durable buffer of Messages with its own retry, visibility and dead-letter settings.
_Avoid_: topic, channel, stream

**Message**:
One item in a Queue, delivered at least once to a consumer.
_Avoid_: job, event, item

**Dead-Letter Queue**:
The Queue that receives Messages which exhausted their retries.
_Avoid_: DLQ in prose (fine in identifiers), error queue

**Job**:
An internal unit of engine work (advance a Run, fire a Schedule, deliver a Message) claimed by a Worker. Never user-facing.
_Avoid_: task

**Lease**:
A time-bounded claim a Worker holds on a Job; when it expires the Job becomes claimable again.
_Avoid_: lock (for this meaning)

### Operation and access

**Role**:
A responsibility a `bia serve` process takes on: `api`, `worker`, `scheduler`, or `all`.

**Worker**:
A process with the `worker` Role, which claims Jobs and executes Actions.

**Diagnostic**:
One structured validation finding: JSON path, code, severity (error or warning), message and suggestion.
_Avoid_: validation error (when it may be a warning), lint

**Token**:
A credential for the API with a Scope.

**Scope**:
What a Token may do: `admin` (everything), `agent` (author Drafts, dry-run, trigger, read; never publish or activate), `webhook` (start Runs from one webhook Trigger).

**Secret**:
A named sensitive value referenced as `secrets.NAME` in Params, resolved only at execution time and never persisted in Outputs or the Timeline.

**Secret Provider**:
A source Secrets are resolved from: the internal encrypted store first, external managers later.
