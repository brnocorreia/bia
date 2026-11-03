import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { ADMIN_PASSWORD, e2eEnv } from "./support/env";

// One test, since setup works once per database and every later step needs the one before.
test("the Setup Link sets the admin password once and signs the admin in", async ({ page }) => {
  const link = process.env.BIA_E2E_SETUP_LINK;
  if (!link) throw new Error("global setup did not provide BIA_E2E_SETUP_LINK");
  const { pathname, search } = new URL(link);
  const code = new URLSearchParams(search).get("code")!;
  const password = page.getByLabel("Password", { exact: true });
  const again = page.getByLabel("Password again");
  const submit = page.getByRole("button", { name: "Set password" });

  const [start] = code.split("-");
  await page.goto(`/setup?code=${start}-${"0".repeat(32)}`);
  await password.fill(ADMIN_PASSWORD);
  await again.fill(ADMIN_PASSWORD);
  await submit.click();
  await expect(page.getByRole("alert")).toContainText("code is wrong");

  await page.goto(`${pathname}${search}`);
  await expect(page.getByLabel("Setup code")).toHaveCount(0);
  const tooShort = ADMIN_PASSWORD.slice(0, 5);
  await password.fill(tooShort);
  await again.fill(tooShort);
  await expect(submit).toBeDisabled();
  await password.fill(ADMIN_PASSWORD);
  await again.fill(ADMIN_PASSWORD.slice(0, -1));
  await expect(page.getByText("The passwords differ.")).toBeVisible();
  await again.fill(ADMIN_PASSWORD);
  await submit.click();

  // Setup ends by signing in: the Session cookie it started opens the shell.
  await expect(page).toHaveURL(/\/workflows$/);
  await expect(page.getByRole("navigation", { name: "Main" })).toBeVisible();

  // Seeding data goes through the API with a manual Token, minted through the Session the
  // page holds. The `Origin` is the UI's, as a browser at that address would send.
  const minted = await page.request.post(`${e2eEnv().apiUrl}/v1/tokens`, {
    data: { name: "e2e", scope: "admin" },
    headers: { Origin: e2eEnv().uiUrl },
  });
  expect(minted.ok()).toBe(true);
  const token = ((await minted.json()) as { value: string }).value;
  expect(token).toMatch(/^bia_[0-9a-f]{64}$/);
  const file = e2eEnv().adminTokenFile;
  mkdirSync(path.dirname(file), { recursive: true });
  writeFileSync(file, token);

  const other = ADMIN_PASSWORD.split("").reverse().join("");
  await page.goto(`${pathname}${search}`);
  await password.fill(other);
  await again.fill(other);
  await submit.click();
  await expect(page.getByRole("alert")).toContainText("already set up");
});
