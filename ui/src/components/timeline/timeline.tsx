import { useMemo } from "react";
import { StatusBadge } from "@/components/status-badge";
import { Badge } from "@/components/ui/badge";
import { JsonBlock } from "./json-block";
import {
  barPlacement,
  buildTimelineModel,
  formatDuration,
  type StepGroup,
  type TimelineEntry,
  type TimelineModel,
} from "./model";

export interface TimelineProps {
  /** The API's Timeline: a Run's `timeline` or a Dry Run report's. Rendered as given, no fetching. */
  entries: readonly TimelineEntry[];
  /**
   * Marks the Timeline as a Dry Run's: shows a badge and a note that waits took no real time
   * and that the times are the ones a real Run would have taken.
   */
  dryRun?: boolean;
}

/**
 * Every Step with its Attempts in order: inputs, Output, error and duration for each, a bar
 * placing the Step in time, and loop iterations indented. Inputs and Outputs are closed until
 * opened, so large values cost nothing; an Attempt's error message is always visible.
 */
export function Timeline({ entries, dryRun = false }: TimelineProps) {
  const model = useMemo(() => buildTimelineModel(entries), [entries]);

  return (
    <section aria-label="Timeline" className="flex flex-col gap-3">
      {dryRun && (
        <div className="flex items-center gap-2 text-sm text-muted-foreground">
          <Badge variant="outline">dry run</Badge>
          <span>
            Not a real Run. Waits took no real time; the durations are what they would take.
          </span>
        </div>
      )}
      {entries.length === 0 && (
        <p className="text-sm text-muted-foreground">Nothing has run yet.</p>
      )}
      {model.runEntries.length > 0 && (
        <ul aria-label="Run events" className="flex flex-col gap-2">
          {model.runEntries.map((entry) => (
            <li key={entry.seq} className="rounded-md border p-3 text-sm">
              <div className="flex items-center gap-2">
                <span className="font-medium">Run</span>
                <StatusBadge status={entry.status} />
                <Duration ms={entry.duration_ms} />
              </div>
              <AttemptDetails entry={entry} inputLabel="Run input" />
            </li>
          ))}
        </ul>
      )}
      {model.steps.length > 0 && (
        <ol aria-label="Steps" className="flex flex-col gap-2">
          {model.steps.map((group) => (
            <StepItem key={group.key} group={group} model={model} />
          ))}
        </ol>
      )}
    </section>
  );
}

function Duration({ ms }: { ms: number }) {
  return (
    <span className="text-xs text-muted-foreground" title={`${ms} ms`}>
      {formatDuration(ms)}
    </span>
  );
}

function StepItem({ group, model }: { group: StepGroup; model: TimelineModel }) {
  const first = group.attempts[0]!;
  const last = group.attempts[group.attempts.length - 1]!;
  const bar = barPlacement(model, first.started_at, last.finished_at);
  const skipped = group.status === "skipped";

  return (
    <li
      aria-label={`Step ${group.stepId}`}
      style={{ marginLeft: `${group.depth * 1.5}rem` }}
      className={`rounded-md border p-3 ${skipped ? "border-dashed text-muted-foreground" : ""}`}
    >
      <div className="flex flex-wrap items-center gap-2 text-sm">
        <span className="font-medium">{group.stepId}</span>
        {group.action && <code className="text-xs text-muted-foreground">{group.action}</code>}
        {group.iterationPath && <Badge variant="secondary">iteration {group.iterationPath}</Badge>}
        <StatusBadge status={group.status} />
        {group.attempts.length > 1 && (
          <span className="text-xs text-muted-foreground">{group.attempts.length} attempts</span>
        )}
        <Duration ms={group.elapsedMs} />
      </div>
      <div aria-hidden className="relative mt-2 h-1.5 rounded bg-muted">
        <div
          className={`absolute h-full rounded ${group.status === "failed" ? "bg-red-500" : "bg-primary/60"}`}
          style={{ left: `${bar.left}%`, width: `${bar.width}%` }}
        />
      </div>
      <ol aria-label={`Attempts of ${group.stepId}`} className="mt-2 flex flex-col gap-2">
        {group.attempts.map((entry) => (
          <li key={entry.seq} className="border-l-2 pl-3 text-sm">
            <div className="flex flex-wrap items-center gap-2">
              <span>Attempt {entry.attempt ?? 1}</span>
              <StatusBadge status={entry.status} />
              <Duration ms={entry.duration_ms} />
            </div>
            <AttemptDetails entry={entry} />
          </li>
        ))}
      </ol>
    </li>
  );
}

interface StepError {
  class?: string;
  message?: string;
  code?: string;
  details?: unknown;
}

function AttemptDetails({
  entry,
  inputLabel = "Input",
}: {
  entry: TimelineEntry;
  inputLabel?: string;
}) {
  const error = entry.error as StepError | undefined | null;
  return (
    <div className="mt-1 flex flex-col gap-0.5">
      {error && (
        <div role="alert" className="text-sm text-destructive">
          <span className="font-medium">Error</span>
          {error.class && (
            <Badge variant="outline" className="mx-2">
              {error.class}
            </Badge>
          )}
          {error.code && <code className="mr-2 text-xs">{error.code}</code>}
          <span>{error.message ?? JSON.stringify(error)}</span>
          {error.details !== undefined && <JsonBlock label="Error details" value={error.details} />}
        </div>
      )}
      {entry.input !== undefined && <JsonBlock label={inputLabel} value={entry.input} />}
      {entry.output !== undefined && <JsonBlock label="Output" value={entry.output} />}
    </div>
  );
}
