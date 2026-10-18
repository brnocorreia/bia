# At-least-once execution with Idempotency Keys

Every delivery in bia (Messages, Jobs, Step Attempts) is at-least-once. Each Step execution gets an Idempotency Key, `run_id:step_id:iteration_path`, which is stable across Attempts, and Actions forward it where the target supports it (e.g. an `Idempotency-Key` HTTP header). A Step's Checkpoint, the completion of its Job and the enqueue of the next Job commit in one transaction, so the only window for duplication is an Action that ran but whose commit failed.
