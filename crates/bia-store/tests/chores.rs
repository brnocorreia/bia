//! A Chore's run is claimed once per interval, however many schedulers race for it.

#![allow(clippy::unwrap_used)]

use bia_store::chore::{ChoreClaim, ChoreRepository, PgChoreRepository};
use chrono::TimeDelta;
use sqlx::PgPool;

mod common;

use common::at;

const HOUR: TimeDelta = TimeDelta::hours(1);

#[sqlx::test]
async fn a_chore_is_claimed_again_only_once_its_interval_has_passed(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    let claim = async |conn: &mut _, seconds| {
        PgChoreRepository
            .claim(conn, "retention", at(seconds), HOUR)
            .await
            .unwrap()
    };

    assert_eq!(claim(&mut conn, 0).await, ChoreClaim::Claimed);
    assert_eq!(claim(&mut conn, 3_599).await, ChoreClaim::RanAt(at(0)));
    assert_eq!(claim(&mut conn, 3_600).await, ChoreClaim::Claimed);
    assert_eq!(claim(&mut conn, 3_601).await, ChoreClaim::RanAt(at(3_600)));
}

#[sqlx::test]
async fn chores_are_claimed_independently(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    for chore in ["retention", "other"] {
        assert_eq!(
            PgChoreRepository
                .claim(&mut conn, chore, at(0), HOUR)
                .await
                .unwrap(),
            ChoreClaim::Claimed
        );
    }
}

#[sqlx::test]
async fn exactly_one_of_racing_claimers_wins_each_interval(pool: PgPool) {
    for seconds in [0, 3_600, 7_200] {
        let claimers = (0..8).map(|_| {
            let pool = pool.clone();
            tokio::spawn(async move {
                let mut conn = pool.acquire().await.unwrap();
                PgChoreRepository
                    .claim(&mut conn, "retention", at(seconds), HOUR)
                    .await
                    .unwrap()
            })
        });
        let mut claimed = 0;
        for claimer in claimers.collect::<Vec<_>>() {
            if claimer.await.unwrap() == ChoreClaim::Claimed {
                claimed += 1;
            }
        }
        assert_eq!(claimed, 1, "claims at {seconds}s");
    }
}
