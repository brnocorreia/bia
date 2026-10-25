import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { getApi, type components } from "./client";
import { unwrap } from "./request";

export type WorkflowInfo = components["schemas"]["WorkflowInfo"];
export type VersionSummary = components["schemas"]["VersionSummary"];
export type VersionInfo = components["schemas"]["VersionInfo"];

export const workflowKeys = {
  all: ["workflows"] as const,
  one: (id: string) => ["workflow", id] as const,
  versions: (id: string) => ["workflow", id, "versions"] as const,
  version: (id: string, number: number) => ["workflow", id, "version", number] as const,
  diff: (id: string, from: number, to: number) => ["workflow", id, "diff", from, to] as const,
};

export function useWorkflows() {
  return useQuery({
    queryKey: workflowKeys.all,
    queryFn: async () => unwrap(await getApi().GET("/v1/workflows")).workflows,
  });
}

export function useWorkflow(id: string) {
  return useQuery({
    queryKey: workflowKeys.one(id),
    queryFn: async () =>
      unwrap(
        await getApi().GET("/v1/workflows/{workflow_id}", {
          params: { path: { workflow_id: id } },
        }),
      ),
  });
}

/** A Workflow's Versions, oldest first. */
export function useVersions(id: string) {
  return useQuery({
    queryKey: workflowKeys.versions(id),
    queryFn: async () =>
      unwrap(
        await getApi().GET("/v1/workflows/{workflow_id}/versions", {
          params: { path: { workflow_id: id } },
        }),
      ).versions,
  });
}

export function useVersion(id: string, number: number | undefined) {
  return useQuery({
    queryKey: workflowKeys.version(id, number ?? 0),
    enabled: number !== undefined,
    queryFn: async () =>
      unwrap(
        await getApi().GET("/v1/workflows/{workflow_id}/versions/{number}", {
          params: { path: { workflow_id: id, number: number! } },
        }),
      ),
  });
}

export function useDiff(id: string, from: number | undefined, to: number | undefined) {
  return useQuery({
    queryKey: workflowKeys.diff(id, from ?? 0, to ?? 0),
    enabled: from !== undefined && to !== undefined,
    queryFn: async () =>
      unwrap(
        await getApi().GET("/v1/workflows/{workflow_id}/diff", {
          params: { path: { workflow_id: id }, query: { from: from!, to: to! } },
        }),
      ),
  });
}

/** Publish or activate (which is also how a rollback is done) one Version. */
export function useVersionAction(id: string, action: "publish" | "activate") {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (number: number) => {
      const params = { path: { workflow_id: id, number } };
      if (action === "publish") {
        return unwrap(
          await getApi().POST("/v1/workflows/{workflow_id}/versions/{number}/publish", { params }),
        );
      }
      return unwrap(
        await getApi().POST("/v1/workflows/{workflow_id}/versions/{number}/activate", { params }),
      );
    },
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: workflowKeys.one(id) });
      await queryClient.invalidateQueries({ queryKey: workflowKeys.all });
    },
  });
}
