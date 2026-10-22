//! The interpreter: advances a Run from its Checkpoint, one transition per Job (ADR-0001).
//!
//! A Worker claims an `advance_run` Job and hands it to [`Interpreter::advance`], which loads
//! the Checkpoint, executes the Step at the Cursor and then, in one transaction, records the
//! Attempt on the Timeline, writes the new Checkpoint, completes the Job and enqueues the next
//! one (ADR-0010). No transaction stays open while an Action runs. A crash between the Action
//! and that commit leaves the Checkpoint as it was, so the Job is claimed again once its Lease
//! expires and the Step re-runs with the same Idempotency Key.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

use bia_core::action::flow::{self, Control};
use bia_core::action::{
    ActionCall, ActionError, ActionRegistry, CancellationToken, IdempotencyKey, Lookup, Output,
    RunContext,
};
use bia_core::clock::Clock;
use bia_core::dsl::{Branch, Definition, Params, RetryPolicy, Step, StepKind, WorkflowId};
use bia_core::expression::{self, Scope, Template, resolve_params};
use bia_core::validation::schema_violations;
use bia_store::job::{Claimed, JobRepository, Lease, NewJob, PgJobRepository};
use bia_store::run::{Checkpoint, NewRun, PgRunRepository, Run, RunId, RunRepository, RunStatus};
use bia_store::timeline::{
    AttemptStatus, NewTimelineEntry, PgTimelineRepository, StepAttempt, TimelineRepository,
};
use bia_store::version::{PgVersionRepository, VersionNumber, VersionRepository};
use chrono::{DateTime, TimeDelta, Utc};
use serde_json::{Value, json};
use sqlx::{Acquire, PgConnection, PgPool};

use crate::{EngineError, NoSecrets, SecretResolver};

mod checkpoint;
mod failure;

use checkpoint::{Cursor, ELSE, RunState, Sleep, THEN, checkpoint};
use failure::{Failure, FailureCode};

/// Settings the interpreter applies where a Definition says nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterpreterConfig {
    pub default_timeout: TimeDelta,
    /// How long a timed-out Action gets to stop after its cancellation token fires, before
    /// it is dropped.
    pub cancel_grace: TimeDelta,
    pub expression_limits: expression::Limits,
}

impl Default for InterpreterConfig {
    fn default() -> Self {
        Self {
            default_timeout: TimeDelta::minutes(5),
            cancel_grace: TimeDelta::seconds(5),
            expression_limits: expression::Limits::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Advanced {
    pub run: RunId,
    pub status: RunStatus,
    /// When the Job that continues the Run becomes claimable; `None` once the Run finished.
    pub next_job_at: Option<DateTime<Utc>>,
}

/// Advances Runs from their Checkpoints. The Action Registry, the Clock and the Secret
/// resolver are injected.
pub struct Interpreter {
    pool: PgPool,
    actions: Arc<dyn ActionRegistry>,
    clock: Arc<dyn Clock>,
    secrets: Arc<dyn SecretResolver>,
    config: InterpreterConfig,
}

impl fmt::Debug for Interpreter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Interpreter")
            .field("clock", &self.clock)
            .field("secrets", &self.secrets)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

struct Transition {
    status: RunStatus,
    checkpoint: Checkpoint,
    entry: Option<NewTimelineEntry>,
    next_job_at: Option<DateTime<Utc>>,
    at: DateTime<Utc>,
}

struct Started<'a> {
    step: &'a Step,
    cursor: Cursor,
    /// 1 for the first Attempt of the Step.
    attempt: u32,
    started_at: DateTime<Utc>,
}

impl Started<'_> {
    fn entry(&self, status: AttemptStatus, finished_at: DateTime<Utc>) -> NewTimelineEntry {
        NewTimelineEntry {
            step: Some(StepAttempt {
                step_id: self.step.id.clone(),
                iteration_path: String::new(),
                attempt: i32::try_from(self.attempt).unwrap_or(i32::MAX),
                action: action_name(self.step),
            }),
            status,
            input: None,
            output: None,
            error: None,
            started_at: self.started_at,
            finished_at,
        }
    }
}

fn is_flow(name: &str) -> bool {
    matches!(
        name,
        flow::SLEEP | flow::SLEEP_UNTIL | flow::FAIL | flow::SUCCEED
    )
}

struct Attempted {
    input: Option<Value>,
    result: Result<Output, Failure>,
}

impl Interpreter {
    pub fn new(pool: PgPool, actions: Arc<dyn ActionRegistry>, clock: Arc<dyn Clock>) -> Self {
        Self {
            pool,
            actions,
            clock,
            secrets: Arc::new(NoSecrets),
            config: InterpreterConfig::default(),
        }
    }

