import type { components } from "@/api/client";

export type TimelineEntry = components["schemas"]["TimelineEntryInfo"];

/** All Attempts of one Step in one loop iteration, in Attempt order. */
export interface StepGroup {
  key: string;
  stepId: string;
  action: string | null;
  /** Empty outside loops. */
  iterationPath: string;
  /** How deeply the Step sits in loops; drives indentation. */
  depth: number;
  attempts: TimelineEntry[];
  /** The last Attempt decides how the Step ended. */
  status: TimelineEntry["status"];
  /** Wall time from the first Attempt's start to the last one's end, retries' backoff included. */
  elapsedMs: number;
  startedAt: string;
}

export interface TimelineModel {
  /** Entries about the Run as a whole, such as input rejected before any Step ran. */
  runEntries: TimelineEntry[];
  steps: StepGroup[];
  /** Start and end of the whole Timeline, for placing bars. */
  startMs: number;
  endMs: number;
}

const SEPARATOR = /[./:]/;

export function iterationDepth(iterationPath: string | null | undefined): number {
  return iterationPath ? iterationPath.split(SEPARATOR).length : 0;
}

/** Groups the flat entries by Step and iteration, keeping the order Steps first appeared in. */
export function buildTimelineModel(entries: readonly TimelineEntry[]): TimelineModel {
  const ordered = [...entries].sort((a, b) => a.seq - b.seq);
  const runEntries: TimelineEntry[] = [];
  const groups = new Map<string, StepGroup>();

  for (const entry of ordered) {
    if (!entry.step_id) {
      runEntries.push(entry);
      continue;
    }
    const iterationPath = entry.iteration_path ?? "";
    const key = `${entry.step_id}\u0000${iterationPath}`;
    let group = groups.get(key);
    if (!group) {
      group = {
        key,
        stepId: entry.step_id,
        action: entry.action ?? null,
        iterationPath,
        depth: iterationDepth(iterationPath),
        attempts: [],
        status: entry.status,
        elapsedMs: 0,
        startedAt: entry.started_at,
      };
      groups.set(key, group);
    }
    group.attempts.push(entry);
    group.status = entry.status;
    group.action ??= entry.action ?? null;
  }

  const steps = [...groups.values()];
  for (const group of steps) {
    const first = group.attempts[0]!;
    const last = group.attempts[group.attempts.length - 1]!;
    group.elapsedMs = Math.max(0, Date.parse(last.finished_at) - Date.parse(first.started_at));
  }

  const starts = ordered.map((e) => Date.parse(e.started_at));
  const ends = ordered.map((e) => Date.parse(e.finished_at));
  const startMs = starts.length ? Math.min(...starts) : 0;
  const endMs = ends.length ? Math.max(...ends) : 0;
  return { runEntries, steps, startMs, endMs };
}

/** Position and width of a bar as percentages of the Timeline's span, never invisible. */
export function barPlacement(
  model: Pick<TimelineModel, "startMs" | "endMs">,
  startedAt: string,
  finishedAt: string,
): { left: number; width: number } {
  const span = model.endMs - model.startMs;
  if (span <= 0) return { left: 0, width: 100 };
  const left = ((Date.parse(startedAt) - model.startMs) / span) * 100;
  const width = ((Date.parse(finishedAt) - Date.parse(startedAt)) / span) * 100;
  return { left: Math.min(Math.max(left, 0), 99), width: Math.min(Math.max(width, 1), 100 - left) };
}

/** `850ms`, `2.4s`, `3m 05s`, `1h 02m`, `3d 4h`: the two largest units that matter. */
export function formatDuration(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)}ms`;
  const seconds = ms / 1000;
  if (seconds < 60) return `${seconds < 10 ? seconds.toFixed(1) : Math.round(seconds)}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${String(Math.floor(seconds % 60)).padStart(2, "0")}s`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ${String(minutes % 60).padStart(2, "0")}m`;
  return `${Math.floor(hours / 24)}d ${hours % 24}h`;
}

const SECRET_TEMPLATE = /\{\{[^}]*\bsecrets\b[^}]*\}\}/;
const REDACTED = "[REDACTED]";

/**
 * Whether a value holds a Secret placeholder: a Template reading `secrets`, shown as written,
 * or the redaction marker. The API never sends a Secret's value; this only labels the
 * placeholders so a reader knows they are not the real thing.
 */
export function hasSecretPlaceholder(value: unknown): boolean {
  if (typeof value === "string") return value.includes(REDACTED) || SECRET_TEMPLATE.test(value);
  if (Array.isArray(value)) return value.some(hasSecretPlaceholder);
  if (typeof value === "object" && value !== null) {
    return Object.values(value).some(hasSecretPlaceholder);
  }
  return false;
}
