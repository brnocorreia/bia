import { useQuery } from "@tanstack/react-query";
import { getApi, type components } from "./client";
import { ApiError, unwrap } from "./request";
import { assertionJson, creationOptions, registrationJson, requestOptions } from "@/auth/webauthn";

export type PasskeyInfo = components["schemas"]["PasskeyInfo"];

export const passkeyKeys = { all: ["passkeys"] as const };

export function usePasskeys(enabled: boolean) {
  return useQuery({
    queryKey: passkeyKeys.all,
    enabled,
    queryFn: async () => unwrap(await getApi().GET("/v1/passkeys")).passkeys,
  });
}

/** Asks the browser for a new passkey and registers it under `name`. */
export async function registerPasskey(name: string): Promise<PasskeyInfo> {
  const api = getApi();
  const begun = unwrap(await api.POST("/v1/passkeys/registration/begin", { body: { name } }));
  const credential = (await navigator.credentials.create({
    publicKey: creationOptions(begun.options),
  })) as PublicKeyCredential | null;
  if (!credential) throw new Error("The browser did not create a passkey.");
  return unwrap(
    await api.POST("/v1/passkeys/registration/finish", {
      body: { ceremony_id: begun.ceremony_id, credential: registrationJson(credential) },
    }),
  );
}

export async function renamePasskey(id: string, name: string): Promise<PasskeyInfo> {
  return unwrap(
    await getApi().PATCH("/v1/passkeys/{passkey_id}", {
      params: { path: { passkey_id: id } },
      body: { name },
    }),
  );
}

export async function removePasskey(id: string): Promise<void> {
  const { error, response } = await getApi().DELETE("/v1/passkeys/{passkey_id}", {
    params: { path: { passkey_id: id } },
  });
  if (error !== undefined || !response.ok) {
    throw new ApiError(
      response.status,
      typeof error === "object" && error !== null && "code" in error
        ? (error as components["schemas"]["ErrorBody"])
        : null,
    );
  }
}

/** Asks the browser to sign with a passkey; the answer goes to `session/passkey/finish`. */
export async function signInWithPasskeyCeremony() {
  const api = getApi();
  const begun = unwrap(await api.POST("/v1/session/passkey/begin"));
  const credential = (await navigator.credentials.get({
    publicKey: requestOptions(begun.options),
  })) as PublicKeyCredential | null;
  if (!credential) throw new Error("The browser did not return a passkey.");
  return unwrap(
    await api.POST("/v1/session/passkey/finish", {
      body: { ceremony_id: begun.ceremony_id, credential: assertionJson(credential) },
    }),
  );
}

/** What to tell the person whose passkey action failed. */
export function passkeyErrorMessage(error: unknown): string {
  if (error instanceof ApiError) {
    if (error.code === "reauthentication_required") {
      return "Sign out and sign in again, then remove it: removing a passkey needs a sign-in from the last 10 minutes.";
    }
    return error.message;
  }
  // The browser's own refusal: the person dismissed the prompt or no authenticator answered.
  if (error instanceof DOMException) {
    return error.name === "NotAllowedError"
      ? "The passkey prompt was cancelled or timed out."
      : error.message;
  }
  if (error instanceof Error) return error.message;
  return "Cannot reach the API. Check that bia is running.";
}
