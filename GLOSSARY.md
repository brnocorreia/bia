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
A side-effect-free CEL program evaluated against the Run's scope (`input`, `steps`, `trigger`, `run`, `item`, `index`, `error`, and `secrets` in Params), within a bounded cost.

**Template**:
A string in Params containing `{{ expression }}`. A Template that is exactly one expression keeps the expression's type; otherwise it renders to a string.
_Avoid_: interpolation, placeholder

### Triggers and runs

**Trigger**:
A declaration in a Definition of what starts a Run: cron, queue, webhook, manual, event, and later others.
_Avoid_: hook, listener

**Schedule**:
The scheduler's materialised state for a cron Trigger of an Active Version: when it fires next, its Overlap Policy and Catch-up Policy, the occurrences it holds back and the Run it started last.

**Overlap Policy**:
What a cron Trigger does when the previous Run is still active: `skip`, `queue` or `allow`.

**Catch-up Policy**:
What a cron Trigger does with firings missed while bia was down: `none`, `latest` or `all`.

**Run**:
One execution of a specific Version, started by a Trigger, pinned to that Version for its whole life.
_Avoid_: execution, instance, job

**Resume**:
Putting a failed Run back to work from the Step that failed, on its own Version; Steps that succeeded do not run again. Starting a new Run from a finished one is a **Retry**, which keeps its Version and input and links back to the original.
_Avoid_: restart, replay, re-run

**Attempt**:
One try at executing a Step. A Step with a retry policy can have several Attempts that share an Idempotency Key.

**Idempotency Key**:
A key stable across a Step's Attempts (`run_id:step_id:iteration_path`) that Actions forward to external systems.

**Checkpoint**:
The persisted state of a Run after a transition. Resuming a Run means reading its Checkpoint and continuing; code is never replayed.
_Avoid_: snapshot, savepoint

**Cursor**:
The position inside the Definition that a Run's Checkpoint points at: the path to the Step it executes next, through nested Blocks (e.g. `["steps", 2]`).

**Timeline**:
The ordered, human- and agent-readable record of a Run: each Step's Attempts, inputs, Outputs, errors and durations, plus entries about the Run as a whole, such as input rejected before any Step ran.
_Avoid_: history, log, trace

**Dry Run**:
An unpersisted Run of a Version or Definition where pure Actions execute for real, side-effecting Actions return Mocks, and waits resolve instantly. Its result is a Timeline marked `dry_run` (ADR-0046).
_Avoid_: simulation, test run

**Mock**:
A stand-in result for a Step during a Dry Run, an Output or an error, supplied by the caller per Step id or, for an Output, derived from the Action's first example.

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
One item in a Queue, delivered at least once to a consumer, which hides it from other consumers for the Queue's visibility timeout and then acks it; a Message that is not acked in time is delivered again. A queue Trigger's Run is the exception: it holds its Messages until it finishes, however long that takes.
_Avoid_: job, event, item

**Dead-Letter Queue**:
An ordinary Queue that another Queue names in `dead_letter` to receive its Messages which exhausted their retries. Such a Message is a **dead letter**; a Queue without a Dead-Letter Queue keeps its dead letters and never delivers them again. **Replaying** a dead letter makes it deliverable again in the Queue it was dead-lettered from, with its delivery count reset; **purging** deletes it. Replay applies only to dead letters, never to Runs (see Resume and Retry).
_Avoid_: DLQ in prose (fine in identifiers), error queue

**Job**:
An internal unit of engine work (advance a Run, deliver a Message) claimed by a Worker. Never user-facing.
_Avoid_: task

**Lease**:
A time-bounded claim a Worker holds on a Job, extended by heartbeats while the Job runs; when it expires, or the Worker releases it on shutdown, the Job becomes claimable again.
_Avoid_: lock (for this meaning)

### Operation and access

**Role**:
A responsibility a `bia serve` process takes on: `api`, `worker`, `scheduler`, or `all`.

**Worker**:
A process with the `worker` Role, which claims Jobs and executes Actions.

**Leader**:
The one scheduler, among those running, that holds the Postgres advisory lock and so fires Schedules and runs Chores.
_Avoid_: primary, master

**Chore**:
Periodic upkeep the Leader runs once per interval, such as Run retention. Postgres records when each Chore last ran, so it runs once per interval across schedulers. Unlike a Job, it carries no work of its own to store and no Worker claims it.
_Avoid_: task, cron job

**Clock**:
Where the engine, scheduler and store read the current time from, and wait on it: the system Clock in production, a manual Clock that tests move forward.
_Avoid_: timer, now

**Diagnostic**:
One structured validation finding: JSON path, code, severity (error or warning), message and suggestion.
_Avoid_: validation error (when it may be a warning), lint

**User**:
A person who signs in to bia and on whose behalf Tokens act. An install has one, the admin, created by first-run setup.
_Avoid_: account, admin (for the row; "admin" is a Scope)

**Token**:
A credential for the API with a Scope, belonging to a User, shown once when created and stored only as a hash.
_Avoid_: API key

**Setup Link**:
`{ui_url}/setup?code=<code>`, which `bia serve` logs while no User can sign in; its one-time code sets the User's password. A new code is made on every start.
_Avoid_: invite, bootstrap link

**Public URL**:
The URL people reach the API at (`public_url`), and `ui_url` the one they reach the UI at; links the server hands out are built from them.
_Avoid_: base URL, host

**Scope**:
What a Token may do: `admin` (everything), `agent` (author Drafts, dry-run, trigger, read; never publish or activate), `webhook` (start Runs from one webhook Trigger).

**Secret**:
A named sensitive value referenced as `secrets.NAME` in Params, resolved only at execution time and never persisted in Outputs or the Timeline.

**Secret Provider**:
A source Secrets are resolved from: the internal encrypted store first, external managers later. The internal one encrypts under the master key (`BIA_MASTER_KEY`); no route, command or tool returns a value.
