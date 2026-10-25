import { e2eEnv } from "./support/env";
import { expect, signIn, startSessionWith, test, uniqueName } from "./support/fixtures";

test("signing in with a valid Token shows the shell", async ({ page, adminToken }) => {
  await signIn(page, adminToken);

  await expect(page).toHaveURL(/\/workflows$/);
  const nav = page.getByRole("navigation", { name: "Main" });
  for (const label of ["Workflows", "Runs", "Secrets", "Tokens"]) {
    await expect(nav.getByRole("link", { name: label })).toBeVisible();
  }
  await expect(page.getByRole("heading", { name: "Workflows" })).toBeVisible();
});

test("an invalid Token shows an error and stays on the login screen", async ({ page }) => {
  await signIn(page, "bia_" + "0".repeat(64));

  await expect(page.getByRole("alert")).toHaveText("That Token is not valid.");
  await expect(page).toHaveURL(/\/login$/);
});

test("the Token is kept for the tab only and signing out forgets it", async ({
  page,
  adminToken,
}) => {
  await signIn(page, adminToken);
  await expect(page).toHaveURL(/\/workflows$/);

  expect(await page.evaluate(() => JSON.stringify(window.localStorage))).not.toContain("bia_");
  await page.reload();
  await expect(page).toHaveURL(/\/workflows$/);

  await page.getByRole("button", { name: "Sign out" }).click();
  await expect(page).toHaveURL(/\/login$/);
  await page.goto("/workflows");
  await expect(page).toHaveURL(/\/login$/);
});

test("a screen without a session redirects to login", async ({ page }) => {
  await page.goto("/runs");
  await expect(page).toHaveURL(/\/login$/);
});

test("a 401 after signing in returns to the login screen", async ({ page, api }) => {
  const created = await api.POST("/v1/tokens", {
    body: { name: uniqueName("short-lived"), scope: "agent" },
  });
  const { token, value } = created.data!;

  await signIn(page, value);
  await expect(page).toHaveURL(/\/workflows$/);

  const revoked = await api.POST("/v1/tokens/{token_id}/revoke", {
    params: { path: { token_id: token.id } },
  });
  expect(revoked.response.ok).toBe(true);
  await page.reload();

  await expect(page).toHaveURL(/\/login$/);
});

test("config.json decides which API the UI talks to, with no rebuild", async ({
  page,
  adminToken,
}) => {
  const unreachable = "http://127.0.0.1:1";
  await page.route("**/config.json", (route) => route.fulfill({ json: { apiUrl: unreachable } }));
  await signIn(page, adminToken);

  await expect(page.getByRole("alert")).toContainText(`Cannot reach the API at ${unreachable}`);
});

test("the served config.json points at the test API", async ({ page, adminToken }) => {
  await startSessionWith(page, adminToken);
  const config = await page.request.get("/config.json");
  expect(await config.json()).toEqual({ apiUrl: e2eEnv().apiUrl });
});
