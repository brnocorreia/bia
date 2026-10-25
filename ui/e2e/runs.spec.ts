import type { Page } from "@playwright/test";
import { expect, test, uniqueName, type SeedApi } from "./support/fixtures";

type Step = { id: string; action: string; params: Record<string, unknown> };

const sleep = (id: string, duration: string): Step => ({
  id,
  action: "flow.sleep",
  params: { duration },
});
const fail = (id: string, message: string): Step => ({
  id,
  action: "flow.fail",
  params: { message },
});

/** Saves, publishes and activates a Workflow with a manual Trigger. */
async function deploy(api: SeedApi, id: string, steps: Step[]) {
  const created = await api.POST("/v1/workflows/{workflow_id}/versions", {
    params: { path: { workflow_id: id } },
    body: {
      schema_version: 1,
      id,
      triggers: [{ id: "by_hand", type: "manual" }],
      steps,
    } as never,
  });
  expect(created.response.status).toBe(201);
  const path = { workflow_id: id, number: created.data!.version.number };
  expect(
    (await api.POST("/v1/workflows/{workflow_id}/versions/{number}/publish", { params: { path } }))
      .response.ok,
  ).toBe(true);
  expect(
    (await api.POST("/v1/workflows/{workflow_id}/versions/{number}/activate", { params: { path } }))
      .response.ok,
  ).toBe(true);
}

async function trigger(api: SeedApi, id: string, input: unknown = {}) {
  const run = await api.POST("/v1/workflows/{workflow_id}/runs", {
    params: { path: { workflow_id: id } },
    body: { input },
  });
  expect(run.response.status).toBe(201);
  return run.data!.id;
}

/** Polls every 100 ms in this browser, so tests do not wait on the default interval. */
async function pollFast(page: Page) {
  await page.addInitScript(() => window.localStorage.setItem("bia.pollIntervalMs", "100"));
}

test("a Run triggered from the UI is watched to completion by polling", async ({
  signedInPage: page,
  api,
}) => {
  await pollFast(page);
  const id = uniqueName("trigger");
  await deploy(api, id, [sleep("pause", "1s"), sleep("again", "1s")]);

  await page.goto("/runs");
  await page.getByRole("button", { name: "Trigger a Run" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Workflow").selectOption({ label: `${id} (v1)` });
  await dialog.getByLabel("Input (JSON)").fill("{ not json");
  await dialog.getByRole("button", { name: "Trigger" }).click();
  await expect(dialog.getByRole("alert")).toContainText("not valid JSON");
  await dialog.getByLabel("Input (JSON)").fill('{"hello": "world"}');
  await dialog.getByRole("button", { name: "Trigger" }).click();

  await expect(page).toHaveURL(/\/runs\/[0-9a-f-]{36}$/);
  await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
  await expect(page.getByRole("status", { name: "Run status" })).toHaveText("succeeded", {
    timeout: 15_000,
  });
  await expect(page.getByLabel("Step again")).toContainText("succeeded");
  await expect(page.getByLabel("Step pause")).toContainText("succeeded");
  await expect(page.getByRole("button", { name: "Cancel", exact: true })).toBeDisabled();
});

test("a failed Run's Timeline shows each Attempt's error and duration; resume and retry work", async ({
  signedInPage: page,
  api,
}) => {
  await pollFast(page);
  const id = uniqueName("failing");
  await deploy(api, id, [sleep("first", "1s"), fail("explode", "kaboom")]);
  const runId = await trigger(api, id);

  await page.goto(`/runs/${runId}`);
  const step = page.getByLabel("Step explode");
  await expect(step).toContainText("failed", { timeout: 15_000 });
  const attempt = step.getByRole("listitem").filter({ hasText: "Attempt 1" });
  await expect(attempt.getByRole("alert")).toContainText("kaboom");
  await expect(attempt).toContainText(/\d+(ms|s)/);
  await expect(page.getByLabel("Step first")).toContainText("succeeded");

  await page.getByRole("button", { name: "Resume" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Resume" }).click();
  await expect(step).toContainText("Attempt 2", { timeout: 15_000 });

  await page.getByRole("button", { name: "Retry" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Retry" }).click();
  await page.getByRole("link", { name: "Open the new Run" }).click();
  await expect(page).not.toHaveURL(new RegExp(`${runId}$`));
  await expect(page.getByText("Retry of")).toBeVisible();
  await expect(page.getByLabel("Step explode")).toContainText("failed", { timeout: 15_000 });
});

test("cancelling an active Run ends it as cancelled", async ({ signedInPage: page, api }) => {
  await pollFast(page);
  const id = uniqueName("cancel");
  await deploy(api, id, [sleep("long", "1h")]);
  const runId = await trigger(api, id);

  await page.goto(`/runs/${runId}`);
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Cancel Run" }).click();
  await expect(page.getByRole("status", { name: "Run status" })).toHaveText("cancelled", {
    timeout: 15_000,
  });
});

test("filters narrow the Runs list", async ({ signedInPage: page, api }) => {
  const ok = uniqueName("filter-ok");
  const bad = uniqueName("filter-bad");
  await deploy(api, ok, [sleep("s", "1s")]);
  await deploy(api, bad, [fail("f", "nope")]);
  const okRun = await trigger(api, ok);
  const badRun = await trigger(api, bad);
  const rowOf = (runId: string) => page.getByRole("row").filter({ hasText: runId.slice(0, 8) });

  await page.goto("/runs");
  await expect(rowOf(okRun)).toBeVisible();
  await expect(rowOf(badRun)).toBeVisible();

  const filters = page.getByRole("form", { name: "Filters" });
  await filters.getByLabel("Workflow").selectOption(bad);
  await expect(rowOf(badRun)).toBeVisible();
  await expect(rowOf(okRun)).toHaveCount(0);

  await filters.getByRole("button", { name: "Clear filters" }).click();
  await filters.getByLabel("Status").selectOption("failed");
  await expect(rowOf(badRun)).toBeVisible({ timeout: 15_000 });
  await expect(rowOf(okRun)).toHaveCount(0);

  await filters.getByRole("button", { name: "Clear filters" }).click();
  await filters.getByLabel("From").fill("2999-01-01");
  await expect(page.getByText("No Runs match.")).toBeVisible();
});

test("Secret placeholders are labelled and large Outputs stay collapsed until opened", async ({
  signedInPage: page,
  api,
}) => {
  const id = uniqueName("large");
  const big = "x".repeat(200_000);
  await deploy(api, id, [
    {
      id: "make",
      action: "transform.expr",
      params: { expression: "values.big", values: { big } },
    },
  ]);
  const runId = await trigger(api, id, { token: "{{ secrets.API_KEY }}" });

  await page.goto(`/runs/${runId}`);
  const step = page.getByLabel("Step make");
  await expect(step).toContainText("succeeded", { timeout: 15_000 });

  await expect(page.getByLabel("Output", { exact: true })).toHaveCount(0);
  await step.getByText("Output", { exact: true }).click();
  const output = page.getByLabel("Output", { exact: true });
  await expect(output).toBeVisible();
  expect((await output.textContent())!.length).toBeLessThan(10_000);
  await step.getByRole("button", { name: /Show all/ }).click();
  expect((await output.textContent())!.length).toBeGreaterThan(200_000);

  await expect(page.getByText("secret placeholder")).toBeVisible();
  await page.getByText("Run input").click();
  await expect(page.getByLabel("Run input", { exact: true })).toContainText(
    "{{ secrets.API_KEY }}",
  );
});
