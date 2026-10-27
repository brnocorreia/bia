import pg from "pg";
import {
  apiClient,
  expect,
  startSessionWith,
  test,
  uniqueName,
  type SeedApi,
} from "./support/fixtures";
import { e2eEnv } from "./support/env";

interface Seeded {
  body: unknown;
  /** Seconds from now until it is deliverable; `null` for a dead letter kept in its Queue. */
  visibleIn?: number | null;
  claimedBy?: string;
  deliveries?: number;
  lastError?: string;
  /** The Queue it was dead-lettered from, which makes it a dead letter. */
  deadLetteredFrom?: string;
}

// Nothing consumes Queues through the API yet, so Messages in flight and dead letters are
// written straight into the e2e database.
async function seedMessages(queue: string, messages: Seeded[]) {
  const client = new pg.Client({ connectionString: e2eEnv().databaseUrl });
  await client.connect();
  try {
    for (const m of messages) {
      await client.query(
        `INSERT INTO messages (queue, body, published_at, visible_at, deliveries, claimed_by,
                               last_error, dead_lettered_at, dead_lettered_from)
         VALUES ($1, $2, now(), now() + make_interval(secs => $3), $4, $5, $6,
                 CASE WHEN $7::text IS NULL THEN NULL ELSE now() END, $7)`,
        [
          queue,
          JSON.stringify(m.body),
          m.visibleIn === null ? null : (m.visibleIn ?? 0),
          m.deliveries ?? 0,
          m.claimedBy ?? null,
          m.lastError ?? null,
          m.deadLetteredFrom ?? null,
        ],
      );
    }
  } finally {
    await client.end();
  }
}

/** A Queue dead-lettering into its own Dead-Letter Queue, both freshly named. */
async function queuePair(api: SeedApi) {
  const source = uniqueName("orders");
  const dlq = `${source}-dlq`;
  await api.PUT("/v1/queues/{name}", { params: { path: { name: dlq } }, body: {} });
  await api.PUT("/v1/queues/{name}", {
    params: { path: { name: source } },
    body: { max_retries: 2, dead_letter: dlq },
  });
  return { source, dlq };
}

async function messagesOf(api: SeedApi, name: string) {
  const listed = await api.GET("/v1/queues/{name}/messages", { params: { path: { name } } });
  return listed.data!.messages;
}

test("Queues are listed with their settings and Message counts", async ({
  signedInPage: page,
  api,
}) => {
  const { source, dlq } = await queuePair(api);
  await seedMessages(source, [
    { body: 1 },
    { body: 2 },
    { body: 3, visibleIn: 30, claimedBy: "worker", deliveries: 1 },
    { body: 4, visibleIn: 600 },
  ]);
  await seedMessages(dlq, [{ body: 5, deadLetteredFrom: source, lastError: "boom" }]);

  await page.goto("/queues");
  const cells = (name: string) =>
    page
      .getByRole("row")
      .filter({ has: page.getByRole("link", { name, exact: true }) })
      .getByRole("cell");
  await expect(cells(source)).toHaveText([source, "2", "1", "1", "0", "2", "30s", "10", dlq]);
  await expect(cells(dlq)).toHaveText([dlq, "0", "0", "0", "1", "3", "30s", "10", "—"]);

  await page.getByRole("link", { name: source, exact: true }).click();
  await expect(page.getByRole("heading", { name: source })).toBeVisible();
  const rows = page.getByRole("row").filter({ has: page.getByRole("cell") });
  await expect(rows).toHaveCount(4);
  await expect(rows.nth(2)).toContainText("in flight");
  await expect(rows.nth(3)).toContainText("delayed");
});

test("a dead letter is listed with its last error and replayed to its source Queue", async ({
  signedInPage: page,
  api,
}) => {
  const { source, dlq } = await queuePair(api);
  await seedMessages(dlq, [
    { body: { order: 1 }, deadLetteredFrom: source, lastError: "card declined" },
    { body: { order: 2 }, deadLetteredFrom: source, lastError: "timed out" },
  ]);
  const [first, second] = await messagesOf(api, dlq);

  await page.goto(`/queues/${dlq}?show=dead`);
  const row = page.getByRole("row").filter({ hasText: "card declined" });
  await expect(row).toContainText("dead letter");
  await expect(row).toContainText(`from ${source}`);
  await expect(page.getByRole("row").filter({ hasText: "timed out" })).toBeVisible();

  await page.getByLabel(`Select Message ${first!.id}`).check();
  await page.getByRole("button", { name: "Replay selected (1)" }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("delivery count reset");
  await dialog.getByRole("button", { name: "Replay" }).click();

  await expect(page.getByText("Replayed 1 dead letter.")).toBeVisible();
  await expect(row).toHaveCount(0);
  expect((await messagesOf(api, dlq)).map((m) => m.id)).toEqual([second!.id]);
  const [replayed] = await messagesOf(api, source);
  expect(replayed).toMatchObject({
    body: { order: 1 },
    deliveries: 0,
    in_flight: false,
    dead_lettered_at: null,
    last_error: "card declined",
  });
});

test("purging removes the selected dead letters after confirmation", async ({
  signedInPage: page,
  api,
}) => {
  const audit = uniqueName("audit");
  await api.PUT("/v1/queues/{name}", { params: { path: { name: audit } }, body: {} });
  await seedMessages(audit, [
    { body: "a", visibleIn: null, deadLetteredFrom: audit, lastError: "rejected" },
    { body: "b", visibleIn: null, deadLetteredFrom: audit, lastError: "rejected" },
    { body: "alive" },
  ]);

  await page.goto(`/queues/${audit}`);
  await page.getByLabel("Select all dead letters").check();
  await page.getByRole("button", { name: "Purge selected (2)" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("button", { name: "Cancel" }).click();
  expect(await messagesOf(api, audit)).toHaveLength(3);

  await page.getByRole("button", { name: "Purge selected (2)" }).click();
  await dialog.getByRole("button", { name: "Purge" }).click();
  await expect(page.getByText("Purged 2 dead letters.")).toBeVisible();
  await expect(page.getByRole("row").filter({ hasText: "dead letter" })).toHaveCount(0);
  expect((await messagesOf(api, audit)).map((m) => m.body)).toEqual(["alive"]);
});

test("a non-admin Token sees dead letters but cannot replay or purge them", async ({
  page,
  api,
}) => {
  const { source, dlq } = await queuePair(api);
  await seedMessages(dlq, [{ body: 1, deadLetteredFrom: source, lastError: "boom" }]);
  const made = await api.POST("/v1/tokens", {
    body: { name: uniqueName("viewer"), scope: "agent" },
  });
  const agent = made.data!.value;
  await startSessionWith(page, agent);

  await page.goto(`/queues/${dlq}`);
  await expect(page.getByRole("row").filter({ hasText: "boom" })).toBeVisible();
  await expect(page.getByText("need an admin Token")).toBeVisible();
  await expect(page.getByRole("button", { name: /Replay selected/ })).toBeDisabled();
  await expect(page.getByRole("button", { name: /Purge selected/ })).toBeDisabled();
  await expect(page.getByLabel("Select all dead letters")).toBeDisabled();

  const [dead] = await messagesOf(api, dlq);
  const replay = await apiClient(agent).POST("/v1/queues/{name}/dead-letters/replay", {
    params: { path: { name: dlq } },
    body: { ids: [dead!.id] },
  });
  expect(replay.response.status).toBe(403);
});
