import { createFileRoute, Link, useNavigate } from "@tanstack/react-router";
import { useApprovalRequests } from "@/api/approvals";
import { messageOf } from "@/api/request";
import { ACTION_LABEL, age, displayState, inTab, type Tab } from "@/approvals/model";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";

interface Search {
  tab?: Tab;
}

export const Route = createFileRoute("/_app/approvals")({
  staticData: { nav: { label: "Approvals", order: 15 } },
  validateSearch: (search: Record<string, unknown>): Search => ({
    tab: search.tab === "decided" ? "decided" : undefined,
  }),
  component: Approvals,
});

function Approvals() {
  const tab: Tab = Route.useSearch().tab ?? "pending";
  const navigate = useNavigate({ from: Route.fullPath });
  const requests = useApprovalRequests();
  const shown = requests.data?.filter((request) => inTab(request, tab)) ?? [];
  const pendingCount = requests.data?.filter((request) => inTab(request, "pending")).length ?? 0;

  return (
    <section>
      <h1 className="text-2xl font-semibold">Approvals</h1>
      <p className="mt-1 text-sm text-muted-foreground">
        Agents ask for a Version to be published or activated; an admin reviews and decides.
      </p>
      <div role="tablist" aria-label="Approval requests" className="mt-4 flex gap-1">
        {(["pending", "decided"] as const).map((name) => (
          <Button
            key={name}
            role="tab"
            aria-selected={tab === name}
            variant={tab === name ? "secondary" : "ghost"}
            size="sm"
            onClick={() => void navigate({ search: { tab: name } })}
          >
            {name === "pending" ? `Pending (${pendingCount})` : "Decided"}
          </Button>
        ))}
      </div>
      {requests.isPending && <p className="mt-4 text-sm text-muted-foreground">Loading…</p>}
      {requests.error && (
        <p role="alert" className="mt-4 text-sm text-destructive">
          {messageOf(requests.error)}
        </p>
      )}
      {requests.data && shown.length === 0 && (
        <p className="mt-4 text-sm text-muted-foreground">
          {tab === "pending" ? "Nothing is waiting for review." : "No decided requests yet."}
        </p>
      )}
      {shown.length > 0 && (
        <table className="mt-4 w-full text-left text-sm">
          <thead className="border-b text-muted-foreground">
            <tr>
              <th className="py-2 pr-4 font-medium">Workflow</th>
              <th className="py-2 pr-4 font-medium">Version</th>
              <th className="py-2 pr-4 font-medium">Action</th>
              <th className="py-2 pr-4 font-medium">Requested by</th>
              <th className="py-2 pr-4 font-medium">Note</th>
              <th className="py-2 pr-4 font-medium">Age</th>
              {tab === "decided" && <th className="py-2 font-medium">Status</th>}
            </tr>
          </thead>
          <tbody>
            {shown.map((request) => (
              <tr key={request.id} className="border-b">
                <td className="py-2 pr-4">
                  <Link
                    to="/approvals/$requestId"
                    params={{ requestId: request.id }}
                    className="font-mono underline-offset-4 hover:underline"
                  >
                    {request.workflow_id}
                  </Link>
                </td>
                <td className="py-2 pr-4">v{request.number}</td>
                <td className="py-2 pr-4">{ACTION_LABEL[request.action]}</td>
                <td className="py-2 pr-4">{request.requester_name}</td>
                <td className="max-w-xs truncate py-2 pr-4">{request.note ?? "—"}</td>
                <td className="py-2 pr-4">{age(request.created_at, new Date())}</td>
                {tab === "decided" && (
                  <td className="py-2">
                    <Badge>{displayState(request).label}</Badge>
                  </td>
                )}
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}
