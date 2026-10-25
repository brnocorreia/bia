//! Dry Run: the interpreter's own transitions driven in memory, with side effects Mocked
//! (ADR-0046).
//!
//! Nothing here touches the database. The loop calls the same `start` and `step` transitions a
//! Worker does and keeps the Checkpoint in a local [`Run`], so branches, `if`, retries and
//! failures behave as they do for a real Run. Waits resolve instantly because the loop moves a
//! [`VirtualClock`] to the time the next Job would have become claimable instead of waiting
//! for it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};

use bia_core::action::{ActionDescriptor, ActionError, BoxFuture, CancellationToken, Effect};
use bia_core::clock::Clock;
use bia_core::dsl::{Definition, Step, StepKind};
use bia_store::run::{Run, RunId, RunStatus};
use bia_store::timeline::NewTimelineEntry;
use bia_store::version::VersionNumber;
use chrono::{DateTime, TimeDelta, Utc};
use serde_json::Value;
use uuid::Uuid;

use super::checkpoint::{RunState, checkpoint};
use super::failure::{Failure, FailureCode};
use super::{Interpreter, Outcome};
use crate::{EngineError, SecretResolver};

/// How many transitions a Dry Run takes before it is cut off, so a retry policy asking for
/// millions of Attempts cannot hold a request.
pub const MAX_TRANSITIONS: usize = 10_000;

/// What a Mocked Attempt returns.
#[derive(Debug, Clone, PartialEq)]
pub enum MockOutcome {
    Output(Value),
    Error(ActionError),
}

/// What a Step returns in a Dry Run instead of calling its Action: the Nth Attempt gets the
/// Nth outcome, and the last one repeats once the list is used up.
#[derive(Debug, Clone, PartialEq)]
pub struct Mock {
    attempts: Vec<MockOutcome>,
}

impl Mock {
    /// `None` when `attempts` is empty: a Mock must say what it returns.
    pub fn new(attempts: Vec<MockOutcome>) -> Option<Self> {
        (!attempts.is_empty()).then_some(Self { attempts })
    }

    fn outcome(&self, attempt: u32) -> &MockOutcome {
        let index = usize::try_from(attempt.saturating_sub(1)).unwrap_or(usize::MAX);
        &self.attempts[index.min(self.attempts.len() - 1)]
    }
}

/// Mocks by Step id.
pub type Mocks = BTreeMap<String, Mock>;

#[derive(Debug, Clone)]
pub struct DryRun {
    pub definition: Definition,
    pub version: VersionNumber,
    pub input: Value,
    pub mocks: Mocks,
}

/// How a Dry Run ended and what it recorded, in Timeline order.
#[derive(Debug, Clone, PartialEq)]
pub struct DryRunResult {
    pub status: RunStatus,
    pub timeline: Vec<NewTimelineEntry>,
}

impl Interpreter {
    /// Runs `request.definition` to the end in memory and returns its Timeline.
    ///
    /// Actions of effect `pure` run for real. A `side_effecting` Action returns its Step's
    /// Mock, else its first example's Output, else fails the Attempt with `no_mock`. A Mock on
    /// a Step overrides a `pure` Action too, which lets a caller force a failure. Secrets are
    /// not resolved: `secrets.NAME` reads a placeholder.
    ///
    /// Fails without running when a Mock names a Step the Definition lacks or one that does
    /// not call an Action the engine can Mock (`flow.*` Steps and Blocks).
    pub async fn dry_run(&self, request: DryRun) -> Result<DryRunResult, EngineError> {
        check_mocks(&request.definition, &request.mocks)?;
        let DryRun {
            definition,
            version,
            input,
            mocks,
        } = request;

        let clock = Arc::new(VirtualClock::new(Arc::clone(&self.clock)));
        let dry = Self {
            pool: self.pool.clone(),
            actions: Arc::clone(&self.actions),
            clock: clock.clone(),
            secrets: Arc::new(PlaceholderSecrets),
            config: self.config,
            mocks: Some(mocks),
        };

        let now = clock.now();
        let mut run = Run {
            id: RunId::from_uuid(Uuid::nil()),
            workflow_id: definition.id.clone(),
            version,
            status: RunStatus::Pending,
            input,
            trigger: serde_json::json!({ "type": "dry_run" }),
            checkpoint: checkpoint(None, &RunState::default()),
            created_at: now,
            updated_at: now,
            finished_at: None,
            cancel_requested_at: None,
        };

        let cancel = CancellationToken::new();
        let mut timeline = Vec::new();
        for _ in 0..MAX_TRANSITIONS {
            let transition = if run.status == RunStatus::Pending {
                dry.start(&run, &definition)
            } else {
                dry.step(&run, &definition, &cancel).await?
            };
            timeline.extend(transition.entry);
            run.status = transition.status;
            run.checkpoint = transition.checkpoint;
            if run.status.is_finished() {
                return Ok(DryRunResult {
                    status: run.status,
                    timeline,
                });
            }
            // The Job that continues the Run is due at `next_job_at`; nothing waits for it.
            if let Some(due) = transition.next_job_at {
                clock.advance_to(due);
            }
        }
        Err(EngineError::DryRunTooLong {
            limit: MAX_TRANSITIONS,
        })
    }

