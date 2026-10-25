import createClient, { type Client, type Middleware } from "openapi-fetch";
import { session as appSession, type Session } from "@/auth/session";
import type { components, paths } from "./schema.gen";

export type { components, paths };
export type ErrorBody = components["schemas"]["ErrorBody"];
export type ApiClient = Client<paths>;

/**
 * Sends the session's Token and, when the API rejects it with 401, ends the session so the
 * app shell returns to the login screen. A request that sets its own `Authorization` (the
 * login probe) is left alone: its failure is the login form's to show.
 */
export function createApi(apiUrl: string, session: Session = appSession): ApiClient {
  const auth: Middleware = {
    onRequest({ request }) {
      const token = session.getToken();
      if (token && !request.headers.has("Authorization")) {
        request.headers.set("Authorization", `Bearer ${token}`);
      }
      return request;
    },
    onResponse({ request, response }) {
      // Compare with the Token that was sent, so a late 401 for an old Token cannot end a
      // newer session.
      const token = session.getToken();
      if (
        response.status === 401 &&
        token &&
        request.headers.get("Authorization") === `Bearer ${token}`
      ) {
        session.clear();
      }
      return response;
    },
  };
  const client = createClient<paths>({ baseUrl: apiUrl });
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
