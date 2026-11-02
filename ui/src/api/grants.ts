import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { getApi, type components } from "./client";
import { unwrap } from "./request";
import { tokenKeys } from "./tokens";

export type GrantInfo = components["schemas"]["GrantInfo"];
export type ConsentRequest = components["schemas"]["ConsentRequest"];
export type DeviceConsentRequest = components["schemas"]["DeviceConsentRequest"];
export type ScopeName = components["schemas"]["GrantScopeName"];

export const grantKeys = { all: ["grants"] as const };

export function useGrants() {
  return useQuery({
    queryKey: grantKeys.all,
    queryFn: async () => unwrap(await getApi().GET("/v1/grants")).grants,
  });
}

/** Revokes the Grant with every access token and refresh token it issued. */
export function useRevokeGrant() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (id: string) =>
      unwrap(
        await getApi().POST("/v1/grants/{grant_id}/revoke", {
          params: { path: { grant_id: id } },
        }),
      ),
    onSuccess: () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: grantKeys.all }),
        queryClient.invalidateQueries({ queryKey: tokenKeys.all }),
      ]),
  });
}

/** What the person is asked to approve. The server recorded it; nothing here can change it. */
export async function getConsent(requestId: string): Promise<ConsentRequest> {
  return unwrap(
    await getApi().GET("/oauth/consent/{request_id}", {
      params: { path: { request_id: requestId } },
    }),
  );
}

/** Approves with `scope` (at most what was requested); returns where to send the browser. */
export async function approveConsent(requestId: string, scope: ScopeName): Promise<string> {
  const decision = unwrap(
    await getApi().POST("/oauth/consent/{request_id}/approve", {
      params: { path: { request_id: requestId } },
      body: { scope },
    }),
  );
  return decision.redirect_to;
}

export async function denyConsent(requestId: string): Promise<string> {
  const decision = unwrap(
    await getApi().POST("/oauth/consent/{request_id}/deny", {
      params: { path: { request_id: requestId } },
    }),
  );
  return decision.redirect_to;
}

/** Finds the pending device sign-in a user code names. The server throttles wrong guesses. */
export async function lookupDevice(userCode: string): Promise<DeviceConsentRequest> {
  return unwrap(await getApi().POST("/oauth/device/lookup", { body: { user_code: userCode } }));
}

export async function approveDevice(id: string, scope: ScopeName): Promise<void> {
  unwrap(
    await getApi().POST("/oauth/device/{id}/approve", {
      params: { path: { id } },
      body: { scope },
    }),
  );
}

export async function denyDevice(id: string): Promise<void> {
  unwrap(await getApi().POST("/oauth/device/{id}/deny", { params: { path: { id } } }));
}
