// Changing the password signs every other Session out and moves what signing in needs, so
// this runs after the other specs (see `playwright.config.ts`) instead of beside them. Signing
// out other Sessions ends them too, so the Access tests live here: one file runs in one worker.
import { e2eEnv, ADMIN_PASSWORD } from "../support/env";
import { expect, sessionCookies, test } from "../support/fixtures";

test.describe.configure({ mode: "serial" });

const REPLACEMENT = `${ADMIN_PASSWORD} again`;

const me = (cookie: string) =>
  fetch(`${e2eEnv().apiUrl}/v1/tokens/me`, { headers: { Cookie: `bia_session=${cookie}` } });

async function changePassword(
  page: import("@playwright/test").Page,
  current: string,
  replacement: string,
) {
  await page.getByText("Account").click();
  await page.getByRole("button", { name: "Change password" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Current password").fill(current);
  await dialog.getByLabel("New password", { exact: true }).fill(replacement);
  await dialog.getByLabel("New password again").fill(replacement);
  await dialog.getByRole("button", { name: "Change password" }).click();
}

test("changing the password keeps this Session, signs the others out and moves the password", async ({
  signedInPage: page,
  context,
}) => {
  const otherCookie = (await sessionCookies()).find((c) => c.name === "bia_session")!;
  expect((await me(otherCookie.value)).status).toBe(200);

  await page.goto("/workflows");
  await changePassword(page, `${ADMIN_PASSWORD} wrong`, REPLACEMENT);
  await expect(page.getByRole("alert")).toContainText("The current password is not right.");
  await page.getByRole("dialog").getByRole("button", { name: "Cancel" }).click();

  await changePassword(page, ADMIN_PASSWORD, REPLACEMENT);
  await expect(page.getByRole("dialog")).toContainText("Password changed.");
  await page.getByRole("dialog").getByRole("button", { name: "Done" }).click();

  expect((await me(otherCookie.value)).status).toBe(401);
  await page.reload();
  await expect(page.getByRole("heading", { name: "Workflows" })).toBeVisible();
  expect((await context.cookies()).some((c) => c.name === "bia_session")).toBe(true);

  // Put it back for whatever runs next.
  await changePassword(page, REPLACEMENT, ADMIN_PASSWORD);
  await expect(page.getByRole("dialog")).toContainText("Password changed.");
});

test("the Access screen marks this device and signs the other Sessions out", async ({
  signedInPage: page,
}) => {
  const other = (await sessionCookies()).find((c) => c.name === "bia_session")!;
  expect((await me(other.value)).status).toBe(200);

  await page.goto("/access");
  const sessions = page.getByRole("region", { name: "Sessions" });
  await expect(sessions.getByText("This device")).toHaveCount(1);
  await expect(page.getByRole("region", { name: "Connected apps" })).toBeVisible();
  await expect(page.getByRole("region", { name: "Tokens" })).toBeVisible();

  await sessions.getByRole("button", { name: "Sign out other sessions" }).click();

  await expect(sessions.getByRole("button", { name: "Sign out other sessions" })).toBeDisabled();
  await expect.poll(async () => (await me(other.value)).status).toBe(401);
  await page.reload();
  await expect(page.getByRole("heading", { name: "Access" })).toBeVisible();
});

test("revoking this device's own Session signs it out", async ({ signedInPage: page }) => {
  await page.goto("/access");
  const sessions = page.getByRole("region", { name: "Sessions" });
  const own = sessions.getByRole("row").filter({ hasText: "This device" });

  await own.getByRole("button", { name: /^Revoke session/ }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("you will be signed out");
  await dialog.getByRole("button", { name: "Revoke", exact: true }).click();

  await expect(page).toHaveURL(/\/login$/);
});
