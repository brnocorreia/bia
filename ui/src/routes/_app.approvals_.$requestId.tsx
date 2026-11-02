import { createFileRoute, Link } from "@tanstack/react-router";
import { useState } from "react";
import {
  useApprovalRequest,
  useDecision,
  usePublishDiagnostics,
  type ApprovalRequestInfo,
} from "@/api/approvals";
import { ApiError, messageOf } from "@/api/request";
import { useVersion, useWorkflow } from "@/api/workflows";
import { ACTION_LABEL, displayState } from "@/approvals/model";
import { useCurrentToken } from "@/auth/scope";
import { DefinitionViewer } from "@/components/definition-viewer";
import { DiagnosticsList } from "@/components/diagnostics-list";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { VersionDiff } from "@/components/version-diff";
import { formatTime } from "@/lib/format";

export const Route = createFileRoute("/_app/approvals_/$requestId")({
  component: Review,
});

function Review() {
  const { requestId } = Route.useParams();
  const request = useApprovalRequest(requestId);

  if (request.error) {
    return (
      <p role="alert" className="text-sm text-destructive">
        {messageOf(request.error)}
      </p>
    );
  }
  if (!request.data) return <p className="text-sm text-muted-foreground">Loading…</p>;
  return <ReviewOf request={request.data} />;
}

function ReviewOf({ request }: { request: ApprovalRequestInfo }) {
  const state = displayState(request);
  const workflow = useWorkflow(request.workflow_id);
  const activeNumber = workflow.data?.active_version ?? undefined;
  const target = useVersion(request.workflow_id, request.number);
  const publishes = request.action !== "activate";
  // Only a pending request still has something to validate; a decided one is history.
  const diagnostics = usePublishDiagnostics(
    request.workflow_id,
    request.number,
    publishes && request.status === "pending",
  );

  return (
    <section className="flex flex-col gap-6">
      <div>
        <Link to="/approvals" className="text-sm text-muted-foreground hover:underline">
          ← Approvals
        </Link>
        <h1 className="mt-1 text-2xl font-semibold">
          {ACTION_LABEL[request.action]} v{request.number} of{" "}
          <Link
            to="/workflows/$workflowId"
            params={{ workflowId: request.workflow_id }}
            className="font-mono underline-offset-4 hover:underline"
          >
            {request.workflow_id}
          </Link>
        </h1>
        <p className="mt-1 flex items-center gap-2 text-sm text-muted-foreground">
          <Badge aria-label="Status">{state.label}</Badge>
          Requested by {request.requester_name}, {formatTime(request.created_at)}
        </p>
      </div>

      <div>
        <h2 className="mb-2 text-lg font-semibold">Note from the requester</h2>
        <p className="text-sm">{request.note || "No note."}</p>
      </div>

      {request.status !== "pending" && (
        <div role="status" className="rounded-md border p-3 text-sm">
          <p>{state.reason}</p>
          {request.closed_at && (
            <p className="mt-1 text-muted-foreground">{formatTime(request.closed_at)}</p>
          )}
          {request.comment && (
            <p className="mt-2">
              Comment: <span data-testid="decision-comment">{request.comment}</span>
            </p>
          )}
        </div>
      )}

      <div>
        <h2 className="mb-2 text-lg font-semibold">
          {activeNumber !== undefined
            ? `Changes from the Active Version (v${activeNumber})`
            : "Definition"}
        </h2>
        {workflow.isPending ? (
          <p className="text-sm text-muted-foreground">Loading…</p>
        ) : activeNumber !== undefined ? (
          <VersionDiff workflowId={request.workflow_id} from={activeNumber} to={request.number} />
        ) : (
          <>
            <p className="mb-2 text-sm text-muted-foreground">
              No Active Version yet, so everything below is new.
            </p>
            <DefinitionViewer definition={target.data?.definition} />
          </>
        )}
      </div>

      {diagnostics.data && diagnostics.data.diagnostics.length > 0 && (
        <div>
          <h2 className="mb-2 text-lg font-semibold">Diagnostics if published now</h2>
          <DiagnosticsList diagnostics={diagnostics.data.diagnostics} />
        </div>
      )}

      {state.canDecide && (
        <Decision request={request} blocked={diagnostics.data?.valid === false} />
      )}
    </section>
  );
}

function Decision({ request, blocked }: { request: ApprovalRequestInfo; blocked: boolean }) {
  const token = useCurrentToken();
  const isAdmin = token.data?.scope === "admin";
  const [comment, setComment] = useState("");
  const decision = useDecision(request.id, request.workflow_id);
  const hint = isAdmin ? undefined : "Needs an admin Token";
  const decide = (kind: "approve" | "reject") => decision.mutate({ decision: kind, comment });

  return (
    <div className="flex flex-col gap-3">
      <h2 className="text-lg font-semibold">Decision</h2>
      <label className="flex flex-col gap-1 text-sm">
        Comment (optional, the requester sees it)
        <textarea
          className="min-h-20 rounded-md border border-input bg-transparent px-3 py-2 text-sm shadow-xs focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50"
          maxLength={2000}
          value={comment}
          disabled={!isAdmin}
          onChange={(event) => setComment(event.target.value)}
        />
      </label>
      {decision.error && <DecisionError error={decision.error} />}
      <div className="flex gap-2">
        <Button
          disabled={!isAdmin || decision.isPending || blocked}
          title={hint ?? (blocked ? "Publishing would fail; see the Diagnostics" : undefined)}
          onClick={() => decide("approve")}
        >
          Approve
        </Button>
        <Button
          variant="outline"
          disabled={!isAdmin || decision.isPending}
          title={hint}
          onClick={() => decide("reject")}
        >
          Reject
        </Button>
      </div>
      {!isAdmin && token.data && (
        <p className="text-xs text-muted-foreground">
          Approving and rejecting need an admin Token; this one is {token.data.scope}.
        </p>
      )}
    </div>
  );
}

const EXPLANATION: Record<string, string> = {
  approval_request_not_pending:
    "This request is no longer pending, most likely because the Draft was replaced or the requester withdrew it.",
  version_changed:
    "The Version's content is not what the agent requested, so it was not published. Reject this request and ask for a new one.",
  version_published: "This Version is already Published, so it cannot be published again as asked.",
  version_not_published: "This Version is not Published, so it cannot be activated.",
};

function DecisionError({ error }: { error: unknown }) {
  const code = error instanceof ApiError ? error.code : undefined;
  const diagnostics = error instanceof ApiError ? (error.body?.diagnostics ?? []) : [];
  return (
    <div role="alert" className="flex flex-col gap-2 text-sm text-destructive">
      <p>{messageOf(error)}</p>
      {code && EXPLANATION[code] && <p className="text-foreground">{EXPLANATION[code]}</p>}
      <DiagnosticsList diagnostics={diagnostics} />
    </div>
  );
}
