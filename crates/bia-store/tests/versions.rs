//! Workflows, Drafts, publishing and the Active Version (ADR-0009).

// `allow-unwrap-in-tests` covers `#[test]` functions only, not the helpers they share.
#![allow(clippy::unwrap_used)]

use bia_store::versions::VersionState;
use bia_store::{StoreError, versions, workflows};
use sqlx::PgPool;

mod common;

use common::{at, definition, number, published_workflow, t0, workflow_id};

#[sqlx::test]
async fn version_numbers_increase_per_workflow(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    workflows::create(&mut conn, &workflow_id("a"))
        .await
        .unwrap();
    workflows::create(&mut conn, &workflow_id("b"))
        .await
        .unwrap();

    let mut numbers = vec![];
    for workflow in ["a", "a", "b", "a", "b"] {
        let version = versions::create_draft(&mut conn, &definition(workflow, "s"))
            .await
            .unwrap();
        numbers.push((workflow, version.number.get()));
    }

    assert_eq!(numbers, [("a", 1), ("a", 2), ("b", 1), ("a", 3), ("b", 2)]);
    let listed: Vec<i32> = versions::list(&mut conn, &workflow_id("a"))
        .await
        .unwrap()
        .iter()
        .map(|version| version.number.get())
        .collect();
    assert_eq!(listed, [1, 2, 3]);
}

#[sqlx::test]
async fn a_draft_can_be_replaced(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    let workflow = workflow_id("onboarding-email");
    workflows::create(&mut conn, &workflow).await.unwrap();
    let draft = versions::create_draft(&mut conn, &definition("onboarding-email", "before"))
        .await
        .unwrap();

    let replaced = versions::replace_draft(
        &mut conn,
        &workflow,
        draft.number,
        &definition("onboarding-email", "after"),
    )
    .await
    .unwrap();

    assert_eq!(replaced.number, draft.number);
    assert_eq!(replaced.state, VersionState::Draft);
    let stored = versions::get(&mut conn, &workflow, draft.number)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.definition, definition("onboarding-email", "after"));
}

#[sqlx::test]
async fn a_published_version_cannot_be_modified(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    let published = published_workflow(&mut conn, "onboarding-email").await;
    let workflow = &published.workflow_id;

    let replaced = versions::replace_draft(
        &mut conn,
        workflow,
        published.number,
        &definition("onboarding-email", "changed"),
    )
    .await;
    assert!(
        matches!(replaced, Err(StoreError::VersionPublished { .. })),
        "{replaced:?}"
    );

    let republished = versions::publish(&mut conn, workflow, published.number, at(60)).await;
    assert!(
        matches!(republished, Err(StoreError::VersionPublished { .. })),
        "{republished:?}"
    );

    let stored = versions::get(&mut conn, workflow, published.number)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored, published);
    assert_eq!(stored.published_at, Some(t0()));
}

#[sqlx::test]
async fn a_draft_is_saved_under_its_own_workflow(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    workflows::create(&mut conn, &workflow_id("a"))
        .await
        .unwrap();
    workflows::create(&mut conn, &workflow_id("b"))
        .await
        .unwrap();
    let draft = versions::create_draft(&mut conn, &definition("a", "s"))
        .await
        .unwrap();

    let replaced = versions::replace_draft(
        &mut conn,
        &workflow_id("a"),
        draft.number,
        &definition("b", "s"),
    )
    .await;
    assert!(
        matches!(replaced, Err(StoreError::DefinitionMismatch { .. })),
        "{replaced:?}"
    );

    let orphan = versions::create_draft(&mut conn, &definition("missing", "s")).await;
    assert!(
        matches!(orphan, Err(StoreError::WorkflowNotFound(_))),
        "{orphan:?}"
    );
}

#[sqlx::test]
async fn workflow_ids_are_unique(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    let workflow = workflow_id("onboarding-email");
    workflows::create(&mut conn, &workflow).await.unwrap();

    let again = workflows::create(&mut conn, &workflow).await;

    assert!(
        matches!(again, Err(StoreError::WorkflowExists(_))),
        "{again:?}"
    );
}

#[sqlx::test]
async fn only_a_published_version_can_become_active(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    let published = published_workflow(&mut conn, "onboarding-email").await;
    let workflow = &published.workflow_id;
    let draft = versions::create_draft(&mut conn, &definition("onboarding-email", "next"))
        .await
        .unwrap();

    let activated = workflows::set_active_version(&mut conn, workflow, draft.number).await;
    assert!(
        matches!(activated, Err(StoreError::VersionNotPublished { .. })),
        "{activated:?}"
    );
    let missing = workflows::set_active_version(&mut conn, workflow, number(9)).await;
    assert!(
        matches!(missing, Err(StoreError::VersionNotFound { .. })),
        "{missing:?}"
    );

    workflows::set_active_version(&mut conn, workflow, published.number)
        .await
        .unwrap();
    let stored = workflows::get(&mut conn, workflow).await.unwrap().unwrap();
    assert_eq!(stored.active_version, Some(published.number));
}

#[sqlx::test]
async fn rolling_back_points_the_active_version_at_an_older_one(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    let v1 = published_workflow(&mut conn, "onboarding-email").await;
    let workflow = &v1.workflow_id;
    let v2 = versions::create_draft(&mut conn, &definition("onboarding-email", "second"))
        .await
        .unwrap();
    versions::publish(&mut conn, workflow, v2.number, at(60))
        .await
        .unwrap();

    workflows::set_active_version(&mut conn, workflow, v2.number)
        .await
        .unwrap();
    workflows::set_active_version(&mut conn, workflow, v1.number)
        .await
        .unwrap();

    let stored = workflows::get(&mut conn, workflow).await.unwrap().unwrap();
    assert_eq!(stored.active_version, Some(v1.number));
}
