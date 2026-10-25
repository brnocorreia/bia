import { describe, expect, it } from "vitest";
import {
  barPlacement,
  buildTimelineModel,
  formatDuration,
  hasSecretPlaceholder,
  type TimelineEntry,
} from "./model";

function entry(over: Partial<TimelineEntry> & { seq: number }): TimelineEntry {
  return {
    status: "succeeded",
    started_at: "2026-01-01T00:00:00Z",
    finished_at: "2026-01-01T00:00:01Z",
    duration_ms: 1000,
    ...over,
  };
}

describe("buildTimelineModel", () => {
  it("groups the Attempts of a Step and keeps the Step order", () => {
    const model = buildTimelineModel([
      entry({ seq: 2, step_id: "b", attempt: 1 }),
      entry({ seq: 1, step_id: "a", attempt: 1, status: "failed" }),
      entry({ seq: 3, step_id: "a", attempt: 2 }),
    ]);
    expect(model.steps.map((s) => s.stepId)).toEqual(["a", "b"]);
    expect(model.steps[0]!.attempts.map((e) => e.attempt)).toEqual([1, 2]);
    expect(model.steps[0]!.status).toBe("succeeded");
  });

  it("separates loop iterations and indents them", () => {
    const model = buildTimelineModel([
      entry({ seq: 1, step_id: "s", iteration_path: "0" }),
      entry({ seq: 2, step_id: "s", iteration_path: "1" }),
      entry({ seq: 3, step_id: "t", iteration_path: "" }),
    ]);
    expect(model.steps.map((s) => s.depth)).toEqual([1, 1, 0]);
    expect(model.steps).toHaveLength(3);
  });

  it("keeps entries about the Run as a whole apart from Steps", () => {
    const model = buildTimelineModel([entry({ seq: 1, status: "failed" })]);
    expect(model.runEntries).toHaveLength(1);
    expect(model.steps).toHaveLength(0);
  });
});

describe("formatDuration", () => {
  it.each([
    [850, "850ms"],
    [2400, "2.4s"],
    [185_000, "3m 05s"],
    [3_720_000, "1h 02m"],
    [4 * 86_400_000 + 3 * 3_600_000, "4d 3h"],
  ])("%d ms is %s", (ms, text) => expect(formatDuration(ms)).toBe(text));
});

describe("barPlacement", () => {
  it("keeps a zero-length bar visible", () => {
    const bar = barPlacement(
      { startMs: Date.parse("2026-01-01T00:00:00Z"), endMs: Date.parse("2026-01-01T00:00:10Z") },
      "2026-01-01T00:00:05Z",
      "2026-01-01T00:00:05Z",
    );
    expect(bar.left).toBe(50);
    expect(bar.width).toBe(1);
  });
});

describe("hasSecretPlaceholder", () => {
  it("finds a Template reading secrets and the redaction marker, deeply", () => {
    expect(hasSecretPlaceholder({ headers: ["Bearer {{ secrets.API_KEY }}"] })).toBe(true);
    expect(hasSecretPlaceholder({ message: "got [REDACTED]" })).toBe(true);
    expect(hasSecretPlaceholder({ a: "{{ input.x }}" })).toBe(false);
  });
});
