# The LLM agent loop runs inside one Step

`llm.tool_loop` runs its whole agent loop inside a single Step, bounded by `max_iterations`, with tools drawn from an allowlist of Actions in the Action Registry. Each iteration is recorded as a sub-entry in the Timeline, but the Checkpoint covers the Step as a whole: a crash restarts the loop. Users who need per-iteration durability model the loop explicitly with `flow.loop_until` and `llm.chat`. Providers sit behind a trait (Anthropic, OpenAI-compatible, Ollama), defaulting to current Claude models.
