import { createFileRoute, Link, useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { messageOf } from "@/api/request";
import { RUN_STATUSES, useRuns, type RunStatus } from "@/api/runs";
import { useWorkflows } from "@/api/workflows";
import { StatusBadge } from "@/components/status-badge";
import { TriggerRunDialog } from "@/components/trigger-run-dialog";
import { Button } from "@/components/ui/button";
import { formatTime } from "@/lib/format";

interface Search {
  workflow?: string;
  status?: RunStatus;
  /** First day to include, `YYYY-MM-DD`, in the browser's time zone. */
  from?: string;
  /** Last day to include, `YYYY-MM-DD`. */
  to?: string;
}

const day = (value: unknown) =>
  typeof value === "string" && /^\d{4}-\d{2}-\d{2}$/.test(value) ? value : undefined;

export const Route = createFileRoute("/_app/runs")({
  staticData: { nav: { label: "Runs", order: 20 } },
  validateSearch: (search: Record<string, unknown>): Search => ({
    workflow: typeof search.workflow === "string" && search.workflow ? search.workflow : undefined,
    status: RUN_STATUSES.find((s) => s === search.status),
    from: day(search.from),
    to: day(search.to),
  }),
  component: Runs,
});

/** Local midnight of a `YYYY-MM-DD` day, as an ISO instant; `plusDays` makes an exclusive end. */
function startOfDay(value: string | undefined, plusDays = 0): string | undefined {
  if (!value) return undefined;
  const [y, m, d] = value.split("-").map(Number);
  return new Date(y!, m! - 1, d! + plusDays).toISOString();
}

const FIELD = "h-9 rounded-md border bg-background px-2 text-sm";

function Runs() {
  const search = Route.useSearch();
  const navigate = useNavigate({ from: Route.fullPath });
  const workflows = useWorkflows();
  const [triggering, setTriggering] = useState(false);
  const runs = useRuns({
    workflow: search.workflow,
    status: search.status,
    from: startOfDay(search.from),
    to: startOfDay(search.to, 1),
  });
  const rows = runs.data?.pages.flatMap((page) => page.runs) ?? [];
  const set = (patch: Partial<Search>) =>
    void navigate({ search: (previous) => ({ ...previous, ...patch }) });

  return (
    <section>
      <div className="flex items-center justify-between">
        <h1 className="text-2xl font-semibold">Runs</h1>
        <Button onClick={() => setTriggering(true)}>Trigger a Run</Button>
      </div>

      <form
        aria-label="Filters"
        className="mt-4 flex flex-wrap items-end gap-3 text-sm"
        onSubmit={(event) => event.preventDefault()}
      >
        <label className="flex flex-col gap-1">
          Workflow
          <select
            className={FIELD}
            value={search.workflow ?? ""}
            onChange={(event) => set({ workflow: event.target.value || undefined })}
          >
            <option value="">All</option>
            {workflows.data?.map((w) => (
              <option key={w.id} value={w.id}>
                {w.id}
              </option>
            ))}
          </select>
        </label>
        <label className="flex flex-col gap-1">
          Status
          <select
            className={FIELD}
            value={search.status ?? ""}
            onChange={(event) => set({ status: (event.target.value as RunStatus) || undefined })}
          >
            <option value="">All</option>
            {RUN_STATUSES.map((s) => (
              <option key={s} value={s}>
                {s}
              </option>
            ))}
          </select>
        </label>
        <label className="flex flex-col gap-1">
          From
          <input
            type="date"
            className={FIELD}
            value={search.from ?? ""}
            onChange={(event) => set({ from: day(event.target.value) })}
          />
        </label>
        <label className="flex flex-col gap-1">
          To
          <input
            type="date"
            className={FIELD}
            value={search.to ?? ""}
            onChange={(event) => set({ to: day(event.target.value) })}
          />
        </label>
        <Button
          type="button"
          variant="outline"
          onClick={() =>
            void navigate({
              search: { workflow: undefined, status: undefined, from: undefined, to: undefined },
            })
          }
        >
          Clear filters
        </Button>
      </form>

      {runs.isPending && <p className="mt-4 text-sm text-muted-foreground">Loading…</p>}
      {runs.error && (
        <p role="alert" className="mt-4 text-sm text-destructive">
          {messageOf(runs.error)}
        </p>
      )}
      {runs.data && rows.length === 0 && (
        <p className="mt-4 text-sm text-muted-foreground">No Runs match.</p>
      )}
      {rows.length > 0 && (
        <table className="mt-4 w-full text-left text-sm">
          <thead className="border-b text-muted-foreground">
            <tr>
              <th className="py-2 pr-4 font-medium">Run</th>
              <th className="py-2 pr-4 font-medium">Workflow</th>
              <th className="py-2 pr-4 font-medium">Version</th>
              <th className="py-2 pr-4 font-medium">Status</th>
              <th className="py-2 pr-4 font-medium">Trigger</th>
              <th className="py-2 font-medium">Started</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((run) => (
              <tr key={run.id} className="border-b">
                <td className="py-2 pr-4">
                  <Link
                    to="/runs/$runId"
                    params={{ runId: run.id }}
                    className="font-mono text-xs underline-offset-4 hover:underline"
                  >
                    {run.id.slice(0, 8)}
                  </Link>
                </td>
                <td className="py-2 pr-4">{run.workflow_id}</td>
                <td className="py-2 pr-4">v{run.version}</td>
                <td className="py-2 pr-4">
                  <StatusBadge status={run.status} />
                </td>
                <td className="py-2 pr-4 text-muted-foreground">
                  {(run.trigger as { type?: string }).type ?? "—"}
                  {run.retry_of ? " (retry)" : ""}
                </td>
                <td className="py-2">{formatTime(run.created_at)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {runs.hasNextPage && (
        <Button
          variant="outline"
          className="mt-4"
          disabled={runs.isFetchingNextPage}
          onClick={() => void runs.fetchNextPage()}
        >
          Load more
        </Button>
      )}

      <TriggerRunDialog
        open={triggering}
        initialWorkflow={search.workflow}
        onClose={() => setTriggering(false)}
      />
    </section>
  );
}
