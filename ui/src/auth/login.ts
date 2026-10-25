import { createApi, describeError } from "@/api/client";
import { Session } from "./session";

export type LoginResult = { ok: true } | { ok: false; message: string };

/**
 * Checks a Token against the API before the session keeps it. The API has no "who am I"
 * route, so this reads the Workflow list, which every Scope the UI can use may call. A
 * `webhook` Token is refused with 403 and cannot drive the UI.
 */
export async function verifyToken(apiUrl: string, token: string): Promise<LoginResult> {
  // A throwaway session keeps the probe's 401 from touching the real one.
  const api = createApi(apiUrl, new Session(null));
  try {
    const { response, error } = await api.GET("/v1/workflows", {
      headers: { Authorization: `Bearer ${token}` },
    });
    if (response.ok) return { ok: true };
    if (response.status === 401) return { ok: false, message: "That Token is not valid." };
    if (response.status === 403) {
      return { ok: false, message: "That Token's Scope cannot be used to sign in to the UI." };
    }
    return { ok: false, message: describeError(error) };
  } catch {
    return { ok: false, message: `Cannot reach the API at ${apiUrl || "this origin"}.` };
  }
}
