import { getApi } from "@/api/client";
import { ApiError, unwrap } from "@/api/request";
import { getConfig } from "@/config";
import { session } from "./session";

export type SignInResult =
  | { ok: true }
  | {
      ok: false;
      /** `same-site`: the API accepted the password but the browser dropped its cookie. */
      kind: "credentials" | "same-site" | "other";
      message: string;
    };

const SAME_SITE =
  "Signed in, but the browser did not keep the Session cookie. The UI and the API must be " +
  "same-site, for example ui.example.com with api.example.com. See docs/deploy-ui.md.";

/**
 * Asks the API who the Session is. Right after a sign-in a 401 here means the cookie never
 * arrived, which is what a UI and an API on different sites cause.
 */
export async function confirmSession(): Promise<SignInResult> {
  try {
    const { response } = await getApi().GET("/v1/tokens/me");
    if (response.ok) {
      session.signedIn();
      return { ok: true };
    }
    if (response.status === 401) return { ok: false, kind: "same-site", message: SAME_SITE };
    return { ok: false, kind: "other", message: `The API answered ${response.status}.` };
  } catch {
    return unreachable();
  }
}

/**
 * Signs in with the password. A plain function, not a `useMutation`, so the password stays out
 * of the TanStack Query cache.
 */
export async function signIn(password: string): Promise<SignInResult> {
  try {
    unwrap(await getApi().POST("/v1/session", { body: { password } }));
  } catch (failure) {
    if (failure instanceof ApiError) {
      return failure.code === "invalid_credentials"
        ? { ok: false, kind: "credentials", message: "That password is not right." }
        : { ok: false, kind: "other", message: failure.message };
    }
    return unreachable();
  }
  return confirmSession();
}

/** Ends the Session on the server; the UI signs out even when the server cannot be reached. */
export async function signOut(): Promise<void> {
  try {
    await getApi().DELETE("/v1/session");
  } catch {
    /* the cookie expires on its own */
  } finally {
    session.signedOut();
  }
}

function unreachable(): SignInResult {
  const { apiUrl } = getConfig();
  return {
    ok: false,
    kind: "other",
    message: `Cannot reach the API at ${apiUrl || "this origin"}.`,
  };
}
