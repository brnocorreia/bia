import { createFileRoute, Link, useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { ApiError, messageOf } from "@/api/request";
import {
  useVersion,
  useVersionAction,
  useVersions,
  useWorkflow,
  type VersionSummary,
} from "@/api/workflows";
import { useCurrentToken } from "@/auth/scope";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { DefinitionViewer } from "@/components/definition-viewer";
import { DiagnosticsList } from "@/components/diagnostics-list";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { VersionDiff } from "@/components/version-diff";
import { formatTime } from "@/lib/format";

interface Search {
  /** The Version whose Definition is shown. */
  v?: number;
  /** The two Versions being compared. */
  from?: number;
  to?: number;
}

const number = (value: unknown) => {
  const parsed = Number(value);
  return Number.isInteger(parsed) && parsed > 0 ? parsed : undefined;
};

export const Route = createFileRoute("/_app/workflows_/$workflowId")({
  validateSearch: (search: Record<string, unknown>): Search => ({
    v: number(search.v),
    from: number(search.from),
    to: number(search.to),
  }),
  component: WorkflowDetail,
});

type Pending = { kind: "publish" | "activate"; version: VersionSummary; rollback: boolean };

function WorkflowDetail() {
  const { workflowId } = Route.useParams();
  const search = Route.useSearch();
  const navigate = useNavigate({ from: Route.fullPath });
  const workflow = useWorkflow(workflowId);
  const versions = useVersions(workflowId);
  const token = useCurrentToken();
  const isAdmin = token.data?.scope === "admin";
  const [pending, setPending] = useState<Pending | null>(null);

  const list = versions.data ?? [];
  const activeNumber = workflow.data?.active_version ?? undefined;
  const viewed = search.v ?? activeNumber ?? list.at(-1)?.number;
  const definition = useVersion(workflowId, viewed);

  const publish = useVersionAction(workflowId, "publish");
  const activate = useVersionAction(workflowId, "activate");
  const action = pending?.kind === "publish" ? publish : activate;

  const setSearch = (next: Partial<Search>) =>
    void navigate({ search: (previous: Search) => ({ ...previous, ...next }) });

  function close() {
    publish.reset();
    activate.reset();
    setPending(null);
  }

  function confirm() {
    if (!pending) return;
    action.mutate(pending.version.number, { onSuccess: close });
  }

  if (workflow.error) {
    return (
      <p role="alert" className="text-sm text-destructive">
        {messageOf(workflow.error)}
      </p>
    );
  }

  const adminHint = isAdmin ? undefined : "Needs an admin Token";

  return (
    <section className="flex flex-col gap-6">
      <div>
        <Link to="/workflows" className="text-sm text-muted-foreground hover:underline">
          ← Workflows
        </Link>
        <h1 className="mt-1 text-2xl font-semibold">{workflowId}</h1>
        <p className="text-sm text-muted-foreground">
          {activeNumber ? `Active Version: v${activeNumber}` : "No Active Version yet."}
        </p>
      </div>

      <div>
        <h2 className="mb-2 text-lg font-semibold">Versions</h2>
        {versions.error && (
          <p role="alert" className="text-sm text-destructive">
            {messageOf(versions.error)}
          </p>
        )}
        <table className="w-full text-left text-sm">
          <thead className="border-b text-muted-foreground">
            <tr>
              <th className="py-2 pr-4 font-medium">Version</th>
              <th className="py-2 pr-4 font-medium">Status</th>
              <th className="py-2 pr-4 font-medium">Created</th>
              <th className="py-2 pr-4 font-medium">Published</th>
              <th className="py-2 font-medium">Actions</th>
            </tr>
          </thead>
          <tbody>
            {[...list].reverse().map((version) => {
              const rollback = activeNumber !== undefined && version.number < activeNumber;
              return (
                <tr
                  key={version.number}
                  aria-label={`Version ${version.number}`}
                  className="border-b"
                >
                  <td className="py-2 pr-4 font-medium">v{version.number}</td>
                  <td className="py-2 pr-4">
                    <span className="flex gap-1">
                      <Badge>{version.state === "draft" ? "Draft" : "Published"}</Badge>
                      {version.active && <Badge variant="default">Active</Badge>}
                    </span>
                  </td>
                  <td className="py-2 pr-4">{formatTime(version.created_at)}</td>
                  <td className="py-2 pr-4">{formatTime(version.published_at)}</td>
                  <td className="py-2">
                    <span className="flex flex-wrap gap-1">
                      <Button
                        size="sm"
                        variant="ghost"
                        aria-pressed={viewed === version.number}
                        onClick={() => setSearch({ v: version.number })}
                      >
                        View
                      </Button>
                      {version.state === "draft" && (
                        <Button
                          size="sm"
                          variant="outline"
                          disabled={!isAdmin}
                          title={adminHint}
                          onClick={() => setPending({ kind: "publish", version, rollback })}
                        >
                          Publish
                        </Button>
                      )}
                      {version.state === "published" && !version.active && (
                        <Button
                          size="sm"
                          variant="outline"
                          disabled={!isAdmin}
                          title={adminHint}
                          onClick={() => setPending({ kind: "activate", version, rollback })}
                        >
                          {rollback ? "Roll back to this" : "Activate"}
                        </Button>
                      )}
                    </span>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
        {!isAdmin && token.data && (
          <p className="mt-2 text-xs text-muted-foreground">
            Publishing and activating need an admin Token; this one is {token.data.scope}.
          </p>
        )}
      </div>

      <div>
        <h2 className="mb-2 text-lg font-semibold">
          Definition{viewed !== undefined && ` of v${viewed}`}
        </h2>
        {definition.error && (
          <p role="alert" className="text-sm text-destructive">
            {messageOf(definition.error)}
          </p>
        )}
        <DefinitionViewer definition={definition.data?.definition} />
      </div>

      {list.length >= 2 && (
        <div>
          <h2 className="mb-2 text-lg font-semibold">Compare Versions</h2>
          <div className="mb-3 flex items-center gap-2 text-sm">
            <VersionSelect
              label="From"
              value={search.from}
              versions={list}
              onChange={(from) => setSearch({ from })}
            />
            <VersionSelect
              label="To"
              value={search.to}
              versions={list}
              onChange={(to) => setSearch({ to })}
            />
          </div>
          {search.from !== undefined && search.to !== undefined ? (
            <VersionDiff workflowId={workflowId} from={search.from} to={search.to} />
          ) : (
            <p className="text-sm text-muted-foreground">Pick two Versions to compare.</p>
          )}
        </div>
      )}

      <ConfirmDialog
        open={pending !== null}
        title={dialogTitle(pending)}
        confirmLabel={
          pending?.kind === "publish" ? "Publish" : pending?.rollback ? "Roll back" : "Activate"
        }
        busy={action.isPending}
        onConfirm={confirm}
        onCancel={close}
        error={action.error && <ActionError error={action.error} />}
      >
        {pending?.kind === "publish"
          ? "A Published Version is frozen for good. It is validated first, and any error stops the publish."
          : pending?.rollback
            ? `New Runs will start on v${pending.version.number}. Runs already started keep their Version.`
            : `New Runs will start on v${pending?.version.number} and its Triggers take effect.`}
      </ConfirmDialog>
    </section>
  );
}

function dialogTitle(pending: Pending | null) {
  if (!pending) return "";
  const n = pending.version.number;
  if (pending.kind === "publish") return `Publish v${n}?`;
  return pending.rollback ? `Roll back to v${n}?` : `Activate v${n}?`;
}

function ActionError({ error }: { error: unknown }) {
  const diagnostics = error instanceof ApiError ? (error.body?.diagnostics ?? []) : [];
  return (
    <div className="flex flex-col gap-2">
      <p>{messageOf(error)}</p>
      <DiagnosticsList diagnostics={diagnostics} />
    </div>
  );
}

function VersionSelect({
  label,
  value,
  versions,
  onChange,
}: {
  label: string;
  value: number | undefined;
  versions: VersionSummary[];
  onChange: (number: number) => void;
}) {
  return (
    <label className="flex items-center gap-2">
      {label}
      <select
        className="h-9 rounded-md border border-input bg-background px-2"
        value={value ?? ""}
        onChange={(event) => onChange(Number(event.target.value))}
      >
        <option value="" disabled>
          Select…
        </option>
        {versions.map((v) => (
          <option key={v.number} value={v.number}>
            v{v.number}
          </option>
        ))}
      </select>
    </label>
  );
}
