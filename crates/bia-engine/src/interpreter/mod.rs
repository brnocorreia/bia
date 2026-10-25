//! The interpreter: advances a Run from its Checkpoint, one transition per Job (ADR-0001).
//!
//! A Worker claims an `advance_run` Job and hands it to [`Interpreter::advance`], which loads
//! the Checkpoint, executes the Step at the Cursor and then, in one transaction, records the
//! Attempt on the Timeline, writes the new Checkpoint, completes the Job and enqueues the next
//! one (ADR-0010). No transaction stays open while an Action runs. A crash between the Action
//! and that commit leaves the Checkpoint as it was, so the Job is claimed again once its Lease
//! expires and the Step re-runs with the same Idempotency Key.
//!
//! A Run flagged for cancellation ends `cancelled` at its next transition, and the Worker
//! cancels the Attempt in flight through the token it passes to
//! [`Interpreter::advance_cancellable`] (ADR-0036).

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
use tracing::Instrument;

use crate::{EngineError, NoSecrets, NotResumable, SecretResolver};

mod checkpoint;
mod failure;
mod limits;

use checkpoint::{Cursor, ELSE, RunState, Sleep, THEN, checkpoint};
use failure::{Failure, FailureCode};
pub use limits::{Limit, LimitExceeded, RunLimits};

/// Settings the interpreter applies where a Definition says nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterpreterConfig {
    pub default_timeout: TimeDelta,
    /// How long a timed-out Action gets to stop after its cancellation token fires, before
    /// it is dropped.
    pub cancel_grace: TimeDelta,
    pub expression_limits: expression::Limits,
    pub limits: RunLimits,
}

