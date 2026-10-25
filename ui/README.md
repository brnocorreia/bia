# bia UI

A static React SPA (Vite, TypeScript, TanStack Router and Query, Tailwind, shadcn/ui) that talks to the bia HTTP API. See ADR-0013, ADR-0050 and ADR-0051; `docs/deploy-ui.md` covers embedding and static deployment.

Needs Node.js 22+ and pnpm (the version is pinned by `packageManager`; `corepack enable` installs it).

## Develop

```sh
cd ui
pnpm install
pnpm dev          # http://localhost:5173
```

The API URL comes from `config.json` at the site root, read at startup, so one build works everywhere. In development it is `public/config.json` (default `http://127.0.0.1:8080`); on a deployment, put your own `config.json` next to `index.html`. The site must be served from the root of its origin. An empty `apiUrl` means "the page's own origin". The API must allow the UI's origin in `BIA_CORS_ORIGINS`.

Run an API for development with `just db-up`, `bia migrate`, then `bia serve` (set `BIA_BOOTSTRAP_KEY` and `BIA_CORS_ORIGINS=http://localhost:5173`), and mint a Token with `bia token create --bootstrap`.

## Layout

- `src/routes/`: one file per screen. A screen is `_app.<name>.tsx`; setting `staticData: { nav: { label, order } }` adds it to the navigation, so a new screen edits no shared file. `login.tsx` is the only route outside the signed-in shell.
- `src/api/client.ts`: the one API client, typed by `schema.gen.ts`. It sends the session Token and signs the user out on 401. Use `getApi()`, with TanStack Query.
- `src/auth/`: the Token session (`sessionStorage`) and the login check.
- `src/auth/scope.ts`: `useCurrentToken()` and `useIsAdmin()` read the signed-in Token's Scope from `GET /v1/tokens/me` (ADR-0053). `components/admin-only.tsx` has `AdminOnly` (render or fall back) and `RequireAdmin` (a whole admin-only screen). Disable, rather than hide, controls a Scope cannot use; the server still enforces it.
- `src/api/request.ts`: `unwrap(result)` turns an `openapi-fetch` result into data or an `ApiError` (with the error body and its Diagnostics), and `messageOf(error)` words it for users. `src/api/workflows.ts` holds the Workflow and Version queries and mutations.
- Shared components in `src/components/`: `ConfirmDialog` (modal confirmation with an inline error), `DiagnosticsList`, `DefinitionViewer` (read-only YAML/JSON toggle), `VersionDiff`.
- `src/components/ui/`: shadcn/ui components, added by hand to `components.json`'s conventions.
- `src/routeTree.gen.ts` is generated and git-ignored; every script regenerates it.

## Checks

```sh
pnpm typecheck
pnpm lint          # eslint + prettier --check; `pnpm format` fixes formatting
pnpm test          # Vitest
pnpm build         # static bundle in dist/
pnpm check:api     # regenerates the API types and fails if they differ from the committed file
```

`just ui-check` runs all of these (the end-to-end tests excepted).

## End-to-end tests

Playwright drives a real `bia serve`. The global setup (`e2e/support/global-setup.ts`) drops and recreates a database, migrates it, starts `bia serve`, bootstraps an admin Token, and serves `dist/` with a `config.json` pointing at that server. `pnpm test:e2e` builds first. One-time: `pnpm exec playwright install chromium`.

| Variable               | Default                                     | Meaning                                                                                               |
| ---------------------- | ------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| `BIA_E2E_PORT`         | `18080`                                     | Port of the `bia serve` the tests start.                                                              |
| `BIA_E2E_UI_PORT`      | `BIA_E2E_PORT + 1000`                       | Port of the static server for the bundle.                                                             |
| `BIA_E2E_DATABASE_URL` | `postgres://bia:bia@localhost:5432/bia_e2e` | Database to **drop and recreate**; its name must start with `bia_e2e`. Needs Postgres (`just db-up`). |
| `BIA_E2E_BIN`          | builds `bia-cli`, uses `target/debug/bia`   | A prebuilt `bia` binary.                                                                              |

To test the UI embedded in the binary (`--features ui`, ADR-0052), run `pnpm test:e2e:embedded` (or `BIA_E2E_EMBEDDED=1`): the setup builds `bia-cli` with the feature, skips the static server, and `embedded.spec.ts` runs. `BIA_E2E_BIN` must then be a binary built with the feature.

Use your own port and database when several checkouts run at once, for example `BIA_E2E_PORT=18028 BIA_E2E_DATABASE_URL=postgres://bia:bia@localhost:5432/bia_e2e_28 pnpm test:e2e`.

Specs import `test` and `expect` from `e2e/support/fixtures.ts`:

