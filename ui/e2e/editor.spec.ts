import type { Page } from "@playwright/test";
import { expect, test, uniqueName } from "./support/fixtures";

const definitionYaml = (id: string, action = "flow.sleep") => `schema_version: 1
id: ${id}
description: Written in the editor
triggers:
  - id: by_hand
    type: manual
steps:
  - id: pause
    action: ${action}
    params:
      duration: 1s
`;

async function setEditorText(page: Page, text: string) {
  // Close any suggestion widget and click clear of it: a click on the widget would pick a suggestion.
  await page.keyboard.press("Escape");
  await page
    .locator(".monaco-editor")
    .first()
    .click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("ControlOrMeta+A");
  // Pasting keeps the text as written; typing it would let the editor auto-indent each line.
  await page
    .locator(".monaco-editor textarea")
    .first()
    .evaluate((el, pasted) => {
      const clipboardData = new DataTransfer();
      clipboardData.setData("text/plain", pasted);
      el.dispatchEvent(
        new ClipboardEvent("paste", { clipboardData, bubbles: true, cancelable: true }),
      );
    }, text);
}

/** The editor's visible text, with Monaco's non-breaking spaces turned back into spaces. */
async function editorText(page: Page) {
  const text = await page.locator(".monaco-editor .view-lines").first().innerText();
  return text.replaceAll(" ", " ");
}

async function openNew(page: Page) {
  await page.goto("/workflows/new");
  await expect(page.locator(".monaco-editor").first()).toBeVisible();
}

test("a typo in an Action name is underlined with the server's suggestion", async ({
  signedInPage: page,
}) => {
  await openNew(page);
  await setEditorText(page, definitionYaml(uniqueName("typo"), "flow.slep"));

  const diagnostics = page.getByRole("list", { name: "Diagnostics" });
  await expect(diagnostics).toContainText("unknown_action");
  await expect(diagnostics).toContainText("/steps/0/action");
  await expect(diagnostics).toContainText("flow.sleep");
  await expect(page.locator(".monaco-editor .squiggly-error").first()).toBeVisible();

  // The squiggle sits on the misspelled name: hovering it shows the Diagnostic.
  await page
    .locator(".monaco-editor .view-line", { hasText: "flow.slep" })
    .first()
    .hover({
      position: { x: 150, y: 8 },
    });
  await expect(page.locator(".monaco-hover:not(.hidden)")).toContainText("flow.sleep");
  await expect(page.getByRole("status", { name: "Validation" })).toContainText("1 error");
});

test("a fixed Definition clears the Diagnostics", async ({ signedInPage: page }) => {
  await openNew(page);
  await setEditorText(page, definitionYaml(uniqueName("fixed"), "flow.slep"));
  await expect(page.getByRole("list", { name: "Diagnostics" })).toBeVisible();

  await setEditorText(page, definitionYaml(uniqueName("fixed")));
  await expect(page.getByRole("status", { name: "Validation" })).toContainText("No errors");
  await expect(page.getByRole("list", { name: "Diagnostics" })).toHaveCount(0);
});

/** Asks for suggestions until the language worker has loaded and answers from the schema. */
async function suggestionsContaining(page: Page, ...expected: string[]) {
  await expect(async () => {
    await page.keyboard.press("Control+Space");
    for (const label of expected) {
      await expect(page.locator(".suggest-widget")).toContainText(label, { timeout: 1500 });
    }
  }).toPass({ timeout: 15_000 });
}

test("completion proposes Trigger types and Step fields from the schema", async ({
  signedInPage: page,
}) => {
  await openNew(page);
  // The pasted text ends where completion is asked for, which is where the cursor is left.
  await setEditorText(page, "schema_version: 1\nid: completing\ntriggers:\n  - id: t\n    type: ");
  await suggestionsContaining(page, "cron", "webhook", "queue", "manual");
  await page.keyboard.press("Escape");

  await setEditorText(page, "schema_version: 1\nid: completing\nsteps:\n  - id: a\n    ");
  await suggestionsContaining(page, "action");

  // The list is long and scrolls, so narrow it to see a field further down.
  await setEditorText(page, "schema_version: 1\nid: completing\nsteps:\n  - id: a\n    par");
  await suggestionsContaining(page, "params");
});

