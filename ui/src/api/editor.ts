import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { getApi, type components } from "./client";
import { unwrap } from "./request";
import { workflowKeys } from "./workflows";

export type ValidationReport = components["schemas"]["ValidationReport"];
export type SavedVersion = components["schemas"]["SavedVersion"];

/** The DSL JSON Schema; it only changes with the server, so it is fetched once per session. */
export function useDefinitionSchema() {
  return useQuery({
    queryKey: ["schema"],
    staleTime: Infinity,
    queryFn: async () => unwrap(await getApi().GET("/v1/schema")).definition,
  });
}

/** Validates a Definition as a Draft. Diagnostics are the result, so errors in it are a 200. */
export async function validateDefinition(definition: unknown): Promise<ValidationReport> {
  return unwrap(
    await getApi().POST("/v1/validate", {
      params: { query: { target: "draft" } },
      body: definition as never,
    }),
  );
}

/** Saves a Definition as a new Draft (creating the Workflow if it is the first), or replaces `replace`. */
export function useSaveDraft(workflowId: string | undefined) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async ({ definition, replace }: { definition: unknown; replace?: number }) => {
      const id = workflowId ?? (definition as { id?: unknown } | null)?.id;
      if (typeof id !== "string" || id === "") {
        throw new Error("The Definition needs an `id`: it names the Workflow.");
      }
      const path = { workflow_id: id };
      if (replace !== undefined) {
        return unwrap(
          await getApi().PUT("/v1/workflows/{workflow_id}/versions/{number}", {
            params: { path: { ...path, number: replace } },
            body: definition as never,
          }),
        );
      }
      return unwrap(
        await getApi().POST("/v1/workflows/{workflow_id}/versions", {
          params: { path },
          body: definition as never,
        }),
      );
    },
    onSuccess: async (_saved, { definition }) => {
      const id = workflowId ?? (definition as { id: string }).id;
      await queryClient.invalidateQueries({ queryKey: workflowKeys.all });
      await queryClient.invalidateQueries({ queryKey: workflowKeys.one(id) });
      await queryClient.invalidateQueries({ queryKey: workflowKeys.versions(id) });
      await queryClient.invalidateQueries({ queryKey: ["workflow", id, "version"] });
    },
  });
}
