import createClient, { type Client, type Middleware } from "openapi-fetch";
import { session as appSession, type Session } from "@/auth/session";
import type { components, paths } from "./schema.gen";

export type { components, paths };
export type ErrorBody = components["schemas"]["ErrorBody"];
export type ApiClient = Client<paths>;

/** The codes that say the Session (or Token) the request carried is no longer accepted. */
const UNAUTHENTICATED = new Set(["missing_token", "invalid_token"]);

/**
 * Sends the Session cookie with every request and, when the API answers that it no longer
 * knows the caller, ends the session so the app shell returns to the sign-in screen. Other
 * 401s, such as a wrong password, belong to the form that asked.
 */
export function createApi(apiUrl: string, session: Session = appSession): ApiClient {
  const auth: Middleware = {
    async onResponse({ response }) {
      if (response.status === 401) {
        const body: unknown = await response
          .clone()
          .json()
          .catch(() => null);
        const code =
          typeof body === "object" && body !== null && "code" in body ? body.code : undefined;
        if (typeof code === "string" && UNAUTHENTICATED.has(code)) session.signedOut();
      }
      return response;
    },
  };
  const client = createClient<paths>({ baseUrl: apiUrl, credentials: "include" });
  client.use(auth);
  return client;
}

let current: ApiClient | null = null;

/** Called once at startup, after `config.json` is read. */
export function configureApi(apiUrl: string) {
  current = createApi(apiUrl);
}

/** The client every screen uses; typed by the OpenAPI document the API serves. */
export function getApi(): ApiClient {
  if (!current) throw new Error("configureApi must run before the first request");
  return current;
}

/** A human-readable line for an error body, or for a network failure that has none. */
export function describeError(error: unknown): string {
  if (typeof error === "object" && error !== null && "message" in error) {
    return String((error as { message: unknown }).message);
  }
  return "The request failed.";
}
