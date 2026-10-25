import { createFileRoute, Link } from "@tanstack/react-router";
import { Badge } from "@/components/ui/badge";
import {
  useVersion,
  useVersions,
  useWorkflows,
  type VersionSummary,
  type WorkflowInfo,
} from "@/api/workflows";
import { messageOf } from "@/api/request";
import { formatTime } from "@/lib/format";

export const Route = createFileRoute("/_app/workflows")({
  staticData: { nav: { label: "Workflows", order: 10 } },
  component: Workflows,
});

function Workflows() {
  const workflows = useWorkflows();
  return (
    <section>
      <div className="flex items-center justify-between">
        <h1 className="text-2xl font-semibold">Workflows</h1>
        <Link
          to="/workflows/new"
          className="rounded-md bg-primary px-3 py-1.5 text-sm text-primary-foreground hover:bg-primary/90"
        >
          New Workflow
        </Link>
      </div>
      {workflows.isPending && <p className="mt-4 text-sm text-muted-foreground">Loading…</p>}
      {workflows.error && (
        <p role="alert" className="mt-4 text-sm text-destructive">
          {messageOf(workflows.error)}
        </p>
      )}
      {workflows.data?.length === 0 && (
        <p className="mt-4 text-sm text-muted-foreground">
          No Workflows yet. Create one here, or save a Draft through the API, the CLI or an agent.
        </p>
      )}
      {workflows.data && workflows.data.length > 0 && (
        <table className="mt-4 w-full text-left text-sm">
          <thead className="border-b text-muted-foreground">
            <tr>
              <th className="py-2 pr-4 font-medium">Id</th>
              <th className="py-2 pr-4 font-medium">Description</th>
              <th className="py-2 pr-4 font-medium">Active Version</th>
              <th className="py-2 font-medium">Last change</th>
            </tr>
          </thead>
          <tbody>
            {workflows.data.map((workflow) => (
              <WorkflowRow key={workflow.id} workflow={workflow} />
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}

// One row reads its own Versions (for the last change) and its Active Version's Definition
// (for the description); the list endpoint carries neither.
function WorkflowRow({ workflow }: { workflow: WorkflowInfo }) {
  const versions = useVersions(workflow.id);
  const active = useVersion(workflow.id, workflow.active_version ?? undefined);
  const description = (active.data?.definition as { description?: string } | undefined)
    ?.description;
  return (
    <tr className="border-b">
      <td className="py-2 pr-4">
        <Link
          to="/workflows/$workflowId"
          params={{ workflowId: workflow.id }}
          className="font-medium underline-offset-4 hover:underline"
        >
          {workflow.id}
        </Link>
      </td>
      <td className="py-2 pr-4 text-muted-foreground">{description ?? "—"}</td>
      <td className="py-2 pr-4">
        {workflow.active_version ? (
          <Badge variant="default">v{workflow.active_version}</Badge>
        ) : (
          <span className="text-muted-foreground">none</span>
        )}
      </td>
      <td className="py-2">{formatTime(lastChange(versions.data) ?? workflow.created_at)}</td>
    </tr>
  );
}

function lastChange(versions: VersionSummary[] | undefined): string | undefined {
  const times = versions?.flatMap((v) => [v.created_at, v.published_at ?? v.created_at]);
  return times?.sort().at(-1);
}
