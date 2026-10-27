import { createFileRoute, Link, useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import {
  PAGE_SIZE,
  useDeadLetterAction,
  useMessages,
  useQueue,
  type MessageFilter,
  type MessageInfo,
  type QueueInfo,
} from "@/api/queues";
import { messageOf } from "@/api/request";
import { useIsAdmin } from "@/auth/scope";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { JsonBlock } from "@/components/timeline/json-block";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { formatTime } from "@/lib/format";

interface Search {
  show?: MessageFilter;
}

const FILTERS: { value: MessageFilter; label: string }[] = [
  { value: "all", label: "All Messages" },
  { value: "dead", label: "Dead letters" },
  { value: "live", label: "Not dead-lettered" },
];

export const Route = createFileRoute("/_app/queues_/$queueName")({
  validateSearch: (search: Record<string, unknown>): Search => ({
    show: FILTERS.find((f) => f.value === search.show && f.value !== "all")?.value,
  }),
  component: QueueDetail,
});

function QueueDetail() {
  const { queueName } = Route.useParams();
  const search = Route.useSearch();
  const navigate = useNavigate({ from: Route.fullPath });
  const filter = search.show ?? "all";
  const queue = useQueue(queueName);
  const messages = useMessages(queueName, filter);
  const rows = messages.data?.pages.flatMap((page) => page.messages) ?? [];

  return (
    <section>
      <Link to="/queues" className="text-sm text-muted-foreground hover:underline">
        ← Queues
      </Link>
      <div className="mt-2 flex items-center justify-between">
        <h1 className="font-mono text-2xl font-semibold">{queueName}</h1>
        <Button
          variant="outline"
          disabled={queue.isFetching || messages.isFetching}
          onClick={() => {
            void queue.refetch();
            void messages.refetch();
          }}
        >
          Refresh
        </Button>
      </div>
      {queue.error && (
        <p role="alert" className="mt-4 text-sm text-destructive">
          {messageOf(queue.error)}
        </p>
      )}
      {queue.data && <QueueSummary queue={queue.data} />}

      <label className="mt-6 flex w-fit flex-col gap-1 text-sm">
        Show
        <select
          className="h-9 rounded-md border bg-background px-2 text-sm"
          value={filter}
          onChange={(event) =>
            void navigate({
              search: {
                show:
                  event.target.value === "all" ? undefined : (event.target.value as MessageFilter),
              },
            })
          }
        >
          {FILTERS.map((f) => (
            <option key={f.value} value={f.value}>
              {f.label}
            </option>
          ))}
        </select>
      </label>

      {messages.isPending && <p className="mt-4 text-sm text-muted-foreground">Loading…</p>}
      {messages.error && (
        <p role="alert" className="mt-4 text-sm text-destructive">
          {messageOf(messages.error)}
        </p>
      )}
      {messages.data && rows.length === 0 && (
        <p className="mt-4 text-sm text-muted-foreground">No Messages match.</p>
      )}
      {rows.length > 0 && (
        // Keyed by the filter so a selection never outlives the list it was made in.
        <MessageTable
          key={filter}
          queueName={queueName}
          rows={rows}
          readAt={messages.dataUpdatedAt}
        />
      )}
      {messages.hasNextPage && (
        <Button
          variant="outline"
          className="mt-4"
          disabled={messages.isFetchingNextPage}
          onClick={() => void messages.fetchNextPage()}
        >
          Load more
        </Button>
      )}
    </section>
  );
}

function QueueSummary({ queue }: { queue: QueueInfo }) {
  const facts: [string, React.ReactNode][] = [
    ["Ready", queue.messages.ready],
    ["In flight", queue.messages.in_flight],
    ["Delayed", queue.messages.delayed],
    ["Dead-lettered", queue.messages.dead_lettered],
    ["Max retries", queue.max_retries],
    ["Visibility timeout", queue.visibility_timeout],
    ["Batch size", queue.batch_size],
    [
      "Dead-Letter Queue",
      queue.dead_letter ? (
        <Link
          to="/queues/$queueName"
          params={{ queueName: queue.dead_letter }}
          className="font-mono underline-offset-4 hover:underline"
        >
          {queue.dead_letter}
        </Link>
      ) : (
        "— (dead letters stay here)"
      ),
    ],
  ];
  return (
    <dl aria-label="Queue" className="mt-4 grid grid-cols-2 gap-x-6 gap-y-2 text-sm sm:grid-cols-4">
      {facts.map(([term, value]) => (
        <div key={term}>
          <dt className="text-muted-foreground">{term}</dt>
          <dd className="font-medium tabular-nums">{value}</dd>
        </div>
      ))}
    </dl>
  );
}

type Pending = { action: "replay" | "purge"; ids: number[] };

function MessageTable({
  queueName,
  rows,
  readAt,
}: {
  queueName: string;
  rows: MessageInfo[];
  /** When `rows` were fetched, in epoch milliseconds. */
  readAt: number;
}) {
  const isAdmin = useIsAdmin();
  const [selected, setSelected] = useState<ReadonlySet<number>>(new Set());
  const [pending, setPending] = useState<Pending | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const replay = useDeadLetterAction(queueName, "replay");
  const purge = useDeadLetterAction(queueName, "purge");
  const mutation = pending?.action === "purge" ? purge : replay;

  // The API takes at most a page of ids at once, so "select all" stops there.
  const selectable = rows.filter((m) => m.dead_lettered_at).map((m) => m.id);
  const capped = selectable.slice(0, PAGE_SIZE);
  const allSelected = capped.length > 0 && capped.every((id) => selected.has(id));
  const toggle = (id: number) =>
    setSelected((previous) => {
      const next = new Set(previous);
      if (!next.delete(id) && next.size < PAGE_SIZE) next.add(id);
      return next;
    });
  const ask = (action: Pending["action"]) => {
    replay.reset();
    purge.reset();
    setNotice(null);
    setPending({ action, ids: [...selected] });
  };
  const confirm = () => {
    if (!pending) return;
    const { action, ids } = pending;
    mutation.mutate(ids, {
      onSuccess: (done) => {
        setPending(null);
        setSelected(new Set());
        setNotice(outcome(action, done, ids.length));
      },
    });
  };

  return (
    <>
      {selectable.length > 0 && (
        <div className="mt-4 flex flex-wrap items-center gap-3 text-sm">
          <Button
            variant="outline"
            size="sm"
            disabled={!isAdmin || selected.size === 0}
            onClick={() => ask("replay")}
          >
            Replay selected ({selected.size})
          </Button>
          <Button
            variant="outline"
            size="sm"
            disabled={!isAdmin || selected.size === 0}
            onClick={() => ask("purge")}
          >
            Purge selected ({selected.size})
          </Button>
          {!isAdmin && (
            <span className="text-muted-foreground">
              Replaying and purging dead letters need an admin Token.
            </span>
          )}
        </div>
      )}
      <p aria-live="polite" className="mt-2 text-sm">
        {notice}
      </p>
      <table className="mt-2 w-full text-left text-sm">
        <thead className="border-b text-muted-foreground">
          <tr>
            <th className="w-8 py-2 pr-2 font-medium">
              <input
                type="checkbox"
                aria-label="Select all dead letters"
                disabled={!isAdmin || selectable.length === 0}
                checked={allSelected}
                onChange={() => setSelected(allSelected ? new Set() : new Set(capped))}
              />
            </th>
            <th className="py-2 pr-4 font-medium">Id</th>
            <th className="py-2 pr-4 font-medium">Body</th>
            <th className="py-2 pr-4 font-medium">State</th>
            <th className="py-2 pr-4 text-right font-medium">Deliveries</th>
            <th className="py-2 pr-4 font-medium">Last error</th>
            <th className="py-2 pr-4 font-medium">Published</th>
            <th className="py-2 font-medium">Visible at</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((message) => (
            <tr key={message.id} className="border-b align-top">
              <td className="py-2 pr-2">
                {message.dead_lettered_at && (
                  <input
                    type="checkbox"
                    aria-label={`Select Message ${message.id}`}
                    disabled={!isAdmin}
                    checked={selected.has(message.id)}
                    onChange={() => toggle(message.id)}
                  />
                )}
              </td>
              <td className="py-2 pr-4 font-mono text-xs">{message.id}</td>
              <td className="max-w-md py-2 pr-4">
                <JsonBlock label="Body" value={message.body} />
              </td>
              <td className="py-2 pr-4">
                <MessageState message={message} readAt={readAt} />
              </td>
              <td className="py-2 pr-4 text-right tabular-nums">{message.deliveries}</td>
              <td className="max-w-sm py-2 pr-4 break-words">{message.last_error ?? "—"}</td>
              <td className="py-2 pr-4">{formatTime(message.published_at)}</td>
              <td className="py-2">{formatTime(message.visible_at)}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <ConfirmDialog
        open={pending !== null}
        title={
          pending?.action === "purge"
            ? `Purge ${count(pending.ids.length)}?`
            : `Replay ${count(pending?.ids.length ?? 0)}?`
        }
        confirmLabel={pending?.action === "purge" ? "Purge" : "Replay"}
        busy={mutation.isPending}
        error={mutation.error ? messageOf(mutation.error) : undefined}
        onCancel={() => setPending(null)}
        onConfirm={confirm}
      >
        {pending?.action === "purge" ? (
          <>They are deleted for good and cannot be recovered.</>
        ) : (
          <>
            Each goes back to the Queue it was dead-lettered from and is delivered again from now,
            with its delivery count reset. Dead letters a consumer holds right now are skipped.
          </>
        )}
      </ConfirmDialog>
    </>
  );
}

function MessageState({ message, readAt }: { message: MessageInfo; readAt: number }) {
  if (message.dead_lettered_at) {
    return (
      <div className="space-y-1">
        <Badge variant="destructive">dead letter</Badge>
        <div className="text-xs text-muted-foreground">
          from <span className="font-mono">{message.dead_lettered_from}</span>,{" "}
          {formatTime(message.dead_lettered_at)}
        </div>
        {message.in_flight && <Badge variant="outline">in flight</Badge>}
      </div>
    );
  }
  if (message.in_flight) return <Badge variant="outline">in flight</Badge>;
  // Judged by the browser's clock when the page was fetched, so only as right as that clock.
  const delayed = !!message.visible_at && Date.parse(message.visible_at) > readAt;
  return <Badge>{delayed ? "delayed" : "ready"}</Badge>;
}

const count = (n: number) => (n === 1 ? "1 dead letter" : `${n} dead letters`);

function outcome(action: Pending["action"], done: number, asked: number): string {
  const verb = action === "purge" ? "Purged" : "Replayed";
  if (done === asked) return `${verb} ${count(done)}.`;
  const why =
    action === "purge"
      ? "the others were already gone"
      : "the others were in flight, already gone, or their source Queue no longer exists";
  return `${verb} ${done} of ${count(asked)}; ${why}.`;
}
