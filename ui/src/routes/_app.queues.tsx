import { createFileRoute, Link } from "@tanstack/react-router";
import { useQueues } from "@/api/queues";
import { messageOf } from "@/api/request";
import { Button } from "@/components/ui/button";

export const Route = createFileRoute("/_app/queues")({
  staticData: { nav: { label: "Queues", order: 25 } },
  component: Queues,
});

function Queues() {
  const queues = useQueues();
  return (
    <section>
      <div className="flex items-center justify-between">
        <h1 className="text-2xl font-semibold">Queues</h1>
        <Button
          variant="outline"
          disabled={queues.isFetching}
          onClick={() => void queues.refetch()}
        >
          Refresh
        </Button>
      </div>
      <p className="mt-1 text-sm text-muted-foreground">
        Create and change Queues with <code>bia queue</code>, a <code>kind: queue</code> file or{" "}
        <code>PUT /v1/queues/NAME</code>.
      </p>
      {queues.isPending && <p className="mt-4 text-sm text-muted-foreground">Loading…</p>}
      {queues.error && (
        <p role="alert" className="mt-4 text-sm text-destructive">
          {messageOf(queues.error)}
        </p>
      )}
      {queues.data?.length === 0 && (
        <p className="mt-4 text-sm text-muted-foreground">No Queues yet.</p>
      )}
      {queues.data && queues.data.length > 0 && (
        <table className="mt-4 w-full text-left text-sm">
          <thead className="border-b text-muted-foreground">
            <tr>
              <th className="py-2 pr-4 font-medium">Name</th>
              <th className="py-2 pr-4 text-right font-medium">Ready</th>
              <th className="py-2 pr-4 text-right font-medium">In flight</th>
              <th className="py-2 pr-4 text-right font-medium">Delayed</th>
              <th className="py-2 pr-4 text-right font-medium">Dead-lettered</th>
              <th className="py-2 pr-4 font-medium">Max retries</th>
              <th className="py-2 pr-4 font-medium">Visibility timeout</th>
              <th className="py-2 pr-4 font-medium">Batch size</th>
              <th className="py-2 font-medium">Dead-Letter Queue</th>
            </tr>
          </thead>
          <tbody>
            {queues.data.map((queue) => (
              <tr key={queue.name} className="border-b">
                <td className="py-2 pr-4">
                  <Link
                    to="/queues/$queueName"
                    params={{ queueName: queue.name }}
                    className="font-mono underline-offset-4 hover:underline"
                  >
                    {queue.name}
                  </Link>
                </td>
                <td className="py-2 pr-4 text-right tabular-nums">{queue.messages.ready}</td>
                <td className="py-2 pr-4 text-right tabular-nums">{queue.messages.in_flight}</td>
                <td className="py-2 pr-4 text-right tabular-nums">{queue.messages.delayed}</td>
                <td className="py-2 pr-4 text-right tabular-nums">
                  {queue.messages.dead_lettered}
                </td>
                <td className="py-2 pr-4">{queue.max_retries}</td>
                <td className="py-2 pr-4">{queue.visibility_timeout}</td>
                <td className="py-2 pr-4">{queue.batch_size}</td>
                <td className="py-2 font-mono">{queue.dead_letter ?? "—"}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}