    pub fn with_secrets(mut self, secrets: Arc<dyn SecretResolver>) -> Self {
        self.secrets = secrets;
        self
    }

    pub fn with_config(mut self, config: InterpreterConfig) -> Self {
        self.config = config;
        self
    }

    /// Handles a claimed `advance_run` Job: makes one transition of its Run and commits it
    /// together with the Job's completion.
    ///
    /// A Step that fails is a transition like any other, not an error. An error means nothing
    /// was committed, e.g. [`StoreError::LeaseLost`](bia_store::StoreError::LeaseLost) when
    /// another Worker took the Job over, and the Job is left for its Lease to expire or for
    /// the caller to fail.
    pub async fn advance(&self, claimed: &Claimed) -> Result<Advanced, EngineError> {
        let run_id = claimed
            .job
            .run_id
            .ok_or(EngineError::JobWithoutRun(claimed.job.id))?;
        let (run, definition) = self.load(run_id).await?;

        let transition = match run.status {
            status if status.is_finished() => {
                // A Run that already ended needs nothing more; only the Job completes.
                let mut tx = self.pool.begin().await?;
                PgJobRepository.ack(&mut tx, &claimed.lease).await?;
                tx.commit().await?;
                return Ok(Advanced {
                    run: run.id,
                    status,
                    next_job_at: None,
                });
            }
            RunStatus::Pending => self.start(&run, &definition),
            _ => self.step(&run, &definition).await?,
        };

        let advanced = Advanced {
            run: run.id,
            status: transition.status,
            next_job_at: transition.next_job_at,
        };
        self.commit(&claimed.lease, run.id, transition).await?;
        Ok(advanced)
    }

    async fn load(&self, run_id: RunId) -> Result<(Run, Definition), EngineError> {
        let mut conn = self.pool.acquire().await?;
        let run = PgRunRepository
            .get(&mut conn, run_id)
            .await?
            .ok_or(EngineError::RunNotFound(run_id))?;
        let version = PgVersionRepository
            .get(&mut conn, &run.workflow_id, run.version)
            .await?
            .ok_or_else(|| EngineError::VersionNotFound {
                run: run.id,
                workflow: run.workflow_id.clone(),
                version: run.version.get(),
            })?;
        Ok((run, version.definition))
    }

    /// `pending → running`, or `failed` when the input does not match the `input_schema`.
    fn start(&self, run: &Run, definition: &Definition) -> Transition {
        let now = self.clock.now();
        if let Some(failure) = input_failure(definition, &run.input) {
            return Transition {
                status: RunStatus::Failed,
                checkpoint: checkpoint(None, &RunState::default()),
                entry: Some(NewTimelineEntry {
                    step: None,
                    status: AttemptStatus::Failed,
                    input: Some(run.input.clone()),
                    output: None,
                    error: Some(failure.to_value()),
                    started_at: now,
                    finished_at: now,
                }),
                next_job_at: None,
                at: now,
            };
        }
        self.at(Cursor::start(definition), RunState::default(), None, now)
    }