    /// The Outcome a Dry Run gives an Attempt instead of calling the Action, or `None` to
    /// call it.
    pub(super) fn mocked(
        &self,
        step: &Step,
        descriptor: &ActionDescriptor,
        attempt: u32,
    ) -> Option<Outcome> {
        let mocks = self.mocks.as_ref()?;
        if let Some(mock) = mocks.get(step.id.as_str()) {
            return Some(match mock.outcome(attempt) {
                MockOutcome::Output(output) => Outcome::Succeeded(output.clone()),
                MockOutcome::Error(error) => Outcome::Failed(Failure::from_action(error.clone())),
            });
        }
        match descriptor.effect {
            Effect::Pure => None,
            Effect::SideEffecting => Some(match descriptor.examples.first() {
                Some(example) => Outcome::Succeeded(example.output.clone()),
                None => Outcome::Failed(Failure::raised(
                    FailureCode::NoMock,
                    ActionError::fatal(format!(
                        "'{}' has side effects, so a Dry Run needs a Mock for Step '{}', and the \
                         Action has no example to use instead",
                        descriptor.name, step.id
                    )),
                )),
            }),
        }
    }
}

/// Rejects Mocks that could never apply, which are nearly always a typo in a Step id.
fn check_mocks(definition: &Definition, mocks: &Mocks) -> Result<(), EngineError> {
    fn collect<'a>(steps: &'a [Step], found: &mut BTreeMap<&'a str, bool>) {
        for step in steps {
            let mockable = matches!(
                &step.kind,
                StepKind::Action { action, .. } if !super::is_flow(action.as_str())
            );
            found.insert(step.id.as_str(), mockable);
            if let StepKind::Branch(branch) = &step.kind {
                collect(&branch.then, found);
                collect(&branch.r#else, found);
            }
        }
    }
    let mut steps = BTreeMap::new();
    collect(&definition.steps, &mut steps);
    let known: BTreeSet<_> = steps.keys().copied().collect();
    for id in mocks.keys() {
        match steps.get(id.as_str()) {
            None => {
                return Err(EngineError::UnknownMockStep {
                    step: id.clone(),
                    known: known.iter().map(|id| (*id).to_owned()).collect(),
                });
            }
            Some(false) => return Err(EngineError::UnmockableStep { step: id.clone() }),
            Some(true) => {}
        }
    }
    Ok(())
}

/// `secrets.NAME` in a Dry Run: no Secret Provider is asked, so a Dry Run neither needs nor
/// reveals a Secret, and a name no Provider knows is not reported.
#[derive(Debug)]
struct PlaceholderSecrets;

impl SecretResolver for PlaceholderSecrets {
    fn resolve<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Option<String>, ActionError>> {
        Box::pin(async move { Ok(Some(format!("<secret {name}>"))) })
    }
}

/// The wrapped Clock's time, moved forward on demand. Timeouts still count real time: a wait
/// for `d` on this Clock is a wait for `d` on the wrapped one.
#[derive(Debug)]
struct VirtualClock {
    base: Arc<dyn Clock>,
    now: Mutex<DateTime<Utc>>,
}

impl VirtualClock {
    fn new(base: Arc<dyn Clock>) -> Self {
        let now = base.now();
        Self {
            base,
            now: Mutex::new(now),
        }
    }

    fn advance_to(&self, to: DateTime<Utc>) {
        let mut now = self.now.lock().unwrap_or_else(PoisonError::into_inner);
        *now = (*now).max(to);
    }
}

impl Clock for VirtualClock {
    fn now(&self) -> DateTime<Utc> {
        *self.now.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn sleep_until(&self, deadline: DateTime<Utc>) -> BoxFuture<'static, ()> {
        let remaining = (deadline - self.now()).max(TimeDelta::zero());
        self.base.sleep_until(self.base.now() + remaining)
    }
}
