//! The harness the Worker tests share: a published Workflow, Workers on a manual Clock, and
//! an Action that blocks until a test lets it go.

#![allow(dead_code, unreachable_pub, clippy::unwrap_used)]

use std::future::Future;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use bia_core::action::{
    Action, ActionCall, ActionDescriptor, ActionError, BoxFuture, CancellationToken, Effect,
    InMemoryRegistry, Output,
};
use bia_core::clock::{Clock, ManualClock};
use bia_core::dsl::{ActionName, Definition};
use bia_engine::{Interpreter, Worker, WorkerConfig, start_run};
use bia_store::job::{Backoff, Job, JobRepository, PgJobRepository};
use bia_store::run::{PgRunRepository, Run, RunId, RunRepository, RunStatus};
use bia_store::timeline::{AttemptStatus, PgTimelineRepository, TimelineEntry, TimelineRepository};
use bia_store::version::{PgVersionRepository, VersionNumber, VersionRepository};
use bia_store::workflow::{PgWorkflowRepository, WorkflowRepository};
use chrono::{DateTime, TimeDelta, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinHandle;

pub const LEASE: TimeDelta = TimeDelta::seconds(30);

pub fn t0() -> DateTime<Utc> {
    DateTime::from_timestamp(1_800_000_000, 0).unwrap()
}

pub fn config() -> WorkerConfig {
    WorkerConfig {
        concurrency: NonZeroUsize::new(4).unwrap(),
        lease: LEASE,
        poll_interval: TimeDelta::seconds(5),
        shutdown_grace: TimeDelta::seconds(20),
        job_backoff: Backoff::default(),
    }
}

/// `test.gate`: blocks every call until the gate opens or the call is cancelled.
#[derive(Debug)]
pub struct Gate {
    descriptor: ActionDescriptor,
    open: watch::Sender<bool>,
    started: Semaphore,
    calls: AtomicUsize,
    saw_cancellation: AtomicBool,
}

impl Gate {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            descriptor: ActionDescriptor::new(
                ActionName::new("test.gate").unwrap(),
                "Waits for its gate to open.",
                Effect::SideEffecting,
            ),
            open: watch::Sender::new(false),
            started: Semaphore::new(0),
            calls: AtomicUsize::new(0),
            saw_cancellation: AtomicBool::new(false),
        })
    }

    /// Waits until one more call has started.
    pub async fn wait_for_a_call(&self) {
        tokio::time::timeout(Duration::from_secs(20), self.started.acquire())
            .await
            .expect("the gate was never called")
            .unwrap()
            .forget();
    }

    /// Lets running and later calls succeed.
    pub fn open(&self) {
        self.open.send_replace(true);
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    pub fn saw_cancellation(&self) -> bool {
        self.saw_cancellation.load(Ordering::SeqCst)
    }
}

impl Action for Gate {
    fn descriptor(&self) -> &ActionDescriptor {
        &self.descriptor
    }

    fn execute(
        &self,
        _call: ActionCall,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<Output, ActionError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.started.add_permits(1);
            let mut open = self.open.subscribe();
            tokio::select! {
                () = cancel.cancelled() => {
                    self.saw_cancellation.store(true, Ordering::SeqCst);
                    Err(ActionError::retryable("cancelled"))
                }
                _ = open.wait_for(|open| *open) => Ok(json!({ "opened": true })),
            }
        })
    }
}

/// One Workflow's Runs and the Workers that run them, all on one manual Clock.
pub struct Harness {
    pub pool: PgPool,
    pub clock: ManualClock,
    actions: Vec<Arc<dyn Action>>,
    published: Option<(Definition, VersionNumber)>,
}

pub struct Running {
    pub shutdown: CancellationToken,
    handle: JoinHandle<()>,
}

impl Running {
    /// Starts the shutdown sequence, if a test has not, and waits for it to end.
    pub async fn shut_down(self) {
        self.shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(20), self.handle)
            .await
            .expect("the worker did not stop")
            .unwrap();
    }
}

impl Harness {
    pub fn new(pool: PgPool, actions: Vec<Arc<dyn Action>>) -> Self {
        Self {
            pool,
            clock: ManualClock::new(t0()),
            actions,
            published: None,
        }
    }

    /// Publishes `steps` as Version 1 of `workflow`; later Runs start on it.
    pub async fn publish(&mut self, workflow: &str, steps: Value) {
        let definition = Definition::from_value(json!({
            "schema_version": 1,
            "id": workflow,
            "steps": steps,
        }))
        .unwrap();
        let mut conn = self.pool.acquire().await.unwrap();
        PgWorkflowRepository
            .create(&mut conn, &definition.id)
            .await
            .unwrap();
        let draft = PgVersionRepository
            .create_draft(&mut conn, &definition)
            .await
            .unwrap();
        PgVersionRepository
            .publish(
                &mut conn,
                &draft.workflow_id,
                draft.number,
                self.clock.now(),
            )
            .await
            .unwrap();
        self.published = Some((definition, draft.number));
    }

    pub async fn start_run(&self) -> RunId {
        let (definition, version) = self.published.as_ref().unwrap();
        let mut conn = self.pool.acquire().await.unwrap();
        start_run(
            &mut conn,
            &definition.id,
            *version,
            json!({}),
            json!({ "type": "manual" }),
            self.clock.now(),
        )
        .await
        .unwrap()
        .id
    }

    pub fn worker(&self, id: &str, config: WorkerConfig) -> Running {
        let mut registry = InMemoryRegistry::new();
        for action in &self.actions {
            registry.register(Arc::clone(action)).unwrap();
        }
        let interpreter = Interpreter::new(
            self.pool.clone(),
            Arc::new(registry),
            Arc::new(self.clock.clone()),
        );
        let worker = Worker::new(id, interpreter, config);
        let shutdown = CancellationToken::new();
        let token = shutdown.clone();
        let handle = tokio::spawn(async move { worker.run(token).await.unwrap() });
        Running { shutdown, handle }
    }

    pub async fn run(&self, id: RunId) -> Run {
        let mut conn = self.pool.acquire().await.unwrap();
        PgRunRepository.get(&mut conn, id).await.unwrap().unwrap()
    }

    pub async fn wait_for(&self, id: RunId, status: RunStatus) {
        eventually(&format!("run {id} to be {}", status.as_str()), || async {
            self.run(id).await.status == status
        })
        .await;
    }

    pub async fn timeline(&self, id: RunId) -> Vec<TimelineEntry> {
        let mut conn = self.pool.acquire().await.unwrap();
        PgTimelineRepository.list(&mut conn, id).await.unwrap()
    }

    pub async fn jobs(&self, id: RunId) -> Vec<Job> {
        let mut conn = self.pool.acquire().await.unwrap();
        PgJobRepository.list_for_run(&mut conn, id).await.unwrap()
    }
}

/// Waits for other tasks to get somewhere. Engine time never moves on its own: only tests
/// move the manual Clock.
pub async fn eventually<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let waited = tokio::time::timeout(Duration::from_secs(20), async {
        while !check().await {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "timed out waiting for {what}");
}

/// Which Step each Timeline entry is about, `None` for the Run itself, and how it ended.
pub fn ran(timeline: &[TimelineEntry]) -> Vec<(Option<&str>, AttemptStatus)> {
    timeline
        .iter()
        .map(|entry| {
            let step = entry.step.as_ref().map(|step| step.step_id.as_str());
            (step, entry.status)
        })
        .collect()
}
