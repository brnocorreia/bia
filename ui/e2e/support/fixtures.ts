import { readFileSync } from "node:fs";
import { test as base, expect, request, type Cookie, type Page } from "@playwright/test";
import createClient from "openapi-fetch";
import type { paths } from "../../src/api/schema.gen";
import { ADMIN_PASSWORD, e2eEnv } from "./env";

export type SeedApi = ReturnType<typeof createClient<paths>>;

interface Fixtures {
  /** Typed API client for seeding and inspecting data, authenticated as a manual admin Token. */
  api: SeedApi;
  adminToken: string;
  /** A page that is already signed in as the admin, so a test starts inside the shell. */
  signedInPage: Page;
}

export function apiClient(token: string): SeedApi {
  return createClient<paths>({
    baseUrl: e2eEnv().apiUrl,
    headers: { Authorization: `Bearer ${token}` },
  });
}

/** Signs in through the sign-in form, as a user does. */
export async function signIn(page: Page, password = ADMIN_PASSWORD) {
  await page.goto("/login");
  await page.getByLabel("Password").fill(password);
  await page.getByRole("button", { name: "Sign in" }).click();
}

/** The Session cookie of a fresh sign-in through the API, to put in a browser context. */
export async function sessionCookies(password = ADMIN_PASSWORD): Promise<Cookie[]> {
  const client = await request.newContext();
  try {
    const response = await client.post(`${e2eEnv().apiUrl}/v1/session`, { data: { password } });
    if (!response.ok()) throw new Error(`sign-in failed: ${response.status()}`);
    return (await client.storageState()).cookies;
  } finally {
    await client.dispose();
  }
}

/**
 * Makes the page believe it is an `agent`: the UI learns its Scope from `GET /v1/tokens/me`,
 * and a Session is always `admin`, so this stubs that one answer to test how the screens
 * treat a Scope that cannot administer. The server still enforces Scopes for real.
 */
export async function actAsAgent(page: Page) {
  await page.route("**/v1/tokens/me", async (route) => {
    const response = await route.fetch();
    await route.fulfill({ response, json: { ...(await response.json()), scope: "agent" } });
  });
}

/** A name no other test (or earlier run, on a reused database) has used. */
export function uniqueName(prefix: string) {
  return `${prefix}-${Math.random().toString(36).slice(2, 10)}`;
}

export const test = base.extend<Fixtures>({
  adminToken: async ({}, use) => {
    await use(readFileSync(e2eEnv().adminTokenFile, "utf8").trim());
  },
  api: async ({ adminToken }, use) => {
    await use(apiClient(adminToken));
  },
  signedInPage: async ({ page, context }, use) => {
    await context.addCookies(await sessionCookies());
    await page.goto("/");
    await use(page);
  },
});

export { expect };
