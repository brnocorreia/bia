import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { getApi, type components } from "./client";
import { unwrap } from "./request";

export type SecretInfo = components["schemas"]["SecretInfo"];

export const secretKeys = { all: ["secrets"] as const };

export function useSecrets() {
  return useQuery({
    queryKey: secretKeys.all,
    queryFn: async () => unwrap(await getApi().GET("/v1/secrets")).secrets,
  });
}

/**
 * Sets or replaces a Secret's value. A plain function, not a `useMutation`, because TanStack
 * Query keeps a mutation's variables in its cache and the value must live only in the request.
 */
export async function putSecret(name: string, value: string): Promise<SecretInfo> {
  return unwrap(
    await getApi().PUT("/v1/secrets/{name}", {
      params: { path: { name } },
      body: { value },
    }),
  );
}

export function useDeleteSecret() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (name: string) =>
      unwrap(await getApi().DELETE("/v1/secrets/{name}", { params: { path: { name } } })),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: secretKeys.all }),
  });
}
