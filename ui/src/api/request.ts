import type { ErrorBody } from "./client";

/** A non-2xx answer. `body` is the API's uniform error body when it sent one. */
export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly body: ErrorBody | null,
  ) {
    super(body?.message ?? `The request failed with status ${status}.`);
  }

  get code() {
    return this.body?.code;
  }
}

/** Returns `data` of an `openapi-fetch` result, or throws an [`ApiError`] so TanStack Query sees it. */
export function unwrap<T>(result: { data?: T; error?: unknown; response: Response }): T {
  if (result.error !== undefined || result.data === undefined) {
    const body =
      typeof result.error === "object" && result.error !== null && "code" in result.error
        ? (result.error as ErrorBody)
        : null;
    throw new ApiError(result.response.status, body);
  }
  return result.data;
}

/** What to show a user for a failed mutation or query. */
export function messageOf(error: unknown): string {
  if (error instanceof ApiError && error.code === "insufficient_scope") {
    return "Your Token's Scope does not allow this. Sign in with an admin Token.";
  }
  if (error instanceof Error) return error.message;
  return "The request failed.";
}
