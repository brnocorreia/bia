import { e2eEnv } from "./support/env";
import { expect, sessionCookies, signIn, test } from "./support/fixtures";

const SESSION_COOKIE = "bia_session";

test("signing in with the password shows the shell", async ({ page }) => {
  await signIn(page);

  await expect(page).toHaveURL(/\/workflows$/);
  const nav = page.getByRole("navigation", { name: "Main" });
  for (const label of ["Workflows", "Runs", "Secrets", "Tokens"]) {
    await expect(nav.getByRole("link", { name: label })).toBeVisible();
  }
  await expect(page.getByRole("heading", { name: "Workflows" })).toBeVisible();
});

test("a wrong password shows an error and stays on the sign-in screen", async ({ page }) => {
  await signIn(page, "not the admin password");

  await expect(page.getByRole("alert")).toHaveText("That password is not right.");
  await expect(page).toHaveURL(/\/login$/);
});

test("the Session survives a reload, signing out ends it, and no script can read it", async ({
  page,
  context,
}) => {
  await signIn(page);
  await expect(page).toHaveURL(/\/workflows$/);

  const cookie = (await context.cookies()).find((c) => c.name === SESSION_COOKIE)!;
  expect(cookie.httpOnly).toBe(true);
  expect(cookie.sameSite).toBe("Lax");
  const reachable = await page.evaluate(() =>
    JSON.stringify({
      cookie: document.cookie,
      local: { ...window.localStorage },
      session: { ...window.sessionStorage },
    }),
  );
  expect(reachable).not.toContain(cookie.value);
  expect(reachable).not.toContain("bia_");

  await page.reload();
  await expect(page).toHaveURL(/\/workflows$/);
  await expect(page.getByRole("heading", { name: "Workflows" })).toBeVisible();

  await page.getByText("Account").click();
  await page.getByRole("button", { name: "Sign out" }).click();
  await expect(page).toHaveURL(/\/login$/);
  await page.goto("/workflows");
  await expect(page).toHaveURL(/\/login$/);
  // Sign-out revoked the Session itself, not only the browser's copy of the cookie.
  const stale = await page.request.get(`${e2eEnv().apiUrl}/v1/tokens/me`, {
    headers: { Cookie: `${SESSION_COOKIE}=${cookie.value}` },
  });
  expect(stale.status()).toBe(401);
});

test("a screen without a session redirects to sign-in", async ({ page }) => {
  await page.goto("/runs");
  await expect(page).toHaveURL(/\/login$/);
});

test("a Session the server revoked returns to sign-in", async ({ page, api }) => {
  await signIn(page);
  await expect(page).toHaveURL(/\/workflows$/);
  const me = await page.request.get(`${e2eEnv().apiUrl}/v1/tokens/me`);
  const { id } = (await me.json()) as { id: string };

  const revoked = await api.POST("/v1/tokens/{token_id}/revoke", {
    params: { path: { token_id: id } },
  });
  expect(revoked.response.ok).toBe(true);
  await page.reload();

  await expect(page).toHaveURL(/\/login$/);
});

test("a Session cookie alone cannot make an unsafe request from another origin", async () => {
  const cookies = await sessionCookies();
  const value = cookies.find((c) => c.name === SESSION_COOKIE)!.value;

  const forged = await fetch(`${e2eEnv().apiUrl}/v1/tokens`, {
    method: "POST",
    headers: {
      Cookie: `${SESSION_COOKIE}=${value}`,
      Origin: "https://evil.example.com",
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ name: "forged", scope: "admin" }),
  });

  expect(forged.status).toBe(403);
  expect(((await forged.json()) as { code: string }).code).toBe("origin_mismatch");
});

test("config.json decides which API the UI talks to, with no rebuild", async ({ page }) => {
  const unreachable = "http://127.0.0.1:1";
  await page.route("**/config.json", (route) => route.fulfill({ json: { apiUrl: unreachable } }));
  await signIn(page);

  await expect(page.getByRole("alert")).toContainText(`Cannot reach the API at ${unreachable}`);
});

test("the served config.json points at the test API", async ({ page }) => {
  test.skip(e2eEnv().embedded, "the embedded UI uses its own origin; see embedded.spec.ts");
  const config = await page.request.get("/config.json");
  expect(await config.json()).toEqual({ apiUrl: e2eEnv().apiUrl });
});
