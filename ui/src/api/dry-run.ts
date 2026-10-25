import { useMutation, useQuery } from "@tanstack/react-query";
import { getApi, type components } from "./client";
import { unwrap } from "./request";

export type DryRunReport = components["schemas"]["DryRunReport"];
export type ActionEntry = components["schemas"]["ActionEntry"];

/** The Action catalogue, whose examples are what a side-effecting Step returns without a Mock. */
export function useActions() {
  return useQuery({
    queryKey: ["schema", "actions"],
    staleTime: Infinity,
    queryFn: async () => unwrap(await getApi().GET("/v1/schema")).actions,
  });
}

/** Dry-runs a Definition that need not be saved (ADR-0046). `mocks` is the API's own format. */
export function useDryRun() {
  return useMutation({
    mutationFn: async (body: {
      definition: unknown;
      input: unknown;
      mocks: Record<string, unknown>;
    }) => unwrap(await getApi().POST("/v1/dry-run", { body: body as never })),
  });
}
