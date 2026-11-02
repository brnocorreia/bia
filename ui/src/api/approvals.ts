import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { getApi, type components } from "./client";
import { pollIntervalMs } from "./polling";
import { unwrap } from "./request";
import { workflowKeys } from "./workflows";

export type ApprovalRequestInfo = components["schemas"]["ApprovalRequestInfo"];
export type ApprovalStatus = ApprovalRequestInfo["status"];

export const approvalKeys = {
  all: ["approvals"] as const,
  list: (status: ApprovalStatus | undefined, workflow: string | undefined) =>
    ["approvals", "list", status ?? "any", workflow ?? "any"] as const,
  one: (id: string) => ["approvals", "one", id] as const,
};

/** Newest first. Polls, so a request an agent makes (or one that goes stale) shows up unprompted. */
export function useApprovalRequests(filters: { status?: ApprovalStatus; workflow?: string } = {}) {
  return useQuery({
    queryKey: approvalKeys.list(filters.status, filters.workflow),
    queryFn: async () =>
      unwrap(
        await getApi().GET("/v1/approval-requests", {
          params: { query: { status: filters.status, workflow_id: filters.workflow } },
        }),
      ).approval_requests,
    refetchInterval: pollIntervalMs,
  });
}

/** One request, polled while pending so a Draft replaced meanwhile turns it stale. */
export function useApprovalRequest(id: string) {
  return useQuery({
    queryKey: approvalKeys.one(id),
    queryFn: async () =>
      unwrap(await getApi().GET("/v1/approval-requests/{id}", { params: { path: { id } } })),
    refetchInterval: (query) => (query.state.data?.status === "pending" ? pollIntervalMs() : false),
  });
}

/** Approves (which publishes and/or activates) or rejects a request, with an optional comment. */
export function useDecision(id: string, workflowId: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async ({
      decision,
      comment,
    }: {
      decision: "approve" | "reject";
      comment: string;
    }) => {
      const params = { params: { path: { id } }, body: { comment: comment.trim() || undefined } };
      return unwrap(
        decision === "approve"
          ? await getApi().POST("/v1/approval-requests/{id}/approve", params)
          : await getApi().POST("/v1/approval-requests/{id}/reject", params),
      );
    },
    // A failure can mean the request just left `pending`, so refetch on settle either way.
    onSettled: async () => {
      await queryClient.invalidateQueries({ queryKey: approvalKeys.all });
      await queryClient.invalidateQueries({ queryKey: workflowKeys.one(workflowId) });
      await queryClient.invalidateQueries({ queryKey: workflowKeys.versions(workflowId) });
      await queryClient.invalidateQueries({ queryKey: workflowKeys.all });
    },
  });
}

/** What publishing would report about a Version now, for the review page. */
export function usePublishDiagnostics(workflowId: string, number: number, enabled: boolean) {
  return useQuery({
    queryKey: ["approvals", "validate", workflowId, number],
    enabled,
    queryFn: async () =>
      unwrap(
        await getApi().POST("/v1/workflows/{workflow_id}/versions/{number}/validate", {
          params: { path: { workflow_id: workflowId, number }, query: { target: "publish" } },
        }),
      ),
  });
}
