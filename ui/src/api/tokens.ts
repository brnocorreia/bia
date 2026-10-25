import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { getApi, type components } from "./client";
import { unwrap } from "./request";

export type TokenInfo = components["schemas"]["TokenInfo"];
export type NewTokenRequest = components["schemas"]["NewTokenRequest"];
export type CreatedToken = components["schemas"]["CreatedToken"];

export const tokenKeys = { all: ["tokens"] as const };

export function useTokens() {
  return useQuery({
    queryKey: tokenKeys.all,
    queryFn: async () => unwrap(await getApi().GET("/v1/tokens")).tokens,
  });
}

/**
 * Creates a Token. A plain function, not a `useMutation`, so the one-time value stays out of
 * the TanStack Query cache; the caller holds it in component state until it is dismissed.
 */
export async function createToken(request: NewTokenRequest): Promise<CreatedToken> {
  return unwrap(await getApi().POST("/v1/tokens", { body: request }));
}

export function useRevokeToken() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (id: string) =>
      unwrap(
        await getApi().POST("/v1/tokens/{token_id}/revoke", {
          params: { path: { token_id: id } },
        }),
      ),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: tokenKeys.all }),
  });
}
