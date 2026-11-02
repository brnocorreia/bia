import { createHash, randomBytes } from "node:crypto";
import type { APIRequestContext, Page } from "@playwright/test";
import { e2eEnv } from "./env";
import { expect, uniqueName } from "./fixtures";

export const CALLBACK = "http://127.0.0.1:49999/callback";

export interface Pkce {
  verifier: string;
  challenge: string;
}

export function pkce(): Pkce {
  const verifier = randomBytes(32).toString("base64url");
  return { verifier, challenge: createHash("sha256").update(verifier).digest("base64url") };
}

/** An app that registered itself, as an MCP client without a metadata document does. */
export async function registerApp(request: APIRequestContext) {
  const name = uniqueName("e2e-app");
  const response = await request.post(`${e2eEnv().apiUrl}/oauth/register`, {
    data: { client_name: name, redirect_uris: [CALLBACK] },
  });
  expect(response.status()).toBe(201);
  return { name, clientId: (await response.json()).client_id as string };
}

/** The server's own name for itself: the `iss` it sends and the `resource` it accepts. */
export async function issuer(request: APIRequestContext): Promise<string> {
  const response = await request.get(`${e2eEnv().apiUrl}/.well-known/oauth-authorization-server`);
  return (await response.json()).issuer as string;
}

export function authorizeUrl(clientId: string, resource: string, challenge: string, scope: string) {
  const query = new URLSearchParams({
    response_type: "code",
    client_id: clientId,
    redirect_uri: CALLBACK,
    code_challenge: challenge,
    code_challenge_method: "S256",
    resource,
    scope,
    state: "e2e-state",
  });
  return `${e2eEnv().apiUrl}/oauth/authorize?${query}`;
}

/** The app's callback has no server behind it; the test reads what the browser was sent to. */
export async function catchCallback(page: Page) {
  await page.route("http://127.0.0.1:49999/**", (route) =>
    route.fulfill({ status: 200, body: "callback" }),
  );
}