    async fn step(&self, run: &Run, definition: &Definition) -> Result<Transition, EngineError> {
        let cursor = Cursor::decode(run.id, &run.checkpoint.cursor, definition)?;
        let mut state = RunState::decode(run.id, &run.checkpoint.state)?;
        let step = cursor
            .step(definition)
            .ok_or_else(|| EngineError::InvalidCheckpoint {
                run: run.id,
                message: "cursor names no Step".to_owned(),
            })?;
        let started = Started {
            step,
            attempt: state.attempts + 1,
            started_at: self.clock.now(),
            cursor,
        };

        // A Run woken from a sleep already passed its `if` when it fell asleep.
        if state.sleep.is_none()
            && let Some(condition) = &step.r#if
        {
            match self.condition(condition, run, started.attempt, &state) {
                Ok(true) => {}
                Ok(false) => {
                    let entry = started.entry(AttemptStatus::Skipped, self.clock.now());
                    state
                        .outputs
                        .insert(step.id.as_str().to_owned(), Value::Null);
                    return Ok(self.advance_past(definition, started.cursor, state, entry));
                }
                Err(error) => {
                    let message = format!("`if` failed to evaluate: {error}");
                    let failure = Failure::raised(
                        FailureCode::InvalidExpression,
                        ActionError::fatal(message),
                    );
                    return Ok(self.fail_step(started, state, None, failure));
                }
            }
        }

        match &step.kind {
            StepKind::Branch(branch) => Ok(self.branch(run, definition, started, state, branch)),
            StepKind::Action { action, params } if is_flow(action.as_str()) => {
                Ok(self.control(run, definition, started, state, params))
            }
            _ => {
                let Attempted { input, result } =
                    self.attempt(run, step, started.attempt, &state).await;
                Ok(match result {
                    Ok(output) => {
                        let mut entry = started.entry(AttemptStatus::Succeeded, self.clock.now());
                        entry.input = input;
                        entry.output = Some(output.clone());
                        state.outputs.insert(step.id.as_str().to_owned(), output);
                        self.advance_past(definition, started.cursor, state, entry)
                    }
                    Err(failure) => self.fail_step(started, state, input, failure),
                })
            }
        }
    }

    fn condition(
        &self,
        condition: &bia_core::dsl::Expression,
        run: &Run,
        attempt: u32,
        state: &RunState,
    ) -> Result<bool, String> {
        let limits = &self.config.expression_limits;
        let expression = expression::Expression::compile_with(condition.as_str(), limits)
            .map_err(|error| error.to_string())?;
        expression
            .evaluate_condition(&scope(run, attempt, state))
            .map_err(|error| error.to_string())
    }

    /// `flow.branch`: evaluates `when` and moves the Cursor into the chosen arm. The Block's
    /// Output says which arm ran. When the arm is empty the Run carries on after the Block.
    fn branch(
        &self,
        run: &Run,
        definition: &Definition,
        started: Started<'_>,
        mut state: RunState,
        branch: &Branch,
    ) -> Transition {
        let taken = match self.condition(&branch.when, run, started.attempt, &state) {
            Ok(taken) => taken,
            Err(error) => {
                let message = format!("`when` failed to evaluate: {error}");
                let failure =
                    Failure::raised(FailureCode::InvalidExpression, ActionError::fatal(message));
                return self.fail_step(started, state, None, failure);
            }
        };
        let arm = if taken { THEN } else { ELSE };
        let output = json!({ "arm": arm });
        let mut entry = started.entry(AttemptStatus::Succeeded, self.clock.now());
        entry.input = Some(json!({ "when": taken }));
        entry.output = Some(output.clone());
        state
            .outputs
            .insert(started.step.id.as_str().to_owned(), output);
        state.attempts = 0;
        let at = entry.finished_at;
        let next = started.cursor.enter(definition, arm);
        self.at(next, state, Some(entry), at)
    }

