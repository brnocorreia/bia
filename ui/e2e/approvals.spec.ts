import { expect, actAsAgent, apiClient, test, uniqueName, type SeedApi } from "./support/fixtures";

// The nav badge counts every pending request, so these tests must not overlap.
test.describe.configure({ mode: "serial" });

type Step = { id: string; action: string; params: Record<string, unknown> };

const sleep = (duration: string): Step => ({
  id: "pause",
  action: "flow.sleep",
  params: { duration },
});

const body = (id: string, steps: Step[]) => ({ schema_version: 1, id, steps }) as never;

async function saveDraft(api: SeedApi, id: string, steps: Step[]) {
  const created = await api.POST("/v1/workflows/{workflow_id}/versions", {
    params: { path: { workflow_id: id } },
    body: body(id, steps),
  });
  expect(created.response.status).toBe(201);
  return created.data!.version.number;
}

/** A Workflow with v1 Active and a Draft v2 that sleeps longer. */
async function workflowWithDraft(api: SeedApi) {
  const id = uniqueName("approve");
  const one = await saveDraft(api, id, [sleep("1s")]);
  const params = { params: { path: { workflow_id: id, number: one } } };
  expect(
    (await api.POST("/v1/workflows/{workflow_id}/versions/{number}/publish", params)).response.ok,
  ).toBe(true);
  expect(
    (await api.POST("/v1/workflows/{workflow_id}/versions/{number}/activate", params)).response.ok,
  ).toBe(true);
  const two = await saveDraft(api, id, [sleep("2s")]);
  return { id, one, two };
}

/** A client for a fresh `agent` Token, which is what asks for approval. */
async function agentClient(api: SeedApi) {
  const created = await api.POST("/v1/tokens", {
    body: { name: uniqueName("agent"), scope: "agent" },
  });
  expect(created.response.status).toBe(201);
  return { client: apiClient(created.data!.value), name: created.data!.token.name };
}

async function ask(agent: SeedApi, workflow: string, number: number, note: string) {
  const asked = await agent.POST("/v1/approval-requests", {
    body: { workflow_id: workflow, number, action: "publish_and_activate", note },
  });
  expect(asked.response.status).toBe(201);
  return asked.data!;
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => window.localStorage.setItem("bia.pollIntervalMs", "100"));
});

test("an agent's request shows on the badge, is reviewed against the Active Version and approved", async ({
  signedInPage: page,
  api,
}) => {
  const { id, one, two } = await workflowWithDraft(api);
  const { client: agent, name } = await agentClient(api);

  await page.goto("/workflows");
  await expect(page.getByRole("navigation", { name: "Main" }).getByLabel("1 pending")).toHaveCount(
    0,
  );

  const request = await ask(agent, id, two, "Longer pause please");
  await expect(
    page.getByRole("navigation", { name: "Main" }).getByLabel("1 pending"),
  ).toBeVisible();

  await page.getByRole("link", { name: /Approvals/ }).click();
  const row = page.getByRole("row").filter({ hasText: id });
  await expect(row).toContainText(`v${two}`);
  await expect(row).toContainText("Publish and activate");
  await expect(row).toContainText(name);
  await expect(row).toContainText("Longer pause please");
  await row.getByRole("link", { name: id }).click();

  await expect(page.getByText(`Changes from the Active Version (v${one})`)).toBeVisible();
  const diff = page.getByRole("table", { name: `Changes from version ${one} to version ${two}` });
  await expect(diff).toContainText("/steps/0/params/duration");
  await expect(diff).toContainText('"2s"');

  await page.getByRole("button", { name: "Approve" }).click();
  await expect(page.getByLabel("Status")).toHaveText("Approved");
  await expect(page.getByLabel("1 pending")).toHaveCount(0);

  const workflow = await api.GET("/v1/workflows/{workflow_id}", {
    params: { path: { workflow_id: id } },
  });
  expect(workflow.data!.active_version).toBe(two);
  const decided = await agent.GET("/v1/approval-requests/{id}", {
    params: { path: { id: request.id } },
  });
  expect(decided.data!.status).toBe("approved");
});

test("rejecting with a comment shows it on the request and to the agent", async ({
  signedInPage: page,
  api,
}) => {
  const { id, two } = await workflowWithDraft(api);
  const { client: agent } = await agentClient(api);
  const request = await ask(agent, id, two, "Please");

  await page.goto(`/approvals/${request.id}`);
  await page.getByLabel("Comment").fill("Use 3s instead");
  await page.getByRole("button", { name: "Reject" }).click();

  await expect(page.getByLabel("Status")).toHaveText("Rejected");
  await expect(page.getByTestId("decision-comment")).toHaveText("Use 3s instead");
  await expect(page.getByRole("button", { name: "Approve" })).toHaveCount(0);

  const seen = await agent.GET("/v1/approval-requests/{id}", {
    params: { path: { id: request.id } },
  });
  expect(seen.data).toMatchObject({ status: "rejected", comment: "Use 3s instead" });

  await page.goto("/approvals?tab=decided");
  await expect(page.getByRole("row").filter({ hasText: id })).toContainText("Rejected");
});

test("replacing the Draft while the review is open turns it stale and blocks approving", async ({
  signedInPage: page,
  api,
}) => {
  const { id, two } = await workflowWithDraft(api);
  const { client: agent } = await agentClient(api);
  const request = await ask(agent, id, two, "Before the change");

  await page.goto(`/approvals/${request.id}`);
  await expect(page.getByRole("button", { name: "Approve" })).toBeEnabled();

  const replaced = await api.PUT("/v1/workflows/{workflow_id}/versions/{number}", {
    params: { path: { workflow_id: id, number: two } },
    body: body(id, [sleep("5s")]),
  });
  expect(replaced.response.ok).toBe(true);

  await expect(page.getByLabel("Status")).toHaveText("Stale");
  await expect(
    page.getByRole("status").filter({ hasText: "no longer what the agent asked" }),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Approve" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Reject" })).toHaveCount(0);
});

test("a pending request is linked from the Workflow's page", async ({
  signedInPage: page,
  api,
}) => {
  const { id, two } = await workflowWithDraft(api);
  const { client: agent, name } = await agentClient(api);
  await ask(agent, id, two, "Look");

  await page.goto(`/workflows/${id}`);
  await expect(page.getByText(`${name} asks to publish and activate v${two}`)).toBeVisible();
  await page.getByRole("link", { name: "Review the request" }).click();
  await expect(page).toHaveURL(/\/approvals\//);

  // Leave nothing pending for the other tests.
  await page.getByRole("button", { name: "Reject" }).click();
  await expect(page.getByLabel("Status")).toHaveText("Rejected");
});

test("an agent Session sees Approve and Reject disabled", async ({ signedInPage: page, api }) => {
  const { id, two } = await workflowWithDraft(api);
  const { client: agent } = await agentClient(api);
  const request = await ask(agent, id, two, "Look");
  await actAsAgent(page);

  await page.goto(`/approvals/${request.id}`);
  await expect(page.getByRole("button", { name: "Approve" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Reject" })).toBeDisabled();

  await agent.POST("/v1/approval-requests/{id}/withdraw", { params: { path: { id: request.id } } });
});
