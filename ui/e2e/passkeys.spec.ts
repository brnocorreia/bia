import type { Page } from "@playwright/test";
import { expect, test, uniqueName } from "./support/fixtures";
import { authorizeUrl, catchCallback, issuer, pkce, registerApp } from "./support/oauth";

// One User means one set of passkeys, so these tests must not overlap.
test.describe.configure({ mode: "serial" });

// A failed attempt can leave its passkey behind, and the retry would then never see an empty list.
test.beforeEach(async ({ api }) => {
  const { data } = await api.GET("/v1/passkeys");
  for (const passkey of data?.passkeys ?? []) {
    await api.DELETE("/v1/passkeys/{passkey_id}", { params: { path: { passkey_id: passkey.id } } });
  }
});

/** A CDP virtual authenticator, which answers the browser's WebAuthn prompts without a person. */
async function addVirtualAuthenticator(page: Page) {
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("WebAuthn.enable");
  await cdp.send("WebAuthn.addVirtualAuthenticator", {
    options: {
      protocol: "ctap2",
      transport: "internal",
      hasResidentKey: true,
      hasUserVerification: true,
      isUserVerified: true,
      automaticPresenceSimulation: true,
    },
  });
}

async function openPasskeys(page: Page) {
  await page.getByText("Account").click();
  await page.getByRole("button", { name: "Passkeys" }).click();
  return page.getByRole("dialog", { name: "Passkeys" });
}

test("a passkey is registered, renamed, signs in after signing out, and is removed", async ({
  signedInPage: page,
}) => {
  await addVirtualAuthenticator(page);

  let dialog = await openPasskeys(page);
  await expect(dialog.getByText("No passkeys yet.")).toBeVisible();
  await dialog.getByLabel("Name for a new passkey").fill("e2e laptop");
  await dialog.getByRole("button", { name: "Add passkey" }).click();
  await expect(dialog.getByText("e2e laptop")).toBeVisible();

  await dialog.getByRole("button", { name: "Rename e2e laptop" }).click();
  await dialog.getByLabel("New name for e2e laptop").fill("e2e phone");
  await dialog.getByRole("button", { name: "Save" }).click();
  await expect(dialog.getByText("e2e phone")).toBeVisible();
  await dialog.getByRole("button", { name: "Close" }).click();

  await page.getByText("Account").click();
  await page.getByRole("button", { name: "Sign out" }).click();
  await expect(page).toHaveURL(/\/login$/);
  await page.getByRole("button", { name: "Sign in with a passkey" }).click();
  await expect(page.getByRole("navigation", { name: "Main" })).toBeVisible();

  dialog = await openPasskeys(page);
  await expect(dialog.getByText(/last used/)).toBeVisible();
  await dialog.getByRole("button", { name: "Remove e2e phone" }).click();
  await dialog.getByRole("button", { name: "Confirm removing e2e phone" }).click();
  await expect(dialog.getByText("No passkeys yet.")).toBeVisible();
});

test("on a page that is not a secure context, the sign-in screen and the menu explain why", async ({
  signedInPage: page,
}) => {
  await page.addInitScript(() => {
    Object.defineProperty(window, "isSecureContext", { value: false });
  });
  await page.reload();

  const dialog = await openPasskeys(page);
  await expect(dialog.getByRole("note")).toContainText("HTTPS");
  await expect(dialog.getByLabel("Name for a new passkey")).toHaveCount(0);
  await dialog.getByRole("button", { name: "Close" }).click();

  await page.getByText("Account").click();
  await page.getByRole("button", { name: "Sign out" }).click();
  await expect(page.getByRole("note")).toContainText("HTTPS");
  await expect(page.getByRole("button", { name: "Sign in with a passkey" })).toHaveCount(0);
});

test("someone who signs in with a passkey comes back to the request as well", async ({
  signedInPage: page,
}) => {
  await addVirtualAuthenticator(page);
  const passkeyName = uniqueName("e2e-return");
  let dialog = await openPasskeys(page);
  await dialog.getByLabel("Name for a new passkey").fill(passkeyName);
  await dialog.getByRole("button", { name: "Add passkey" }).click();
  await expect(dialog.getByText(passkeyName)).toBeVisible();
  await dialog.getByRole("button", { name: "Close" }).click();
  await page.getByText("Account").click();
  await page.getByRole("button", { name: "Sign out" }).click();
  await expect(page).toHaveURL(/\/login$/);

  const app = await registerApp(page.request);
  const resource = await issuer(page.request);
  await catchCallback(page);
  await page.goto(authorizeUrl(app.clientId, resource, pkce().challenge, "agent"));
  await expect(page).toHaveURL(/\/login\?redirect=/);
  await page.getByRole("button", { name: "Sign in with a passkey" }).click();
  await expect(page.getByRole("heading", { name: `Let ${app.name} use bia?` })).toBeVisible();

  // Leave no passkey behind for the specs that expect none.
  await page.getByRole("button", { name: "Deny" }).click();
  // Navigating away before the app's callback loads would abort that navigation.
  await page.waitForURL(/127\.0\.0\.1:49999\/callback\?/);
  await page.goto("/");
  dialog = await openPasskeys(page);
  await dialog.getByRole("button", { name: `Remove ${passkeyName}` }).click();
  await dialog.getByRole("button", { name: `Confirm removing ${passkeyName}` }).click();
  await expect(dialog.getByText("No passkeys yet.")).toBeVisible();
});