    /// `flow.sleep`, `flow.sleep_until`, `flow.fail` and `flow.succeed`: the Actions that act on
    /// the Run itself. Their Params are resolved without Secrets, so no error or Output can
    /// leak one.
    fn control(
        &self,
        run: &Run,
        definition: &Definition,
        started: Started<'_>,
        mut state: RunState,
        params: &Params,
    ) -> Transition {
        let now = started.started_at;
        if let Some(sleep) = state.sleep.take() {
            return self.wake(definition, started, state, sleep);
        }
        let StepKind::Action { action, .. } = &started.step.kind else {
            unreachable!("only Action Steps reach `control`");
        };
        let fail = |this: &Self, started, state, input, code, error: ActionError| {
            let failure = Failure::raised(code, error);
            this.fail_step(started, state, input, failure)
        };
        let resolved = match resolve_params(
            params,
            &scope(run, started.attempt, &state),
            &self.config.expression_limits,
        ) {
            Ok(resolved) => resolved,
            Err(error) => {
                let error = ActionError::fatal(error.to_string());
                let input = Some(Value::Object(params.clone()));
                return fail(
                    self,
                    started,
                    state,
                    input,
                    FailureCode::InvalidTemplate,
                    error,
                );
            }
        };
        let input = Some(Value::Object(resolved.recorded));
        let control = match Control::from_params(action.as_str(), &resolved.call) {
            Some(Ok(control)) => control,
            Some(Err(error)) => {
                return fail(
                    self,
                    started,
                    state,
                    input,
                    FailureCode::InvalidParams,
                    error,
                );
            }
            None => unreachable!("`is_flow` names are all flow Actions"),
        };

        let until = match control {
            Control::Fail(message) => {
                return fail(
                    self,
                    started,
                    state,
                    input,
                    FailureCode::FlowFail,
                    ActionError::fatal(message),
                );
            }
            Control::Succeed => {
                let mut entry = started.entry(AttemptStatus::Succeeded, now);
                entry.input = input;
                entry.output = Some(Value::Null);
                state
                    .outputs
                    .insert(started.step.id.as_str().to_owned(), Value::Null);
                state.attempts = 0;
                return self.at(None, state, Some(entry), now);
            }
            Control::Sleep(duration) => {
                TimeDelta::try_milliseconds(i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
                    .and_then(|duration| now.checked_add_signed(duration))
            }
            Control::SleepUntil(until) => Some(until),
        };
        let Some(until) = until else {
            let error = ActionError::fatal("the sleep ends beyond the dates a timestamp can hold");
            return fail(
                self,
                started,
                state,
                input,
                FailureCode::InvalidParams,
                error,
            );
        };

        let sleep = Sleep::new(now, until, input);
        if until <= now {
            return self.wake(definition, started, state, sleep);
        }
        // Fall asleep: the Cursor stays on this Step, and the next Job only becomes claimable
        // when it is time to wake, so no Worker holds the Run meanwhile.
        state.sleep = Some(sleep);
        state.attempts = 0;
        Transition {
            status: RunStatus::Sleeping,
            checkpoint: checkpoint(Some(&started.cursor), &state),
            entry: None,
            next_job_at: Some(until),
            at: now,
        }
    }

    /// Finishes a sleep, once its time has come. A Job claimed early goes back to sleep.
    fn wake(
        &self,
        definition: &Definition,
        mut started: Started<'_>,
        mut state: RunState,
        sleep: Sleep,
    ) -> Transition {
        let now = started.started_at;
        state.sleep = Some(sleep.clone());
        if now < sleep.until() {
            return Transition {
                status: RunStatus::Sleeping,
                checkpoint: checkpoint(Some(&started.cursor), &state),
                entry: None,
                next_job_at: Some(sleep.until()),
                at: now,
            };
        }
        started.started_at = sleep.started_at();
        let output = json!({ "slept_until": sleep.until().to_rfc3339() });
        let mut entry = started.entry(AttemptStatus::Succeeded, now);
        entry.input = sleep.input().cloned();
        entry.output = Some(output.clone());
        state.sleep = None;
        state
            .outputs
            .insert(started.step.id.as_str().to_owned(), output);
        self.advance_past(definition, started.cursor, state, entry)
    }

    fn advance_past(
        &self,
        definition: &Definition,
        cursor: Cursor,
        mut state: RunState,
        entry: NewTimelineEntry,
    ) -> Transition {
        state.attempts = 0;
        state.sleep = None;
        let at = entry.finished_at;
        self.at(cursor.after(definition), state, Some(entry), at)
    }

    /// The transition after a Step's Attempt failed: a retry when the error is `retryable` and
    /// the Step's policy allows another Attempt, otherwise the Run fails at this Step.
    fn fail_step(
        &self,
        started: Started<'_>,
        mut state: RunState,
        input: Option<Value>,
        failure: Failure,
    ) -> Transition {
        let finished_at = self.clock.now();
        let mut entry = started.entry(AttemptStatus::Failed, finished_at);
        entry.input = input;
        entry.error = Some(failure.to_value());
        state.attempts = started.attempt;
        let policy = started.step.retry.as_ref();
        let max_attempts = policy.map_or(1, |policy| policy.max_attempts);
        let (status, next_job_at) = if failure.is_retryable() && started.attempt < max_attempts {
            let delay = policy.map_or(TimeDelta::zero(), |p| retry_delay(p, started.attempt));
            (RunStatus::Running, Some(finished_at + delay))
        } else {
            (RunStatus::Failed, None)
        };
        Transition {
            status,
            checkpoint: checkpoint(Some(&started.cursor), &state),
            entry: Some(entry),
            next_job_at,
            at: finished_at,
        }
    }

    fn at(
        &self,
        cursor: Option<Cursor>,
        state: RunState,
        entry: Option<NewTimelineEntry>,
        at: DateTime<Utc>,
    ) -> Transition {
        let finished = cursor.is_none();
        Transition {
            status: if finished {
                RunStatus::Succeeded
            } else {
                RunStatus::Running
            },
            checkpoint: checkpoint(cursor.as_ref(), &state),
            entry,
            next_job_at: (!finished).then_some(at),
            at,
        }
    }

    async fn attempt(&self, run: &Run, step: &Step, attempt: u32, state: &RunState) -> Attempted {
        let (action, params) = match &step.kind {
            StepKind::Action { action, params } => (action, params),
            _ => {
                return Attempted {
                    input: None,
                    result: Err(Failure::raised(
                        FailureCode::UnsupportedStep,
                        ActionError::fatal(format!(
                            "Step '{}' is a Block this build cannot run yet",
                            step.id
                        )),
                    )),
                };
            }
        };
        let written = Some(Value::Object(params.clone()));
        let fail = |input: Option<Value>, code, error| Attempted {
            input,
            result: Err(Failure::raised(code, error)),
        };

        let action = match self.actions.lookup(action.as_str()) {
            Lookup::Enabled(action) => action,
            Lookup::Disabled(_) => {
                let message = format!("Action '{action}' is disabled on this server");
                return fail(
                    written,
                    FailureCode::DisabledAction,
                    ActionError::fatal(message),
                );
            }
            Lookup::Unknown => {
                let message = format!("Action '{action}' is not in the Action Registry");
                return fail(
                    written,
                    FailureCode::UnknownAction,
                    ActionError::fatal(message),
                );
            }
        };

        let mut scope = scope(run, attempt, state);
        for name in secrets_read(params) {
            match self.secrets.resolve(&name).await {
                Ok(Some(value)) => scope = scope.with_secret(name, value),
                Ok(None) => {
                    let message = format!("Secret '{name}' is not defined");
                    return fail(
                        written,
                        FailureCode::UnknownSecret,
                        ActionError::fatal(message),
                    );
                }
                Err(error) => {
                    return Attempted {
                        input: written,
                        result: Err(Failure::from_action(error)),
                    };
                }
            }
        }

        let resolved = match resolve_params(params, &scope, &self.config.expression_limits) {
            Ok(resolved) => resolved,
            Err(error) => {
                let failure = ActionError::fatal(error.to_string());
                return fail(written, FailureCode::InvalidTemplate, failure);
            }
        };
        let recorded = Some(Value::Object(resolved.recorded));

        let schema = action.descriptor().param_schema.as_value();
        let violations = match schema_violations(schema, &Value::Object(resolved.call.clone())) {
            Ok(violations) => violations,
            Err(error) => {
                let failure = ActionError::fatal(error.to_string());
                return fail(recorded, FailureCode::InvalidParams, failure);
            }
        };
        if !violations.is_empty() {
            let failure = ActionError::fatal("Params do not match the Action's param schema")
                .with_details(json!({ "violations": violations }));
            return fail(recorded, FailureCode::InvalidParams, failure);
        }

        let call = ActionCall {
            params: resolved.call,
            idempotency_key: idempotency_key(run.id, step, ""),
            context: RunContext {
                run_id: run.id.to_string(),
                workflow_id: run.workflow_id.clone(),
                version: u32::try_from(run.version.get()).unwrap_or_default(),
                step_id: step.id.clone(),
                attempt,
            },
        };
        let timeout = step
            .timeout
            .and_then(|timeout| TimeDelta::from_std(timeout.to_std()).ok())
            .unwrap_or(self.config.default_timeout);

        Attempted {
            input: recorded,
            result: self.call(&*action, call, timeout).await,
        }
    }

    async fn call(
        &self,
        action: &dyn bia_core::action::Action,
        call: ActionCall,
        timeout: TimeDelta,
    ) -> Result<Output, Failure> {
        let cancel = CancellationToken::new();
        let deadline = self.clock.now() + timeout;
        let mut execution = action.execute(call, cancel.clone());
        tokio::select! {
            result = &mut execution => result.map_err(Failure::from_action),
            () = self.clock.sleep_until(deadline) => {
                cancel.cancel();
                // The Action gets a moment to stop cooperatively; whatever it returns now
                // does not decide the outcome, and dropping it stops it anyway.
                let grace = self.clock.now() + self.config.cancel_grace;
                tokio::select! {
                    _ = execution => {}
                    () = self.clock.sleep_until(grace) => {}
                }
                Err(Failure::raised(
                    FailureCode::Timeout,
                    ActionError::retryable(format!(
                        "Attempt timed out after {}",
                        humanize(timeout)
                    )),
                ))
            }
        }
    }

    async fn commit(
        &self,
        lease: &Lease,
        run: RunId,
        transition: Transition,
    ) -> Result<(), EngineError> {
        let mut tx = self.pool.begin().await?;
        PgJobRepository.ack(&mut tx, lease).await?;
        if let Some(entry) = &transition.entry {
            PgTimelineRepository.append(&mut tx, run, entry).await?;
        }
        PgRunRepository
            .update_checkpoint(
                &mut tx,
                run,
                transition.status,
                &transition.checkpoint,
                transition.at,
            )
            .await?;
        if let Some(run_at) = transition.next_job_at {
            PgJobRepository
                .enqueue(&mut tx, &NewJob::advance_run(run, run_at))
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

/// Creates a `pending` Run of a Published Version at `at`, with the Job that starts it. Both
/// commit together.
pub async fn start_run(
    conn: &mut PgConnection,
    workflow_id: &WorkflowId,
    version: VersionNumber,
    input: Value,
    trigger: Value,
    at: DateTime<Utc>,
) -> Result<Run, EngineError> {
    let mut tx = conn.begin().await?;
    let run = PgRunRepository
        .create(
            &mut tx,
            &NewRun {
                workflow_id: workflow_id.clone(),
                version,
                input,
                trigger,
                checkpoint: checkpoint(None, &RunState::default()),
            },
            at,
        )
        .await?;
    PgJobRepository
        .enqueue(&mut tx, &NewJob::advance_run(run.id, at))
        .await?;
    tx.commit().await?;
    Ok(run)
}

fn input_failure(definition: &Definition, input: &Value) -> Option<Failure> {
    let schema = Value::Object(definition.input_schema.clone()?);
    let error = match schema_violations(&schema, input) {
        Ok(violations) if violations.is_empty() => return None,
        Ok(violations) => ActionError::fatal("Run input does not match the input_schema")
            .with_details(json!({ "violations": violations })),
        Err(error) => ActionError::fatal(format!("Run input cannot be checked: {error}")),
    };
    Some(Failure::raised(FailureCode::InvalidInput, error))
}

/// What an Attempt's Expressions can read (ADR-0011). Secrets are added only once resolved.
fn scope(run: &Run, attempt: u32, state: &RunState) -> Scope {
    let mut scope = Scope::new()
        .with_input(&run.input)
        .with_trigger(&run.trigger)
        .with_run(&json!({
            "id": run.id.to_string(),
            "version": run.version.get(),
            "attempt": attempt,
            "started_at": run.created_at.to_rfc3339(),
        }));
    for (id, output) in &state.outputs {
        scope = scope.with_step_output(id.as_str(), output);
    }
    scope
}

/// The Secret names Templates in `params` read. Templates that do not parse read nothing;
/// resolving them reports the error.
fn secrets_read(params: &Params) -> BTreeSet<String> {
    fn visit(value: &Value, found: &mut BTreeSet<String>) {
        match value {
            Value::String(source) if source.contains("{{") => {
                if let Ok(template) = Template::parse(source) {
                    found.extend(template.references().secrets.iter().cloned());
                }
            }
            Value::Array(items) => items.iter().for_each(|item| visit(item, found)),
            Value::Object(map) => map.values().for_each(|item| visit(item, found)),
            _ => {}
        }
    }
    let mut found = BTreeSet::new();
    params.values().for_each(|value| visit(value, &mut found));
    found
}

/// `run_id:step_id:iteration_path`, the same for every Attempt of a Step (ADR-0010).
fn idempotency_key(run: RunId, step: &Step, iteration_path: &str) -> IdempotencyKey {
    IdempotencyKey::new(format!("{run}:{}:{iteration_path}", step.id))
}

fn retry_delay(policy: &RetryPolicy, attempt: u32) -> TimeDelta {
    let millis = |duration: bia_core::dsl::Duration| {
        TimeDelta::try_milliseconds(i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
            .unwrap_or(TimeDelta::MAX)
    };
    let initial = policy.initial.map_or(TimeDelta::seconds(1), millis);
    let max = policy.max.map_or(TimeDelta::hours(1), millis);
    match policy
        .backoff
        .unwrap_or(bia_core::dsl::Backoff::Exponential)
    {
        bia_core::dsl::Backoff::Fixed => initial.min(max),
        bia_core::dsl::Backoff::Exponential => bia_store::job::Backoff { initial, max }
            .delay(i32::try_from(attempt).unwrap_or(i32::MAX)),
    }
}

fn action_name(step: &Step) -> bia_core::dsl::ActionName {
    match &step.kind {
        StepKind::Action { action, .. } => action.clone(),
        kind => bia_core::dsl::ActionName::new(kind.action())
            .unwrap_or_else(|_| unreachable!("Block Action names are valid")),
    }
}

fn humanize(duration: TimeDelta) -> String {
    let millis = duration.num_milliseconds();
    if millis % 1000 == 0 {
        format!("{}s", millis / 1000)
    } else {
        format!("{millis}ms")
    }
}

#[cfg(test)]
mod tests {
    use bia_core::dsl::{Backoff, Duration, RetryPolicy};
    use chrono::TimeDelta;

    use super::retry_delay;

    fn policy(backoff: Option<Backoff>, initial: Option<&str>, max: Option<&str>) -> RetryPolicy {
        let parse = |d: &str| d.parse::<Duration>().unwrap();
        RetryPolicy {
            max_attempts: 10,
            backoff,
            initial: initial.map(parse),
            max: max.map(parse),
        }
    }

    #[test]
    fn retry_delays_follow_the_backoff_up_to_its_max() {
        let exponential = policy(Some(Backoff::Exponential), Some("2s"), Some("10s"));
        let delays: Vec<_> = (1..=5).map(|n| retry_delay(&exponential, n)).collect();
        assert_eq!(
            delays,
            [2, 4, 8, 10, 10].map(TimeDelta::seconds),
            "exponential doubles, capped at max"
        );

        let fixed = policy(Some(Backoff::Fixed), Some("500ms"), None);
        assert_eq!(retry_delay(&fixed, 1), TimeDelta::milliseconds(500));
        assert_eq!(retry_delay(&fixed, 7), TimeDelta::milliseconds(500));

        let defaults = policy(None, None, None);
        assert_eq!(retry_delay(&defaults, 1), TimeDelta::seconds(1));
        assert_eq!(retry_delay(&defaults, 3), TimeDelta::seconds(4));
        assert_eq!(retry_delay(&defaults, 30), TimeDelta::hours(1));
    }
}
