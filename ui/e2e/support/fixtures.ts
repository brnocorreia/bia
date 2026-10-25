import { test as base, expect, type Page } from "@playwright/test";
import createClient from "openapi-fetch";
import type { paths } from "../../src/api/schema.gen";
import { e2eEnv } from "./env";

/** The key `ui/src/auth/session.ts` keeps the Token under. */
const SESSION_KEY = "bia.token";

export type SeedApi = ReturnType<typeof createClient<paths>>;

interface Fixtures {
  /** Typed API client for seeding and inspecting data, authenticated as the admin Token. */
  api: SeedApi;
  adminToken: string;
  /** A page that is already signed in as the admin Token, so a test starts inside the shell. */
  signedInPage: Page;
}

export function apiClient(token: string): SeedApi {
  return createClient<paths>({
    baseUrl: e2eEnv().apiUrl,
    headers: { Authorization: `Bearer ${token}` },
  });
}

/** Signs in through the login form, as a user does. */
export async function signIn(page: Page, token: string) {
  await page.goto("/login");
  await page.getByLabel("Token").fill(token);
  await page.getByRole("button", { name: "Sign in" }).click();
}

/** Puts a Token in the page's session before any script runs, skipping the form. */
export async function startSessionWith(page: Page, token: string) {
  await page.addInitScript(
    ([key, value]) => window.sessionStorage.setItem(key as string, value as string),
    [SESSION_KEY, token],
  );
}

/** A name no other test (or earlier run, on a reused database) has used. */
export function uniqueName(prefix: string) {
  return `${prefix}-${Math.random().toString(36).slice(2, 10)}`;
}

export const test = base.extend<Fixtures>({
  adminToken: async ({}, use) => {
    const token = process.env.BIA_E2E_ADMIN_TOKEN;
    if (!token) throw new Error("global setup did not provide BIA_E2E_ADMIN_TOKEN");
    await use(token);
  },
  api: async ({ adminToken }, use) => {
    await use(apiClient(adminToken));
  },
  signedInPage: async ({ page, adminToken }, use) => {
    await startSessionWith(page, adminToken);
    await page.goto("/");
    await use(page);
  },
});

export { expect };
