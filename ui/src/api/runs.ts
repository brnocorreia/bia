import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { getApi, type components } from "./client";
import { pollIntervalMs } from "./polling";
import { unwrap } from "./request";

export type RunSummary = components["schemas"]["RunSummary"];
export type RunInfo = components["schemas"]["RunInfo"];
export type RunStatus = components["schemas"]["RunStatusName"];

export const RUN_STATUSES: RunStatus[] = [
  "pending",
  "running",
  "sleeping",
  "waiting",
  "succeeded",
  "failed",
  "cancelled",
];

/** A Run that has not ended; its status and Timeline can still change. */
export function isActive(run: Pick<RunSummary, "status">): boolean {
  return run.status !== "succeeded" && run.status !== "failed" && run.status !== "cancelled";
}

export interface RunFilters {
  workflow?: string;
  status?: RunStatus;
  /** Inclusive lower bound on creation, an ISO timestamp. */
  from?: string;
  /** Exclusive upper bound on creation, an ISO timestamp. */
  to?: string;
}

export const runKeys = {
  all: ["runs"] as const,
  list: (filters: RunFilters) => ["runs", "list", filters] as const,
  one: (id: string) => ["runs", "one", id] as const,
};

const PAGE_SIZE = 25;

export function useRuns(filters: RunFilters) {
  return useInfiniteQuery({
    queryKey: runKeys.list(filters),
    initialPageParam: undefined as string | undefined,
    queryFn: async ({ pageParam }) =>
      unwrap(
        await getApi().GET("/v1/runs", {
          params: {
            query: {
              workflow_id: filters.workflow,
              status: filters.status,
              created_from: filters.from,
              created_to: filters.to,
              after: pageParam,
              limit: PAGE_SIZE,
            },
          },
        }),
      ),
    getNextPageParam: (page) => page.next_after ?? undefined,
    refetchInterval: (query) =>
      query.state.data?.pages.some((page) => page.runs.some(isActive)) ? pollIntervalMs() : false,
  });
}

/** One Run with its Timeline, refreshed by polling until it has ended. */
export function useRun(id: string) {
  return useQuery({
    queryKey: runKeys.one(id),
    queryFn: async () =>
      unwrap(await getApi().GET("/v1/runs/{run_id}", { params: { path: { run_id: id } } })),
    refetchInterval: (query) =>
      query.state.data && !isActive(query.state.data) ? false : pollIntervalMs(),
  });
}

export function useTriggerRun() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async ({ workflowId, input }: { workflowId: string; input: unknown }) =>
      unwrap(
        await getApi().POST("/v1/workflows/{workflow_id}/runs", {
          params: { path: { workflow_id: workflowId } },
          body: { input },
        }),
      ),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: runKeys.all }),
  });
}

export function useRunAction(id: string, action: "cancel" | "resume" | "retry") {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async () => {
      const params = { path: { run_id: id } };
      if (action === "cancel") {
        return unwrap(await getApi().POST("/v1/runs/{run_id}/cancel", { params }));
      }
      if (action === "resume") {
        return unwrap(await getApi().POST("/v1/runs/{run_id}/resume", { params }));
      }
      return unwrap(await getApi().POST("/v1/runs/{run_id}/retry", { params }));
    },
    onSuccess: () => queryClient.invalidateQueries({ queryKey: runKeys.all }),
  });
}
