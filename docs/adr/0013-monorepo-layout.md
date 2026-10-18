# Monorepo with a small Cargo workspace and a static SPA

One repository holds a root Cargo workspace (`crates/`) and the UI (`ui/`). One Linear issue maps to one PR, and API changes often need UI changes, which a split repo would turn into coordinated PRs and type drift. Following Windmill, TensorZero and Quickwit, the workspace has seven crates instead of dozens: `bia-core` (DSL, schema, Expressions, validation, `Action` trait, no IO), `bia-store`, `bia-engine`, `bia-actions` (heavy categories behind Cargo features), `bia-server`, `bia-client`, `bia-cli`. `bia-actions` depends only on `bia-core`, and `bia-engine` receives the Action Registry by injection, so editing an Action rebuilds only that crate and the binary.

The UI is a static Vite + React SPA (TanStack Router/Query, Tailwind + shadcn/ui, Monaco, React Flow, TS types generated from OpenAPI). It reads its API URL from a runtime `config.json`, so one build deploys to Cloudflare Pages, S3 + CloudFront, or embedded into the binary behind the `ui` feature.

## Considered Options

- UI in a separate repository (as Meilisearch does): rejected for the PR-coordination and drift reasons above.
- Leptos/Dioxus or htmx: rejected because the graph editor is the hardest UI piece, and React Flow has no equal in those ecosystems.