test("switching between JSON and YAML keeps the Definition", async ({ signedInPage: page }) => {
  await openNew(page);
  const id = uniqueName("switch");
  await setEditorText(page, definitionYaml(id));

  await page.getByRole("button", { name: "JSON" }).click();
  await expect(page.getByRole("button", { name: "JSON" })).toHaveAttribute("aria-pressed", "true");
  await expect.poll(() => editorText(page)).toContain(`"id": "${id}"`);
  await expect.poll(() => editorText(page)).toContain('"action": "flow.sleep"');

  await page.getByRole("button", { name: "YAML" }).click();
  await expect.poll(() => editorText(page)).toContain(`id: ${id}`);
  await expect.poll(() => editorText(page)).toContain("action: flow.sleep");
});

test("YAML comments are warned about", async ({ signedInPage: page }) => {
  await openNew(page);
  await setEditorText(page, `# my note\n${definitionYaml(uniqueName("comment"))}`);
  await expect(page.getByText("YAML comments are not kept")).toBeVisible();
});

test("saving creates a Draft, replacing keeps its number, and reopening shows the content", async ({
  signedInPage: page,
  api,
}) => {
  const id = uniqueName("saved");
  await openNew(page);
  await setEditorText(page, definitionYaml(id));
  await page.getByRole("button", { name: "Save as new Draft" }).click();

  await expect(page.getByRole("status").filter({ hasText: "Saved as Draft v1" })).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`/workflows/${id}/edit\\?v=1`));

  const first = await api.GET("/v1/workflows/{workflow_id}/versions/{number}", {
    params: { path: { workflow_id: id, number: 1 } },
  });
  expect(first.data?.definition).toMatchObject({ id, description: "Written in the editor" });

  await setEditorText(page, definitionYaml(id).replace("1s", "9s"));
  await page.getByRole("button", { name: "Replace Draft v1" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Replaced Draft v1" })).toBeVisible();
  const versions = await api.GET("/v1/workflows/{workflow_id}/versions", {
    params: { path: { workflow_id: id } },
  });
  expect(versions.data?.versions).toHaveLength(1);

  await page.getByRole("button", { name: "Save as new Draft" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Saved as Draft v2" })).toBeVisible();

  await page.goto(`/workflows/${id}/edit?v=1`);
  await expect(page.locator(".monaco-editor").first()).toBeVisible();
  await expect.poll(() => editorText(page)).toContain("duration: 9s");
});

test("a Draft with errors can still be saved and shows its Diagnostics", async ({
  signedInPage: page,
}) => {
  await openNew(page);
  await setEditorText(page, definitionYaml(uniqueName("broken"), "flow.slep"));
  await page.getByRole("button", { name: "Save as new Draft" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Saved as Draft v1" })).toBeVisible();
  await expect(page.getByRole("list", { name: "Diagnostics" })).toContainText("unknown_action");
});

test("leaving with unsaved changes asks first", async ({ signedInPage: page }) => {
  await openNew(page);
  await setEditorText(page, definitionYaml(uniqueName("guard")));

  await page.getByRole("link", { name: "Workflows" }).first().click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("Discard unsaved changes?");
  await dialog.getByRole("button", { name: "Cancel" }).click();
  await expect(page).toHaveURL(/\/workflows\/new/);

  await page.getByRole("link", { name: "Workflows" }).first().click();
  await page.getByRole("dialog").getByRole("button", { name: "Discard changes" }).click();
  await expect(page).toHaveURL(/\/workflows$/);
});

test("the editor starts from a published Version and saves a new Draft", async ({
  signedInPage: page,
  api,
}) => {
  const id = uniqueName("fork");
  const created = await api.POST("/v1/workflows/{workflow_id}/versions", {
    params: { path: { workflow_id: id } },
    body: { schema_version: 1, id, steps: [] } as never,
  });
  expect(created.response.status).toBe(201);
  await api.POST("/v1/workflows/{workflow_id}/versions/{number}/publish", {
    params: { path: { workflow_id: id, number: 1 } },
  });

  await page.goto(`/workflows/${id}`);
  await page.getByRole("row", { name: "Version 1" }).getByRole("link", { name: "Edit" }).click();
  await expect(page.locator(".monaco-editor").first()).toBeVisible();
  await expect(page.getByRole("button", { name: "Replace Draft v1" })).toHaveCount(0);
  await page.getByRole("button", { name: "Save as new Draft" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Saved as Draft v2" })).toBeVisible();
});
