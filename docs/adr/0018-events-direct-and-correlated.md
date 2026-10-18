# Events are delivered directly or by Correlation Key, and early Events are dropped

`flow.wait_for_event` can be resumed in two ways: `POST /runs/{run_id}/events/{name}` targets one Run (UI, Human Tasks), and `POST /events {name, key, payload}` reaches every Run waiting on that name and Correlation Key (external systems that do not know the Run). `event.emit` uses the correlated path. An Event that arrives before any Run waits for it is dropped. Buffering is a known gap, left for a later decision. `human.*` Actions are built on this mechanism, and there is no separate `flow.approval`.
