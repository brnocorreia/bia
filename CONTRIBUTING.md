# Contributing to bia

Thanks for your interest! bia is a personal open-source project that is still taking shape, so the process is intentionally small.

## Before you start

- Read [`GLOSSARY.md`](GLOSSARY.md) for the vocabulary and [`docs/architecture.md`](docs/architecture.md) for the structure.
- Check [`docs/adr/`](docs/adr/) before proposing a change in direction. To revisit a decision, open a discussion that references the ADR.
- For anything bigger than a small fix, open an issue first so the approach can be agreed.

## Development

You need a stable Rust toolchain (pinned by `rust-toolchain.toml`), Docker (for Postgres), Node.js and pnpm (for `ui/`), and [`just`](https://github.com/casey/just). Common tasks are recipes in the `Justfile`; run `just --list` to see them.

```sh
just db-up   # start Postgres from compose.yaml (docker compose up --wait)
just ci      # fmt-check, clippy, test, deny: the same recipes CI runs
just db-down # stop Postgres
```

Tests reach Postgres through `DATABASE_URL`, which defaults to `postgres://bia:bia@localhost:5432/bia` (the database in `compose.yaml`). Set it in your environment to point at another server. Database tests use `#[sqlx::test]`, which creates a throwaway database for each test, so the role needs permission to create databases.

## Pull requests

- One logical change per PR, linked to its issue.
- Conventional Commits (`feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`).
- Formatting, clippy (warnings are errors) and all tests must pass; CI enforces this.
- Add tests for behaviour changes, and update `GLOSSARY.md`, `docs/dsl.md` or an ADR when your change affects them.

## AI-assisted contributions

AI-assisted contributions are welcome and are held to the same bar. [`AGENTS.md`](AGENTS.md) is the instruction file for coding agents and doubles as a concise contributor guide.

## Conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md).
