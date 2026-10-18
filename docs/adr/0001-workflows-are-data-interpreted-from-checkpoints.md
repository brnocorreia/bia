# Workflows are data, executed by an interpreter from checkpoints

Pipelines are Definitions (JSON validated against a published JSON Schema) rather than code, so a UI or an AI agent can create and edit them without writing Rust, and validation can return structured Diagnostics an agent can self-correct from. A Run is a state machine: every transition is written to Postgres in a transaction, and resuming means reading the Checkpoint and continuing. There is no deterministic-code replay (as in Temporal or Cloudflare Workflows).

## Consequences

- Durability comes from the Checkpoint, so Actions may be non-deterministic. Side effects are made safe by at-least-once execution plus Idempotency Keys (ADR-0010), not by replay.
- The DSL must be deliberately limited (iteration limits, max steps per Run, max subworkflow depth, side-effect-free Expressions) to avoid accidental Turing-completeness.
- `schema_version` exists from the first Definition, and DSL evolution goes through migrations.
