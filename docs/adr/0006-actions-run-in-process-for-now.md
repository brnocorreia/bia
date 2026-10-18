# Actions run in-process in the Worker, with a planned path to isolation

Builtin Actions execute inside the Worker process as async Rust (`tokio`). Timeouts and cancellation are cooperative, through `tokio::select!` with a cancellation token. This is enough while the catalogue is network calls and pure transforms.

## Evolution path

Isolation is introduced per Action, never globally, in this order:

1. **Action Servers (HTTP)**: Actions in any language run in a separate service behind a documented contract. A crash there is an Attempt failure, not a Worker crash.
2. **WASM** (`wasmtime`): in-process but sandboxed, with explicit capabilities, fuel and memory limits.
3. **Dedicated worker pools**: Workers can be started with an Action allowlist (e.g. a pool that only runs `exec.*` or `container.*` on a hardened host). Jobs carry the Action category so pools only claim what they may run.
4. **Process or container isolation** for `exec.*` and `container.*`, which are disabled by default (ADR-0014).

The `Action` trait must stay transport-agnostic (serialisable params in, serialisable Output or classified error out, cancellation token, Idempotency Key) so any Action can move along this path without changing Definitions.
