# Failure, retry and limit semantics

- Actions return a classified error: `retryable` (network, 5xx, 408, 429, timeout) or `fatal` (invalid params, other 4xx). Only `retryable` errors consume a Step's `retry` policy.
- Retries are per Step. A failed Run is resumed from the failed Step on the same Version, or a new Run is started.
- `flow.try` with `catch` handles errors locally. The Run-level `on_error` runs compensation Steps (a simple saga), with no automatic reverse compensation.
- `flow.parallel` and `flow.foreach` fail fast by default (cancel siblings), with an opt-in `mode: all_settled`. Joins are `all` (default), `any` or `n_of_m`.
- Configurable global limits: `max_steps_per_run`, `max_iterations`, `max_subworkflow_depth`, `max_output_bytes` (default 256 KiB, after which the Step fails; blob storage comes later).

## Amendments

- 408 (Request Timeout) and 429 (Too Many Requests) are `retryable`, not `fatal`, amended with `http.request` (ADR-0032). Both are the server asking to be tried again later, and treating them as `fatal` would fail a Run on a transient rate limit that the Step's `retry` backoff would have absorbed.
