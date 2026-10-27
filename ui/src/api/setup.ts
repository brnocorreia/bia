import { getApi, type components } from "./client";
import { ApiError, unwrap } from "./request";

export type SetupDone = components["schemas"]["SetupDone"];

/** Counted in code points (`[...password].length`), as the API counts characters. */
export const MIN_PASSWORD_LENGTH = 12;

/**
 * Sets up the User with the Setup Link's code. A plain function, not a `useMutation`, so the
 * password and the returned Token stay out of the TanStack Query cache.
 */
export async function setUp(code: string, password: string): Promise<SetupDone> {
  return unwrap(await getApi().POST("/v1/setup", { body: { code, password } }));
}

/** What to tell the person on the setup page when setup is refused. */
export function setupErrorMessage(error: unknown): string {
  if (!(error instanceof ApiError)) return "Cannot reach the API. Check that bia is running.";
  switch (error.code) {
    case "invalid_setup_code":
      return "This Setup Link's code is wrong. Copy the link from the bia serve logs again.";
    case "setup_code_expired":
      return "This Setup Link is from before the server restarted. Use the link it logged since.";
    case "already_set_up":
      return "bia is already set up. To start over, restart it with bia serve --reset-admin.";
    default:
      return error.message;
  }
}
