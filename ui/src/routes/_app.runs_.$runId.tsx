import { createFileRoute, Link } from "@tanstack/react-router";
import { useState } from "react";
import { messageOf } from "@/api/request";
import { isActive, useRun, useRunAction, type RunInfo } from "@/api/runs";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { StatusBadge } from "@/components/status-badge";
import { Timeline } from "@/components/timeline/timeline";
import { JsonBlock } from "@/components/timeline/json-block";
import { Button } from "@/components/ui/button";
import { formatTime } from "@/lib/format";

export const Route = createFileRoute("/_app/runs_/$runId")({
  component: RunDetail,
});

type Pending = "cancel" | "resume" | "retry";

const COPY: Record<Pending, { title: string; body: string; confirm: string }> = {
  cancel: {
    title: "Cancel this Run?",
    body: "The Run stops after the Step that is running now finishes. This cannot be undone.",
    confirm: "Cancel Run",
  },
  resume: {
    title: "Resume this Run?",
    body: "The Run continues from the Step that failed, on its own Version. Steps that succeeded do not run again.",
    confirm: "Resume",
  },
  retry: {
    title: "Retry this Run?",
    body: "A new Run starts with this Run's input and Version. This Run is left as it is.",
    confirm: "Retry",
  },
};

function RunDetail() {
  const { runId } = Route.useParams();
  const run = useRun(runId);
  const [pending, setPending] = useState<Pending | null>(null);
  const [retriedAs, setRetriedAs] = useState<string | null>(null);
  const cancel = useRunAction(runId, "cancel");
  const resume = useRunAction(runId, "resume");
  const retry = useRunAction(runId, "retry");
  const mutations = { cancel, resume, retry };

  if (run.isPending) return <p className="text-sm text-muted-foreground">Loading…</p>;
  if (run.error) {
    return (
      <p role="alert" className="text-sm text-destructive">
        {messageOf(run.error)}
      </p>
    );
  }
  const data = run.data;
  const active = isActive(data);
  const stoppedMidStep = data.status === "failed" && data.timeline.some((e) => e.step_id);
  const mutation = pending ? mutations[pending] : null;

  function confirm() {
    if (!pending) return;
    const kind = pending;
    mutations[kind].mutate(undefined, {
      onSuccess: (result) => {
        setPending(null);
        if (kind === "retry") setRetriedAs(result.id);
      },
    });
  }

  return (
    <section className="flex flex-col gap-4">
      <div>
        <Link
          to="/runs"
          className="text-sm text-muted-foreground underline-offset-4 hover:underline"
        >
          All Runs
        </Link>
        <div className="mt-1 flex flex-wrap items-center gap-3">
          <h1 className="font-mono text-xl font-semibold">{data.id}</h1>
          <span role="status" aria-label="Run status">
            <StatusBadge status={data.status} />
          </span>
          {data.cancel_requested_at && active && (
            <span className="text-sm text-muted-foreground">cancelling…</span>
          )}
        </div>
      </div>

      <div className="flex flex-wrap gap-2">
        <Button variant="outline" disabled={!active} onClick={() => setPending("cancel")}>
          Cancel
        </Button>
        <Button variant="outline" disabled={!stoppedMidStep} onClick={() => setPending("resume")}>
          Resume
        </Button>
        <Button variant="outline" disabled={active} onClick={() => setPending("retry")}>
          Retry
        </Button>
      </div>
      {retriedAs && (
        <p className="text-sm">
          A new Run started.{" "}
          <Link
            to="/runs/$runId"
            params={{ runId: retriedAs }}
            className="underline underline-offset-4"
          >
            Open the new Run
          </Link>
        </p>
      )}

      <Facts run={data} />
      <JsonBlock label="Run input" value={data.input} />

      <div>
        <h2 className="mb-2 text-lg font-semibold">Timeline</h2>
        <Timeline entries={data.timeline} />
      </div>

      <ConfirmDialog
        open={pending !== null}
        title={pending ? COPY[pending].title : ""}
        confirmLabel={pending ? COPY[pending].confirm : ""}
        busy={mutation?.isPending}
        error={mutation?.error ? messageOf(mutation.error) : undefined}
        onConfirm={confirm}
        onCancel={() => {
          mutation?.reset();
          setPending(null);
        }}
      >
        {pending && COPY[pending].body}
      </ConfirmDialog>
    </section>
  );
}

function Facts({ run }: { run: RunInfo }) {
  return (
    <dl className="grid grid-cols-[max-content_1fr] gap-x-6 gap-y-1 text-sm">
      <dt className="text-muted-foreground">Workflow</dt>
      <dd>
        <Link
          to="/workflows/$workflowId"
          params={{ workflowId: run.workflow_id }}
          className="underline-offset-4 hover:underline"
        >
          {run.workflow_id}
        </Link>
      </dd>
      <dt className="text-muted-foreground">Version</dt>
      <dd>
        <Link
          to="/workflows/$workflowId"
          params={{ workflowId: run.workflow_id }}
          search={{ v: run.version }}
          className="underline-offset-4 hover:underline"
        >
          v{run.version}
        </Link>
      </dd>
      <dt className="text-muted-foreground">Started</dt>
      <dd>{formatTime(run.created_at)}</dd>
      <dt className="text-muted-foreground">Finished</dt>
      <dd>{formatTime(run.finished_at)}</dd>
      {run.retry_of && (
        <>
          <dt className="text-muted-foreground">Retry of</dt>
          <dd>
            <Link
              to="/runs/$runId"
              params={{ runId: run.retry_of }}
              className="font-mono text-xs underline-offset-4 hover:underline"
            >
              {run.retry_of}
            </Link>
          </dd>
        </>
      )}
      <dt className="text-muted-foreground">Trigger</dt>
      <dd>
        <JsonBlock label="Trigger metadata" value={run.trigger} />
      </dd>
    </dl>
  );
}
