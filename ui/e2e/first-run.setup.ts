import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { e2eEnv } from "./support/env";

// One test, since setup works once per database and every later step needs the one before.
test("the Setup Link sets the admin password once and shows an admin Token", async ({ page }) => {
  const link = process.env.BIA_E2E_SETUP_LINK;
  if (!link) throw new Error("global setup did not provide BIA_E2E_SETUP_LINK");
  const { pathname, search } = new URL(link);
  const code = new URLSearchParams(search).get("code")!;
  const password = page.getByLabel("Password", { exact: true });
  const again = page.getByLabel("Password again");
  const submit = page.getByRole("button", { name: "Set password" });

  const [start] = code.split("-");
  await page.goto(`/setup?code=${start}-${"0".repeat(32)}`);
  await password.fill("correct horse battery");
  await again.fill("correct horse battery");
  await submit.click();
  await expect(page.getByRole("alert")).toContainText("code is wrong");

  await page.goto(`${pathname}${search}`);
  await expect(page.getByLabel("Setup code")).toHaveCount(0);
  await password.fill("too short");
  await again.fill("too short");
  await expect(submit).toBeDisabled();
  await password.fill("correct horse battery");
  await again.fill("correct horse batter");
  await expect(page.getByText("The passwords differ.")).toBeVisible();
  await again.fill("correct horse battery");
  await submit.click();

  await expect(page.getByRole("heading", { name: "Setup complete" })).toBeVisible();
  const token = (await page.getByTestId("setup-token-value").textContent())!.trim();
  expect(token).toMatch(/^bia_[0-9a-f]{64}$/);
  const file = e2eEnv().adminTokenFile;
  mkdirSync(path.dirname(file), { recursive: true });
  writeFileSync(file, token);

  await page.goto(`${pathname}${search}`);
  await password.fill("another password");
  await again.fill("another password");
  await submit.click();
  await expect(page.getByRole("alert")).toContainText("already set up");

  await page.goto("/login");
  await page.getByLabel("Token").fill(token);
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page).toHaveURL(/\/workflows$/);
});
