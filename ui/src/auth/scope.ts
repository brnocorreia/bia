import { useQuery } from "@tanstack/react-query";
import { getApi, type components } from "@/api/client";
import { unwrap } from "@/api/request";
import { useToken } from "./session";

export type Scope = components["schemas"]["ScopeName"];

/**
 * The Token the UI is signed in with, as the API describes it. The Scope is read from the
 * server (`GET /v1/tokens/me`) rather than guessed, and is keyed by the session Token so
 * signing in as someone else never shows the previous Scope.
 */
export function useCurrentToken() {
  const token = useToken();
  return useQuery({
    queryKey: ["current-token", token],
    enabled: token !== null,
    staleTime: 60_000,
    queryFn: async () => unwrap(await getApi().GET("/v1/tokens/me")),
  });
}

/**
 * Whether the signed-in Token is `admin`. `false` while the Scope is still loading, so an
 * admin-only control is never offered before the server has said it is allowed. This only
 * decides what the UI offers: the server enforces the Scope on every request.
 */
export function useIsAdmin(): boolean {
  return useCurrentToken().data?.scope === "admin";
}
