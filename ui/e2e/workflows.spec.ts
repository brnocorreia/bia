import { expect, startSessionWith, test, uniqueName, type SeedApi } from "./support/fixtures";

type Step = { id: string; action: string; params: Record<string, unknown> };

const sleep = (duration: string): Step => ({
  id: "pause",
  action: "flow.sleep",
  params: { duration },
});

async function saveDraft(api: SeedApi, id: string, steps: Step[], description?: string) {
  const created = await api.POST("/v1/workflows/{workflow_id}/versions", {
    params: { path: { workflow_id: id } },
    body: { schema_version: 1, id, description, steps } as never,
  });
  expect(created.response.status).toBe(201);
  return created.data!.version.number;
}

async function publish(api: SeedApi, id: string, number: number) {
  const published = await api.POST("/v1/workflows/{workflow_id}/versions/{number}/publish", {
    params: { path: { workflow_id: id, number } },
  });
  expect(published.response.ok).toBe(true);
}

async function activate(api: SeedApi, id: string, number: number) {
  const activated = await api.POST("/v1/workflows/{workflow_id}/versions/{number}/activate", {
    params: { path: { workflow_id: id, number } },
  });
  expect(activated.response.ok).toBe(true);
}

test("the list shows Workflows with their Active Version and description", async ({
  signedInPage: page,
  api,
}) => {
  const id = uniqueName("listed");
  const one = await saveDraft(api, id, [sleep("1s")], "Sleeps for a second");
  await publish(api, id, one);
  await activate(api, id, one);
  const unactivated = uniqueName("fresh");
  await saveDraft(api, unactivated, [sleep("1s")]);

  await page.goto("/workflows");

  const row = page.getByRole("row").filter({ hasText: id });
  await expect(row).toContainText("Sleeps for a second");
  await expect(row).toContainText("v1");
  await expect(page.getByRole("row").filter({ hasText: unactivated })).toContainText("none");
});

test("a Workflow's page lists Versions with status and shows the Definition as YAML or JSON", async ({
  signedInPage: page,
  api,
}) => {
  const id = uniqueName("history");
  const one = await saveDraft(api, id, [sleep("1s")]);
  await publish(api, id, one);
  await activate(api, id, one);
  await saveDraft(api, id, [sleep("2s")]);

  await page.goto("/workflows");
  await page.getByRole("link", { name: id }).click();

  const v1 = page.getByRole("row", { name: "Version 1" });
  await expect(v1).toContainText("Published");
  await expect(v1).toContainText("Active");
  await expect(page.getByRole("row", { name: "Version 2" })).toContainText("Draft");

  await expect(page.getByLabel("Definition", { exact: true })).toContainText("duration: 1s");
  await page.getByRole("button", { name: "JSON" }).click();
  await expect(page.getByLabel("Definition", { exact: true })).toContainText('"duration": "1s"');
  await page.getByRole("row", { name: "Version 2" }).getByRole("button", { name: "View" }).click();
  await expect(page.getByLabel("Definition", { exact: true })).toContainText('"duration": "2s"');
});

test("diffing two Versions shows what changed", async ({ signedInPage: page, api }) => {
  const id = uniqueName("diffed");
  await saveDraft(api, id, [sleep("1s")]);
  await saveDraft(api, id, [sleep("2s")]);

  await page.goto(`/workflows/${id}`);
  await page.getByLabel("From").selectOption("1");
  await page.getByLabel("To").selectOption("2");

  const changes = page.getByRole("table", { name: "Changes from version 1 to version 2" });
  const change = changes.getByRole("row").filter({ hasText: "/steps/0/params/duration" });
  await expect(change).toContainText("replace");
  await expect(change).toContainText('"1s"');
  await expect(change).toContainText('"2s"');
});

test("an admin publishes a Draft, activates it, and the list updates", async ({
  signedInPage: page,
  api,
}) => {
  const id = uniqueName("shipped");
  await saveDraft(api, id, [sleep("1s")]);

  await page.goto(`/workflows/${id}`);
  await page.getByRole("button", { name: "Publish" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Publish" }).click();
  await expect(page.getByRole("row", { name: "Version 1" })).toContainText("Published");

  await page.getByRole("button", { name: "Activate" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Activate" }).click();
  await expect(page.getByRole("row", { name: "Version 1" })).toContainText("Active");

  await page.getByRole("link", { name: "Workflows" }).first().click();
  await expect(page.getByRole("row").filter({ hasText: id })).toContainText("v1");
});

test("publishing a Draft with errors shows its Diagnostics", async ({
  signedInPage: page,
  api,
}) => {
  const id = uniqueName("broken");
  await saveDraft(api, id, [
    { id: "send", action: "queue.publish", params: { queue: "no-such-queue", body: {} } },
  ]);

  await page.goto(`/workflows/${id}`);
  await page.getByRole("button", { name: "Publish" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Publish" }).click();

  const diagnostics = page.getByRole("dialog").getByRole("list", { name: "Diagnostics" });
  // A missing Queue is only a warning on a Draft, so it is publishing that fails.
  await expect(diagnostics).toContainText("/steps/0/params/queue");
  await expect(diagnostics).toContainText("unknown_queue");
  await expect(page.getByRole("row", { name: "Version 1" })).toContainText("Draft");
});

test("rolling back activates an older Version", async ({ signedInPage: page, api }) => {
  const id = uniqueName("rolled");
  for (const duration of ["1s", "2s"]) {
    const number = await saveDraft(api, id, [sleep(duration)]);
    await publish(api, id, number);
  }
  await activate(api, id, 2);

  await page.goto(`/workflows/${id}`);
  await expect(page.getByRole("row", { name: "Version 2" })).toContainText("Active");
  await page.getByRole("button", { name: "Roll back to this" }).click();
  await expect(page.getByRole("dialog")).toContainText("Runs already started keep their Version");
  await page.getByRole("dialog").getByRole("button", { name: "Roll back" }).click();

  await expect(page.getByRole("row", { name: "Version 1" })).toContainText("Active");
  await expect(page.getByRole("row", { name: "Version 2" })).not.toContainText("Active");
});

test("with an agent Token the publish and activate controls are disabled", async ({
  page,
  api,
}) => {
  const id = uniqueName("guarded");
  const one = await saveDraft(api, id, [sleep("1s")]);
  await publish(api, id, one);
  await saveDraft(api, id, [sleep("2s")]);
  const agent = await api.POST("/v1/tokens", {
    body: { name: uniqueName("agent"), scope: "agent" },
  });
  await startSessionWith(page, agent.data!.value);

  await page.goto(`/workflows/${id}`);

  await expect(page.getByRole("row", { name: "Version 1" })).toBeVisible();
  await expect(page.getByText("need an admin Token; this one is agent")).toBeVisible();
  await expect(page.getByRole("button", { name: "Publish" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Activate" })).toBeDisabled();
  // The Definition is still readable.
  await expect(page.getByLabel("Definition", { exact: true })).toBeVisible();
});
