# Issue tracker: Linear

Issues for this repo live in Linear, project **bia** (https://linear.app/brnocorreia/project/bia-e17fd941b01e), accessed through the Linear MCP server.

## Conventions

- Create issues inside the **bia** project and attach each to its milestone (M0–M8, see `docs/backlog.md`).
- One issue is one PR.
- Dependencies are *blocked by* relations. Issues carry no estimates.
- Issue body sections, in order: **Context** (why, linking ADRs, `GLOSSARY.md` and docs) · **Goal** · **Scope / Non-goals** · **Acceptance criteria** (behavioural, testable) · **Notes / open questions** · **Depends on**.
- Labels: one triage label (see `triage-labels.md`), one `area:*` (`core`, `store`, `engine`, `actions`, `server`, `cli`, `ui`, `infra`, `docs`), one `type:*` (`feature`, `chore`).
- A spec (e.g. from `to-spec`) is published as a Linear **document** in the bia project, not as an issue, and the issues derived from it link back to it.

## Working an issue

- Branch: `<linear-id>-<slug>`, e.g. `BIA-12-queue-claim`.
- The PR title includes the Linear id so Linear auto-links it.
