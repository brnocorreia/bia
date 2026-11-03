import { createHash, randomBytes } from "node:crypto";
import type { APIRequestContext, Page } from "@playwright/test";
import { e2eEnv } from "./support/env";
import { ADMIN_PASSWORD } from "./support/env";
import { expect, test, uniqueName } from "./support/fixtures";

const CALLBACK = "http://127.0.0.1:49999/callback";

interface Pkce {
  verifier: string;
  challenge: string;
}

function pkce(): Pkce {
  const verifier = randomBytes(32).toString("base64url");
  return { verifier, challenge: createHash("sha256").update(verifier).digest("base64url") };
}

/** An app that registered itself, as an MCP client without a metadata document does. */
async function registerApp(request: APIRequestContext) {
  const name = uniqueName("e2e-app");
  const response = await request.post(`${e2eEnv().apiUrl}/oauth/register`, {
    data: { client_name: name, redirect_uris: [CALLBACK] },
  });
  expect(response.status()).toBe(201);
  return { name, clientId: (await response.json()).client_id as string };
}

/** The server's own name for itself: the `iss` it sends and the `resource` it accepts. */
async function issuer(request: APIRequestContext): Promise<string> {
  const response = await request.get(`${e2eEnv().apiUrl}/.well-known/oauth-authorization-server`);
  return (await response.json()).issuer as string;
}

function authorizeUrl(clientId: string, resource: string, challenge: string, scope: string) {
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
async function catchCallback(page: Page) {
  await page.route("http://127.0.0.1:49999/**", (route) =>
    route.fulfill({ status: 200, body: "callback" }),
  );
}

test("the admin approves an app, which signs itself in and then shows under Connected apps", async ({
  signedInPage: page,
}) => {
  const app = await registerApp(page.request);
  const resource = await issuer(page.request);
  const { verifier, challenge } = pkce();
  await catchCallback(page);

  await page.goto(authorizeUrl(app.clientId, resource, challenge, "agent"));
  await expect(page.getByRole("heading", { name: `Let ${app.name} use bia?` })).toBeVisible();
  await expect(page.getByText("self-registered, unverified").first()).toBeVisible();
  await expect(page.getByText(CALLBACK)).toBeVisible();
  await page.getByRole("button", { name: "Approve" }).click();

  await page.waitForURL(/127\.0\.0\.1:49999\/callback\?/);
  const answer = new URL(page.url()).searchParams;
  expect(answer.get("state")).toBe("e2e-state");
  expect(answer.get("iss")).toBe(resource);
  const exchange = await page.request.post(`${e2eEnv().apiUrl}/oauth/token`, {
    form: {
      grant_type: "authorization_code",
      client_id: app.clientId,
      code: answer.get("code")!,
      redirect_uri: CALLBACK,
      code_verifier: verifier,
    },
  });
  expect(exchange.status()).toBe(200);
  const issued = await exchange.json();
  const bearer = { Authorization: `Bearer ${issued.access_token}` };
  // The audience was `resource`, which is the whole API.
  expect(
    (await page.request.get(`${e2eEnv().apiUrl}/v1/tokens/me`, { headers: bearer })).status(),
  ).toBe(200);

  await page.goto("/access");
  const row = page.getByRole("row").filter({ hasText: app.name });
  await expect(row).toContainText("self-registered, unverified");
  await expect(row).toContainText("Active");
  await row.getByRole("button", { name: `Revoke ${app.name}` }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Revoke" }).click();
  await expect(row).toContainText("Revoked");
  expect(
    (await page.request.get(`${e2eEnv().apiUrl}/v1/tokens/me`, { headers: bearer })).status(),
  ).toBe(401);
});

test("denying sends the app back with access_denied", async ({ signedInPage: page }) => {
  const app = await registerApp(page.request);
  const resource = await issuer(page.request);
  await catchCallback(page);

  await page.goto(authorizeUrl(app.clientId, resource, pkce().challenge, "agent"));
  await page.getByRole("button", { name: "Deny" }).click();

  await page.waitForURL(/127\.0\.0\.1:49999\/callback\?/);
  const answer = new URL(page.url()).searchParams;
  expect(answer.get("error")).toBe("access_denied");
  expect(answer.get("state")).toBe("e2e-state");
  expect(answer.get("iss")).toBe(resource);
  expect(answer.has("code")).toBe(false);
});

test("an app that asks for admin can be approved as agent", async ({ signedInPage: page }) => {
  const app = await registerApp(page.request);
  const resource = await issuer(page.request);
  const { verifier, challenge } = pkce();
  await catchCallback(page);

  await page.goto(authorizeUrl(app.clientId, resource, challenge, "admin"));
  await page.getByRole("radio", { name: /^Agent/ }).check();
  await page.getByRole("button", { name: "Approve" }).click();

  await page.waitForURL(/127\.0\.0\.1:49999\/callback\?/);
  const exchange = await page.request.post(`${e2eEnv().apiUrl}/oauth/token`, {
    form: {
      grant_type: "authorization_code",
      client_id: app.clientId,
      code: new URL(page.url()).searchParams.get("code")!,
      redirect_uri: CALLBACK,
      code_verifier: verifier,
    },
  });
  expect((await exchange.json()).scope).toBe("agent");
});

test("someone who is not signed in signs in and comes back to the request", async ({ page }) => {
  const app = await registerApp(page.request);
  const resource = await issuer(page.request);

  await page.goto(authorizeUrl(app.clientId, resource, pkce().challenge, "agent"));
  await expect(page).toHaveURL(/\/login\?redirect=/);
  await page.getByLabel("Password").fill(ADMIN_PASSWORD);
  await page.getByRole("button", { name: "Sign in" }).click();

  await expect(page.getByRole("heading", { name: `Let ${app.name} use bia?` })).toBeVisible();
});

test("a request that was already answered says so", async ({ signedInPage: page }) => {
  const app = await registerApp(page.request);
  const resource = await issuer(page.request);
  await catchCallback(page);

  await page.goto(authorizeUrl(app.clientId, resource, pkce().challenge, "agent"));
  const consent = page.url();
  await page.getByRole("button", { name: "Approve" }).click();
  await page.waitForURL(/127\.0\.0\.1:49999\/callback\?/);
  await page.goto(consent);

  await expect(page.getByRole("alert")).toContainText("already answered");
});
