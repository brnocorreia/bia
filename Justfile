# CI runs these recipes; they are the source of truth for checks.

export DATABASE_URL := env("DATABASE_URL", "postgres://bia:bia@localhost:5432/bia")
# `sqlx` macros check queries against the committed `.sqlx/` data, not a live database.
export SQLX_OFFLINE := env("SQLX_OFFLINE", "true")

# List recipes
default:
    @just --list

# Run every CI check
ci: fmt-check clippy test deny

# Format the code
fmt:
    cargo fmt --all

# Check formatting
fmt-check:
    cargo fmt --all --check

# Lint with warnings as errors
clippy:
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

# Run the test suite (needs Postgres: `just db-up`)
test:
    cargo test --workspace --all-features --locked

# Check licences, advisories, bans and sources
deny:
    cargo deny --all-features --locked check

# Regenerate `.sqlx/` after changing a query or a migration (needs Postgres and sqlx-cli)
sqlx-prepare:
    sqlx migrate run --source crates/bia-store/migrations
    SQLX_OFFLINE=false cargo sqlx prepare --workspace -- --all-targets --all-features

# Start the local Postgres and wait until it is healthy
db-up:
    docker compose up --detach --wait postgres

# Stop the local Postgres
db-down:
    docker compose down
