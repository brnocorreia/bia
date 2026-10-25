import type { Page } from "@playwright/test";
import { expect, startSessionWith, test, uniqueName } from "./support/fixtures";
import { e2eEnv } from "./support/env";

// Collects every response body the page receives, so a test can prove a value never came back.
function recordBodies(page: Page) {
  const bodies: Promise<string>[] = [];
  page.on("response", (response) => {
    if (response.url().startsWith(e2eEnv().apiUrl)) {
      bodies.push(response.text().catch(() => ""));
    }
  });
  return async () => (await Promise.all(bodies)).join("\n");
}

test("a Secret is created and replaced without its value appearing in the UI or any response", async ({
  signedInPage: page,
  api,
}) => {
  const name = uniqueName("SECRET").toUpperCase().replace(/-/g, "_");
  const first = `first-${uniqueName("value")}`;
  const second = `second-${uniqueName("value")}`;
  const bodies = recordBodies(page);

  await page.goto("/secrets");
  const form = page.locator("form");
  await form.getByLabel("Name").fill(name);
  const value = form.getByLabel("Value");
  await expect(value).toHaveAttribute("type", "password");
  await expect(value).not.toHaveAttribute("autocomplete", "on");
  await value.fill(first);
  await form.getByRole("button", { name: "Set Secret" }).click();

  const row = page.getByRole("row").filter({ hasText: name });
  await expect(row).toBeVisible();
  await expect(value).toHaveValue("");

  await form.getByLabel("Name").fill(name);
  await value.fill(second);
  await form.getByRole("button", { name: "Set Secret" }).click();
  await expect(page.getByText(`Saved ${name}`)).toBeVisible();
  await expect(value).toHaveValue("");

  await page.reload();
  await expect(page.getByRole("row").filter({ hasText: name })).toBeVisible();
  const listed = await api.GET("/v1/secrets");
  expect(listed.data?.secrets.some((s) => s.name === name)).toBe(true);

  const content = await page.content();
  const storage = await page.evaluate(() =>
    JSON.stringify([{ ...sessionStorage }, { ...localStorage }]),
  );
  const everything = [content, storage, await bodies(), JSON.stringify(listed.data)].join("\n");
  for (const secret of [first, second]) expect(everything).not.toContain(secret);
});

test("deleting a Secret asks first and removes it", async ({ signedInPage: page, api }) => {
  const name = uniqueName("DOOMED").toUpperCase().replace(/-/g, "_");
  await api.PUT("/v1/secrets/{name}", { params: { path: { name } }, body: { value: "x" } });

  await page.goto("/secrets");
  const row = page.getByRole("row").filter({ hasText: name });
  await row.getByRole("button", { name: `Delete ${name}` }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("button", { name: "Cancel" }).click();
  await expect(row).toBeVisible();

  await row.getByRole("button", { name: `Delete ${name}` }).click();
  await dialog.getByRole("button", { name: "Delete Secret" }).click();
  await expect(row).toHaveCount(0);
});

test("a new Token's value is shown once and cannot be recovered after a reload", async ({
  signedInPage: page,
}) => {
  const name = uniqueName("agent");
  await page.goto("/tokens");
  await page.getByLabel("Name").fill(name);
  await page.getByLabel("Scope").selectOption("agent");
  await page.getByRole("button", { name: "Create Token" }).click();

  const shown = page.getByTestId("new-token-value");
  await expect(shown).toBeVisible();
  const value = (await shown.textContent())!;
  expect(value.length).toBeGreaterThan(8);

  // The value works as a credential while it is on screen.
  const me = await fetch(`${e2eEnv().apiUrl}/v1/tokens/me`, {
    headers: { Authorization: `Bearer ${value}` },
  });
  expect(me.status).toBe(200);

  const row = page.getByRole("row").filter({ hasText: name });
  await expect(row).toContainText("agent");
  await expect(row).toContainText("Active");

  await page.reload();
  await expect(page.getByRole("row").filter({ hasText: name })).toBeVisible();
  expect(await page.content()).not.toContain(value);
  const stored = await page.evaluate(() =>
    JSON.stringify([{ ...sessionStorage }, { ...localStorage }]),
  );
  expect(stored).not.toContain(value);
});

test("a webhook Token is bound to a Workflow and Trigger", async ({ signedInPage: page, api }) => {
  const name = uniqueName("hook");
  await page.goto("/tokens");
  await page.getByLabel("Name").fill(name);
  await page.getByLabel("Scope").selectOption("webhook");
  await page.getByLabel("Workflow").fill("onboarding");
  await page.getByLabel("Trigger").fill("signup");
  await page.getByRole("button", { name: "Create Token" }).click();
  await expect(page.getByTestId("new-token-value")).toBeVisible();

  const listed = await api.GET("/v1/tokens");
  const created = listed.data!.tokens.find((t) => t.name === name)!;
  expect(created.scope).toBe("webhook");
  expect(created.webhook).toEqual({ workflow_id: "onboarding", trigger_id: "signup" });
});

test("a revoked Token fails authentication", async ({ signedInPage: page, api }) => {
  const name = uniqueName("short-lived");
  const made = await api.POST("/v1/tokens", { body: { name, scope: "agent" } });
  const value = made.data!.value;
  const authenticate = () =>
    fetch(`${e2eEnv().apiUrl}/v1/tokens/me`, { headers: { Authorization: `Bearer ${value}` } });
  expect((await authenticate()).status).toBe(200);

  await page.goto("/tokens");
  const row = page.getByRole("row").filter({ hasText: name });
  await row.getByRole("button", { name: `Revoke ${name}` }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Revoke Token" }).click();

  await expect(row).toContainText("Revoked");
  await expect(row.getByRole("button", { name: `Revoke ${name}` })).toBeDisabled();
  expect((await authenticate()).status).toBe(401);
});

test("a non-admin Token cannot use the Secrets and Tokens screens", async ({ page, api }) => {
  const made = await api.POST("/v1/tokens", {
    body: { name: uniqueName("viewer"), scope: "agent" },
  });
  await startSessionWith(page, made.data!.value);

  for (const path of ["/secrets", "/tokens"]) {
    await page.goto(path);
    await expect(page.getByRole("alert")).toContainText("needs an admin Token");
    await expect(page.getByRole("button", { name: /Set Secret|Create Token/ })).toHaveCount(0);
  }
});
