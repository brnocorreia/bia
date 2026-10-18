//! Proves the test harness reaches Postgres through `DATABASE_URL`.

use sqlx::PgPool;

#[sqlx::test]
async fn connects_to_postgres(pool: PgPool) {
    let one: i32 = sqlx::query_scalar("SELECT 1")
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(one, 1);
}
