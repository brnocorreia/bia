//! Runs: pinned to their Version, with Checkpoints and listing.

// `allow-unwrap-in-tests` covers `#[test]` functions only, not the helpers they share.
#![allow(clippy::unwrap_used)]

use bia_store::StoreError;
use bia_store::models::{Checkpoint, NewRun, Run, RunFilter, RunId, RunStatus};
use bia_store::repositories::{
    PgRunRepository, PgVersionRepository, PgWorkflowRepository, RunRepository, VersionRepository,
    WorkflowRepository,
};
use serde_json::json;
use sqlx::PgPool;

mod common;

use common::{at, definition, published_workflow, start_run};

#[sqlx::test]
async fn a_run_keeps_its_version_after_the_active_version_changes(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    let v1 = published_workflow(&mut conn, "onboarding-email").await;
    let workflow = &v1.workflow_id;
    PgWorkflowRepository
        .set_active_version(&mut conn, workflow, v1.number)
        .await
        .unwrap();
    let run = start_run(&mut conn, &v1, at(1)).await;

    let v2 = PgVersionRepository
        .create_draft(&mut conn, &definition("onboarding-email", "second"))
        .await
        .unwrap();
    PgVersionRepository
        .publish(&mut conn, workflow, v2.number, at(2))
        .await
        .unwrap();
    PgWorkflowRepository
        .set_active_version(&mut conn, workflow, v2.number)
        .await
        .unwrap();

    let stored = PgRunRepository
        .get(&mut conn, run.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.version, v1.number);
    let pinned = PgVersionRepository
        .get(&mut conn, workflow, stored.version)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pinned.definition, definition("onboarding-email", "first"));
}

#[sqlx::test]
async fn a_draft_cannot_start_a_run(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    let published = published_workflow(&mut conn, "onboarding-email").await;
    let draft = PgVersionRepository
        .create_draft(&mut conn, &definition("onboarding-email", "next"))
        .await
        .unwrap();

    let run = NewRun {
        workflow_id: published.workflow_id.clone(),
        version: draft.number,
        input: json!({}),
        trigger: json!({}),
        checkpoint: Checkpoint::default(),
    };
    let created = PgRunRepository.create(&mut conn, &run, at(1)).await;

    assert!(
        matches!(created, Err(StoreError::VersionNotPublished { .. })),
        "{created:?}"
    );
}

#[sqlx::test]
async fn a_new_run_is_pending_with_its_input_and_checkpoint(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    let version = published_workflow(&mut conn, "onboarding-email").await;

    let run = start_run(&mut conn, &version, at(5)).await;

    assert_eq!(run.status, RunStatus::Pending);
    assert_eq!(run.input, json!({ "user": 42 }));
    assert_eq!(run.trigger, json!({ "type": "manual" }));
    assert_eq!(run.checkpoint.cursor, Vec::<serde_json::Value>::new());
    assert_eq!((run.created_at, run.updated_at), (at(5), at(5)));
    assert_eq!(run.finished_at, None);
    assert_eq!(
        PgRunRepository.get(&mut conn, run.id).await.unwrap(),
        Some(run)
    );
}

#[sqlx::test]
async fn checkpoint_updates_are_read_back(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    let version = published_workflow(&mut conn, "onboarding-email").await;
    let run = start_run(&mut conn, &version, at(0)).await;

    let checkpoint = Checkpoint {
        cursor: vec![json!("steps"), json!(2), json!("then"), json!(0)],
        state: json!({ "outputs": { "fetch": { "ok": true } } }),
    };
    PgRunRepository
        .update_checkpoint(&mut conn, run.id, RunStatus::Sleeping, &checkpoint, at(10))
        .await
        .unwrap();
    let sleeping = PgRunRepository
        .get(&mut conn, run.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sleeping.status, RunStatus::Sleeping);
    assert_eq!(sleeping.checkpoint, checkpoint);
    assert_eq!(sleeping.updated_at, at(10));
    assert_eq!(sleeping.finished_at, None);

    PgRunRepository
        .update_checkpoint(&mut conn, run.id, RunStatus::Succeeded, &checkpoint, at(20))
        .await
        .unwrap();
    let finished = PgRunRepository
        .get(&mut conn, run.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(finished.finished_at, Some(at(20)));

    let missing = RunId::from_uuid(uuid::Uuid::nil());
    let updated = PgRunRepository
        .update_checkpoint(&mut conn, missing, RunStatus::Running, &checkpoint, at(30))
        .await;
    assert!(
        matches!(updated, Err(StoreError::RunNotFound(_))),
        "{updated:?}"
    );
}

#[sqlx::test]
async fn runs_are_listed_newest_first_with_filters_and_paging(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    let a = published_workflow(&mut conn, "a").await;
    let b = published_workflow(&mut conn, "b").await;
    let a1 = start_run(&mut conn, &a, at(1)).await;
    let b1 = start_run(&mut conn, &b, at(2)).await;
    let a2 = start_run(&mut conn, &a, at(3)).await;
    let a3 = start_run(&mut conn, &a, at(4)).await;
    PgRunRepository
        .update_checkpoint(
            &mut conn,
            a2.id,
            RunStatus::Failed,
            &Checkpoint::default(),
            at(5),
        )
        .await
        .unwrap();

    let ids = |runs: Vec<Run>| runs.into_iter().map(|run| run.id).collect::<Vec<_>>();

    let all = PgRunRepository
        .list(&mut conn, &RunFilter::default())
        .await
        .unwrap();
    assert_eq!(ids(all), [a3.id, a2.id, b1.id, a1.id]);

    let of_a = RunFilter {
        workflow_id: Some(a.workflow_id.clone()),
        ..RunFilter::default()
    };
    assert_eq!(
        ids(PgRunRepository.list(&mut conn, &of_a).await.unwrap()),
        [a3.id, a2.id, a1.id]
    );

    let failed = RunFilter {
        status: Some(RunStatus::Failed),
        ..RunFilter::default()
    };
    assert_eq!(
        ids(PgRunRepository.list(&mut conn, &failed).await.unwrap()),
        [a2.id]
    );

    let first_page = RunFilter {
        limit: 2,
        ..RunFilter::default()
    };
    let page = ids(PgRunRepository.list(&mut conn, &first_page).await.unwrap());
    assert_eq!(page, [a3.id, a2.id]);
    let second_page = RunFilter {
        after: page.last().copied(),
        ..first_page
    };
    assert_eq!(
        ids(PgRunRepository.list(&mut conn, &second_page).await.unwrap()),
        [b1.id, a1.id]
    );
}
