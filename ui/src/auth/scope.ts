import { queryOptions, useQuery } from "@tanstack/react-query";
import { getApi, type components } from "@/api/client";
import { unwrap } from "@/api/request";
import { useSessionState } from "./session";

export type Scope = components["schemas"]["ScopeName"];

/** The Token behind the Session, as the API describes it (`GET /v1/tokens/me`, ADR-0064). */
export const currentTokenQuery = queryOptions({
  queryKey: ["current-token"],
  staleTime: 60_000,
  queryFn: async () => unwrap(await getApi().GET("/v1/tokens/me")),
});

/**
 * The Token the UI is signed in with, as the API describes it. The Scope is read from the
 * server rather than guessed. The cache is cleared on sign-out, so signing in as someone else
 * never shows the previous Scope.
 */
export function useCurrentToken() {
  const state = useSessionState();
  return useQuery({ ...currentTokenQuery, enabled: state === "signed-in" });
}

/**
 * Whether the signed-in Token is `admin`. `false` while the Scope is still loading, so an
 * admin-only control is never offered before the server has said it is allowed. This only
 * decides what the UI offers: the server enforces the Scope on every request.
 */
export function useIsAdmin(): boolean {
  return useCurrentToken().data?.scope === "admin";
}