impl Default for InterpreterConfig {
    fn default() -> Self {
        Self {
            default_timeout: TimeDelta::minutes(5),
            cancel_grace: TimeDelta::seconds(5),
            expression_limits: expression::Limits::default(),
            limits: RunLimits::default(),
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
    outcome: Outcome,
}

enum Outcome {
    Succeeded(Output),
    Failed(Failure),
    /// The Run was cancelled while the Action ran.
    Cancelled,
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

    pub(crate) fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub(crate) fn clock(&self) -> &dyn Clock {
        &*self.clock
    }

    /// Handles a claimed `advance_run` Job: makes one transition of its Run and commits it
    /// together with the Job's completion.
    ///
    /// A Step that fails is a transition like any other, not an error. An error means nothing
    /// was committed, e.g. [`StoreError::LeaseLost`](bia_store::StoreError::LeaseLost) when
    /// another Worker took the Job over, and the Job is left for its Lease to expire or for
    /// the caller to fail.
    pub async fn advance(&self, claimed: &Claimed) -> Result<Advanced, EngineError> {
        self.advance_cancellable(claimed, &CancellationToken::new())
            .await
    }

    /// [`advance`](Self::advance), where `cancel` firing stops the Attempt in flight: its
    /// Action's cancellation token fires, and the Run ends `cancelled` without a later Step
    /// running. Fire it only for a Run flagged for cancellation.
    pub async fn advance_cancellable(
        &self,
        claimed: &Claimed,
        cancel: &CancellationToken,
    ) -> Result<Advanced, EngineError> {
        let run_id = claimed
            .job
            .run_id
            .ok_or(EngineError::JobWithoutRun(claimed.job.id))?;
        self.advance_run(run_id, &claimed.lease, cancel)
            .instrument(tracing::info_span!("run", run_id = %run_id))
            .await
    }

    async fn advance_run(
        &self,
        run_id: RunId,
        lease: &Lease,
        cancel: &CancellationToken,
    ) -> Result<Advanced, EngineError> {
        let (run, definition) = self.load(run_id).await?;

        let transition = match run.status {
            status if status.is_finished() => {
                // A Run that already ended needs nothing more; only the Job completes.
                let mut tx = self.pool.begin().await?;
                PgJobRepository.ack(&mut tx, lease).await?;
                tx.commit().await?;
                return Ok(Advanced {
                    run: run.id,
                    status,
                    next_job_at: None,
                });
            }
            _ if run.cancel_requested_at.is_some() => self.cancelled(&run),
            RunStatus::Pending => self.start(&run, &definition),
            _ => self.step(&run, &definition, cancel).await?,
        };

        let advanced = Advanced {
            run: run.id,
            status: transition.status,
            next_job_at: transition.next_job_at,
        };
        let recorded = transition
            .entry
            .as_ref()
            .map(|entry| (entry.step.clone(), entry.status));
        self.commit(lease, run.id, transition).await?;
        log_transition(&advanced, recorded);
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

    /// Ends a Run flagged for cancellation between Attempts. The Cursor stays where it was.
    fn cancelled(&self, run: &Run) -> Transition {
        let now = self.clock.now();
        Transition {
            status: RunStatus::Cancelled,
            checkpoint: run.checkpoint.clone(),
            entry: Some(NewTimelineEntry {
                step: None,
                status: AttemptStatus::Cancelled,
                input: None,
                output: None,
                error: None,
                started_at: now,
                finished_at: now,
            }),
            next_job_at: None,
            at: now,
        }
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

    async fn step(
        &self,
        run: &Run,
        definition: &Definition,
        cancel: &CancellationToken,
    ) -> Result<Transition, EngineError> {
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
        let limits = self.config.limits.for_definition(definition);

        // A retry or a wake-up continues a Step the Run already took up.
        if state.attempts == 0 && state.sleep.is_none() {
            state.steps += 1;
            if let Err(exceeded) = limits.check_steps(state.steps) {
                return Ok(self.fail_step(started, state, None, exceeded.failure()));
            }
        }

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
                let Attempted { input, outcome } = self
                    .attempt(run, step, started.attempt, &state, cancel)
                    .await;
                Ok(match outcome {
                    Outcome::Succeeded(output) => {
                        if let Err(exceeded) = limits.check_output(&output) {
                            return Ok(self.fail_step(started, state, input, exceeded.failure()));
                        }
                        let mut entry = started.entry(AttemptStatus::Succeeded, self.clock.now());
                        entry.input = input;
                        entry.output = Some(output.clone());
                        state.outputs.insert(step.id.as_str().to_owned(), output);
                        self.advance_past(definition, started.cursor, state, entry)
                    }
                    Outcome::Failed(failure) => self.fail_step(started, state, input, failure),
                    Outcome::Cancelled => self.cancel_step(started, state, input),
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
        state.finish_step();
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
                state.finish_step();
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
        state.finish_step();
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
        state.finish_step();
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
        // A resume gives the Step a fresh budget while Attempt numbers keep counting up.
        let used = started.attempt - state.attempts_before_resume;
        let (status, next_job_at) = if failure.is_retryable() && used < max_attempts {
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

    /// The transition after the Run was cancelled during an Attempt: the Run ends `cancelled`
    /// with its Cursor on this Step.
    fn cancel_step(
        &self,
        started: Started<'_>,
        mut state: RunState,
        input: Option<Value>,
    ) -> Transition {
        let finished_at = self.clock.now();
        let mut entry = started.entry(AttemptStatus::Cancelled, finished_at);
        entry.input = input;
        state.attempts = started.attempt;
        Transition {
            status: RunStatus::Cancelled,
            checkpoint: checkpoint(Some(&started.cursor), &state),
            entry: Some(entry),
            next_job_at: None,
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

    async fn attempt(
        &self,
        run: &Run,
        step: &Step,
        attempt: u32,
        state: &RunState,
        cancel: &CancellationToken,
    ) -> Attempted {
        let (action, params) = match &step.kind {
            StepKind::Action { action, params } => (action, params),
            _ => {
                return Attempted {
                    input: None,
                    outcome: Outcome::Failed(Failure::raised(
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
            outcome: Outcome::Failed(Failure::raised(code, error)),
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
                        outcome: Outcome::Failed(Failure::from_action(error)),
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

        tracing::debug!(
            step_id = %step.id,
            attempt,
            action = %action.descriptor().name,
            "attempt started"
        );
        Attempted {
            input: recorded,
            outcome: self.call(&*action, call, timeout, cancel).await,
        }
    }

    async fn call(
        &self,
        action: &dyn bia_core::action::Action,
        call: ActionCall,
        timeout: TimeDelta,
        run_cancel: &CancellationToken,
    ) -> Outcome {
        // A child token fires with the Run's cancellation, and on its own for a timeout.
        let cancel = run_cancel.child_token();
        let deadline = self.clock.now() + timeout;
        let mut execution = action.execute(call, cancel.clone());
        tokio::select! {
            biased;
            () = run_cancel.cancelled() => {
                self.stop(execution, &cancel).await;
                Outcome::Cancelled
            }
            result = &mut execution => match result {
                Ok(output) => Outcome::Succeeded(output),
                Err(error) => Outcome::Failed(Failure::from_action(error)),
            },
            () = self.clock.sleep_until(deadline) => {
                self.stop(execution, &cancel).await;
                Outcome::Failed(Failure::raised(
                    FailureCode::Timeout,
                    ActionError::retryable(format!(
                        "Attempt timed out after {}",
                        humanize(timeout)
                    )),
                ))
            }
        }
    }

    /// Cancels an Action and gives it a moment to stop cooperatively. Whatever it returns then
    /// does not decide the outcome, and dropping it stops it anyway.
    async fn stop(
        &self,
        execution: bia_core::action::BoxFuture<'_, Result<Output, ActionError>>,
        cancel: &CancellationToken,
    ) {
        cancel.cancel();
        let grace = self.clock.now() + self.config.cancel_grace;
        tokio::select! {
            _ = execution => {}
            () = self.clock.sleep_until(grace) => {}
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

/// Requests cancellation of a Run at `at` (ADR-0036). The Run ends `cancelled` at its next
/// transition: a Worker running one of its Attempts cancels it, and a Run waiting on a delayed
/// Job, or on none, gets a Job due at `at`. Returns `false`, changing nothing, when the Run
/// already finished.
pub async fn cancel_run(
    conn: &mut PgConnection,
    run: RunId,
    at: DateTime<Utc>,
) -> Result<bool, EngineError> {
    let mut tx = conn.begin().await?;
    if !PgRunRepository.request_cancel(&mut tx, run, at).await? {
        return Ok(false);
    }
    let live = PgJobRepository
        .list_for_run(&mut tx, run)
        .await?
        .into_iter()
        .any(|job| job.failed_at.is_none());
    if live {
        PgJobRepository.bring_forward(&mut tx, run, at).await?;
    } else {
        PgJobRepository
            .enqueue(&mut tx, &NewJob::advance_run(run, at))
            .await?;
    }
    tx.commit().await?;
    Ok(true)
}

/// Puts a `failed` Run back to work on the Step it failed at, on its own Version, and enqueues
/// the Job that continues it (ADR-0045). Steps that succeeded do not run again.
pub async fn resume_run(
    conn: &mut PgConnection,
    run: RunId,
    at: DateTime<Utc>,
) -> Result<Run, EngineError> {
    let mut tx = conn.begin().await?;
    let current = PgRunRepository
        .get(&mut tx, run)
        .await?
        .ok_or(EngineError::RunNotFound(run))?;
    let not_resumable = |reason| EngineError::NotResumable { run, reason };
    if current.status != RunStatus::Failed {
        return Err(not_resumable(NotResumable::NotFailed(current.status)));
    }
    if current.checkpoint.cursor.is_empty() {
        return Err(not_resumable(NotResumable::NoStep));
    }
    let mut state = RunState::decode(run, &current.checkpoint.state)?;
    state.attempts_before_resume = state.attempts;
    let checkpoint = Checkpoint {
        cursor: current.checkpoint.cursor,
        state: serde_json::to_value(&state).unwrap_or_else(|_| json!({})),
    };
    let resumed = PgRunRepository
        .resume(&mut tx, run, &checkpoint, at)
        .await?
        .ok_or_else(|| not_resumable(NotResumable::NotFailed(RunStatus::Running)))?;
    PgJobRepository
        .enqueue(&mut tx, &NewJob::advance_run(run, at))
        .await?;
    tx.commit().await?;
    Ok(resumed)
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

/// Logs a committed transition inside the Run's span, so every line carries its Run id.
/// Inputs, Outputs and errors stay on the Timeline: they are not for logs.
fn log_transition(advanced: &Advanced, recorded: Option<(Option<StepAttempt>, AttemptStatus)>) {
    if let Some((step, status)) = recorded {
        let status = status.as_str();
        match step {
            Some(step) => tracing::info!(
                step_id = %step.step_id,
                attempt = step.attempt,
                action = %step.action,
                status,
                "attempt finished"
            ),
            None => tracing::info!(status, "run entry recorded"),
        }
    }
    let status = advanced.status.as_str();
    if advanced.status.is_finished() {
        tracing::info!(status, "run finished");
    } else {
        tracing::debug!(status, next_job_at = ?advanced.next_job_at, "run advanced");
    }
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