- `signedInPage`: a page already signed in as the admin Token.
- `api`: a typed client authenticated as the admin Token, for seeding data through the API.
- `adminToken`, `signIn(page, token)` (through the form), `startSessionWith(page, token)`, `uniqueName(prefix)`.

Tests share one database, so give seeded data a `uniqueName` and do not assume the list screens are empty.

## Regenerating the API types

When a handler changes, run `just openapi` at the repository root, then `pnpm gen:api` here, and commit both `crates/bia-server/openapi.json` and `src/api/schema.gen.ts`. CI fails if the types are stale.

## Runs and the Timeline

- `/runs` lists Runs (newest first, "Load more" pages by `next_after`) filtered by Workflow, status and a creation date range; filters are URL search params. "Trigger a Run" opens a dialog with the Workflow (those with an Active Version) and a JSON input. `/runs/$runId` shows the Run, its input and Trigger metadata, the Timeline, and Cancel, Resume and Retry behind confirmations (Retry offers a link to the new Run). See ADR-0054.
- Active Runs poll (`api/runs.ts`, `api/polling.ts`): every 2 s by default, stopping when the Run ends. In a browser, `localStorage["bia.pollIntervalMs"]` (at least 50) overrides it; the e2e tests set 100.
- `components/timeline/timeline.tsx` exports `Timeline`, reused by the Dry Run screen. It fetches nothing.

  | Prop      | Type                  | Meaning                                                                                                   |
  | --------- | --------------------- | --------------------------------------------------------------------------------------------------------- |
  | `entries` | `TimelineEntryInfo[]` | The API's Timeline: `RunInfo.timeline` or `DryRunReport.timeline`. Order does not matter (`seq` is used). |
  | `dryRun`  | `boolean`, optional   | Shows a "dry run" badge and a note that the durations are virtual.                                        |

  `components/timeline/model.ts` holds the pure grouping and formatting (Vitest), and `json-block.tsx` the lazy, collapsed JSON disclosure. `components/status-badge.tsx` colours a Run or Attempt status.

## Definition editor

`src/editor/` holds the Monaco-based Definition editor (ADR-0055); `routes/_app.workflows_.new.tsx` and `routes/_app.workflows_.$workflowId_.edit.tsx` mount it.

- Monaco, `monaco-yaml` and their workers are bundled (`editor/environment.ts`, `editor/yaml.worker.ts`), with no CDN loader, so the editor works under the embedded UI's CSP unchanged. `monaco-editor` is pinned to 0.54: `monaco-yaml` 5.5 imports paths that 0.57's export map no longer has.
- Completion and hover come from `GET /v1/schema` (`definition`). Client-side validation is off: the only underlines are the server's Diagnostics from `POST /v1/validate`, run 500 ms after typing stops and mapped to ranges by `editor/path-range.ts` (JSON and YAML).
- The text is JSON or YAML; switching converts through the parsed value (`editor/convert.ts`). The server stores JSON only, so YAML comments are lost, and the editor says so. The editor keeps no YAML source anywhere.
- Saving: `Save as new Draft` (`POST .../versions`, which also creates a Workflow) and, when the loaded Version is a Draft, `Replace Draft vN` (`PUT .../versions/{n}`). A Draft with errors saves; its Diagnostics are shown. Publishing stays on the Version screens.
- Leaving with unsaved changes asks first (router blocker and `beforeunload`).

### Adding a panel beside the editor (Dry Run, BIA-33)

`DefinitionEditor` (`editor/definition-editor.tsx`) takes an optional `sidePanel` render prop and renders it in an `<aside>` next to the editor. It is called with an `EditorContext`: `workflowId`, `definition` (the current text, parsed; `undefined` while it does not parse), `dirty` and the latest server `diagnostics`. A panel reads these and adds no logic to the editor: pass `sidePanel={(context) => <DryRunPanel {...context} />}` from the edit route.

### Editor end-to-end tests

Monaco turns typed newlines into auto-indented lines, so `e2e/editor.spec.ts` pastes text (a synthetic `paste` event) to set the editor's content.

## Dry Run from the editor

`src/dry-run/dry-run-panel.tsx` is passed to `DefinitionEditor` as `sidePanel` by the new and edit routes (ADR-0056). It dry-runs the editor's current text through `POST /v1/dry-run` (`api/dry-run.ts`) with an input and Mocks, and renders `report.timeline` with `<Timeline dryRun />`. Mocks are JSON in the API's format, one per Step id, so they can be copied to a CLI file ("Copy Mocks as JSON"). A Definition that does not validate shows the Diagnostics instead of a Timeline. The last input and Mocks are remembered per Workflow in `localStorage["bia.dryRun.<id>"]`. Pure logic (Step discovery, request building, storage) is in `dry-run/model.ts` with Vitest; `e2e/dry-run.spec.ts` covers the flows.
