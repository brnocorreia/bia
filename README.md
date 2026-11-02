# bia

**Background Integrations & Automations.** _Bia Isn't Airflow._

bia is a self-hosted engine for **cron schedules, durable queues and long-running workflows**, in the spirit of Cloudflare's Cron Triggers, Queues and Workflows, except that every pipeline is **data**. You write a JSON or YAML document validated against a published schema, and bia interprets it, checkpointing every step in Postgres.

Because workflows are data, a UI, a CLI, a GitOps repo or an **AI agent** can create, validate, dry-run and operate them through the same API, without writing or compiling code.

> Status: pre-alpha, under active design. Nothing here is usable yet.

## Getting started

```sh
docker compose up --detach --wait postgres
export DATABASE_URL=postgres://bia:bia@localhost:5432/bia BIA_MASTER_KEY="$(openssl rand -hex 32)"
bia migrate && bia serve          # logs a Setup Link: open it and choose a password
bia login                         # approve the CLI in your browser
claude mcp add --transport http bia http://127.0.0.1:8080/mcp   # approve the agent on the consent screen
```

No Token is copied anywhere: the browser signs you in, MCP clients and the CLI sign in through it, and an agent that wants to publish asks, so you approve in the UI. [`docs/quickstart.md`](docs/quickstart.md) walks the whole scenario.

## Highlights (planned)

- Durable steps with per-step retry and timeout, `sleep` for days, waits for events and human approvals.
- Triggers: cron (with overlap and catch-up policies), queues, webhooks, events, manual runs.
- Postgres only: `SKIP LOCKED` queues, leases, `LISTEN/NOTIFY`; safe to run several replicas.
- Validation with structured diagnostics that agents can fix themselves; dry runs with mocks.
- Agent-first interfaces: HTTP API, MCP server, CLI. Agents author drafts, and humans publish.
- A static web UI to watch runs, edit definitions and, later, build them visually.

## Example

```yaml
schema_version: 1
id: uptime-check
triggers:
  - { id: every_5m, type: cron, expr: "*/5 * * * *" }
steps:
  - id: probe
    action: http.request
    params: { method: GET, url: https://example.com/health }
    retry: { max_attempts: 3, backoff: exponential, initial: 2s }
    timeout: 10s
  - id: alert_if_down
    action: flow.branch
    when: steps.probe.output.status >= 500
    then:
      - id: notify
        action: http.request
        params:
          method: POST
          url: "{{ secrets.ALERT_WEBHOOK }}"
          body: { text: "example.com returned {{ steps.probe.output.status }}" }
```

## Documentation

- [`docs/quickstart.md`](docs/quickstart.md): run bia locally, sign in and walk the agent scenario with an MCP client
- [`GLOSSARY.md`](GLOSSARY.md): domain vocabulary
- [`docs/architecture.md`](docs/architecture.md): how the pieces fit
- [`docs/dsl.md`](docs/dsl.md): the definition format (draft)
- [`docs/deploy-ui.md`](docs/deploy-ui.md): embedding the UI in the binary, or deploying it to Cloudflare Pages or S3 + CloudFront
- [`docs/configuration.md`](docs/configuration.md): configuring `bia serve` and `bia migrate`
- [`docs/adr/`](docs/adr/): architecture decision records

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md) and the [Code of Conduct](CODE_OF_CONDUCT.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this project by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
