import type { Page } from "@playwright/test";
import { expect, test, uniqueName } from "./support/fixtures";

const definitionYaml = (id: string, fetchAction = "http.request") => `schema_version: 1
id: ${id}
triggers:
  - id: by_hand
    type: manual
steps:
  - id: double
    action: transform.expr
    params:
      expression: values.n * 2
      values:
        n: "{{ input.n }}"
  - id: fetch
    action: ${fetchAction}
    params:
      method: GET
      url: https://example.com/orders
    retry:
      max_attempts: 3
      backoff: fixed
      initial: 1h
`;

async function setEditorText(page: Page, text: string) {
  // Close any suggestion widget and click clear of it: a click on the widget would pick a suggestion.
  await page.keyboard.press("Escape");
  await page
    .locator(".monaco-editor")
    .first()
    .click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("ControlOrMeta+A");
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

async function openEditor(page: Page, yaml: string) {
  await page.goto("/workflows/new");
  await expect(page.locator(".monaco-editor").first()).toBeVisible();
  await setEditorText(page, yaml);
  await expect(page.getByRole("status", { name: "Validation" })).toBeVisible();
}

async function addMock(page: Page, option: string, stepId: string, mock: unknown) {
  const panel = page.getByRole("region", { name: "Dry Run" });
  await panel.getByLabel("Step to Mock").selectOption({ label: option });
  await panel.getByRole("button", { name: "Add Mock" }).click();
  await panel.getByLabel(`Mock for ${stepId}`).fill(JSON.stringify(mock));
}

test("a Dry Run of the edited Definition shows the real transform Output and the mocked HTTP Output", async ({
  signedInPage: page,
}) => {
  await openEditor(page, definitionYaml(uniqueName("dry")));
  const panel = page.getByRole("region", { name: "Dry Run" });
  await panel.getByLabel("Input (JSON)").fill('{"n": 21}');
  await addMock(page, "fetch (http.request)", "fetch", {
    output: { status: 200, body: "mocked-body" },
  });
  await panel.getByRole("button", { name: "Run Dry Run" }).click();

  const timeline = panel.getByRole("region", { name: "Timeline" });
  await expect(timeline.getByText("dry run", { exact: true })).toBeVisible();
  await expect(timeline.getByText("Not a real Run")).toBeVisible();

  await timeline.getByRole("listitem", { name: "Step double" }).getByText("Output").click();
  await expect(timeline.getByLabel("Output", { exact: true })).toContainText("42");
  await timeline.getByRole("listitem", { name: "Step fetch" }).getByText("Output").click();
  await expect(timeline.getByLabel("Output", { exact: true }).last()).toContainText("mocked-body");
});

test("a Mock error follows the retry path and fails the Dry Run", async ({
  signedInPage: page,
}) => {
  await openEditor(page, definitionYaml(uniqueName("dry")));
  const panel = page.getByRole("region", { name: "Dry Run" });
  await panel.getByLabel("Input (JSON)").fill('{"n": 1}');
  await addMock(page, "fetch (http.request)", "fetch", {
    error: { class: "retryable", message: "upstream down" },
  });
  await panel.getByRole("button", { name: "Run Dry Run" }).click();

  const attempts = panel.getByRole("list", { name: "Attempts of fetch" });
  await expect(attempts.getByRole("listitem")).toHaveCount(3);
  await expect(attempts.getByText("upstream down")).toHaveCount(3);
  await expect(panel.getByText("Dry Run result")).toBeVisible();
});

test("an invalid Definition shows Diagnostics instead of a Timeline", async ({
  signedInPage: page,
}) => {
  await openEditor(page, definitionYaml(uniqueName("dry"), "http.requset"));
  const panel = page.getByRole("region", { name: "Dry Run" });
  await panel.getByRole("button", { name: "Run Dry Run" }).click();

  await expect(panel.getByText("does not validate")).toBeVisible();
  await expect(panel.getByRole("list", { name: "Diagnostics" })).toContainText("unknown_action");
  await expect(panel.getByRole("region", { name: "Timeline" })).toHaveCount(0);
});

test("the input and Mocks are remembered per Workflow after a reload", async ({
  signedInPage: page,
}) => {
  await openEditor(page, definitionYaml(uniqueName("dry")));
  const panel = page.getByRole("region", { name: "Dry Run" });
  await panel.getByLabel("Input (JSON)").fill('{"n": 5}');
  await addMock(page, "double (transform.expr)", "double", { output: 10 });

  await page.reload();
  await expect(
    page.getByRole("region", { name: "Dry Run" }).getByLabel("Input (JSON)"),
  ).toHaveValue('{"n": 5}');
  await expect(page.getByLabel("Mock for double")).toHaveValue('{"output":10}');
});
