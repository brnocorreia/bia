import { getApi, type components } from "./client";
import { ApiError, unwrap } from "./request";

export type PasswordChanged = components["schemas"]["PasswordChanged"];

/**
 * Replaces the password. A plain function, not a `useMutation`, so neither password reaches
 * the TanStack Query cache.
 */
export async function changePassword(current: string, next: string): Promise<PasswordChanged> {
  return unwrap(await getApi().POST("/v1/user/password", { body: { current, new: next } }));
}

/** What to tell the person who asked to change their password when the API refuses. */
export function passwordChangeErrorMessage(error: unknown): string {
  if (!(error instanceof ApiError)) return "Cannot reach the API. Check that bia is running.";
  return error.code === "invalid_credentials"
    ? "The current password is not right."
    : error.message;
}
