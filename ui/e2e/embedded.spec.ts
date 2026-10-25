import { e2eEnv } from "./support/env";
import { expect, test } from "./support/fixtures";

// Runs only against the UI embedded in `bia` (`pnpm test:e2e:embedded`).
test.skip(!e2eEnv().embedded, "needs the UI embedded in bia");

test("a signed-in deep link survives a reload", async ({ signedInPage: page }) => {
  await page.goto("/runs");
  await page.reload();

  await expect(page).toHaveURL(/\/runs$/);
  await expect(page.getByRole("navigation", { name: "Main" })).toBeVisible();
});

test("the app is configured for the server it was served from", async ({ page }) => {
  const response = await page.request.get("/config.json");

  expect(await response.json()).toEqual({ apiUrl: "" });
});
