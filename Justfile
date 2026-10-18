# CI runs these recipes; they are the source of truth for checks.

export DATABASE_URL := env("DATABASE_URL", "postgres://bia:bia@localhost:5432/bia")

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

# Start the local Postgres and wait until it is healthy
db-up:
    docker compose up --detach --wait postgres

# Stop the local Postgres
db-down:
    docker compose down
