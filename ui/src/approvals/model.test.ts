import { describe, expect, it } from "vitest";
import { age, displayState, inTab } from "./model";

describe("displayState", () => {
  it("lets an admin decide only a pending request", () => {
    expect(displayState({ status: "pending", number: 2 }).canDecide).toBe(true);
    for (const status of ["approved", "rejected", "stale", "withdrawn"] as const) {
      expect(displayState({ status, number: 2 }).canDecide).toBe(false);
    }
  });

  it("explains a stale request with its Version", () => {
    const state = displayState({ status: "stale", number: 3 });
    expect(state.label).toBe("Stale");
    expect(state.reason).toContain("v3");
  });
});

describe("inTab", () => {
  it("puts pending requests on the pending tab and the rest on the decided one", () => {
    expect(inTab({ status: "pending" }, "pending")).toBe(true);
    expect(inTab({ status: "pending" }, "decided")).toBe(false);
    expect(inTab({ status: "stale" }, "decided")).toBe(true);
    expect(inTab({ status: "approved" }, "pending")).toBe(false);
  });
});

describe("age", () => {
  const now = new Date("2026-01-10T12:00:00Z");
  it.each([
    ["2026-01-10T11:59:30Z", "just now"],
    ["2026-01-10T11:55:00Z", "5 min ago"],
    ["2026-01-10T09:00:00Z", "3 h ago"],
    ["2026-01-08T11:00:00Z", "2 d ago"],
    ["2026-01-10T12:00:05Z", "just now"],
  ])("%s is %s", (createdAt, expected) => {
    expect(age(createdAt, now)).toBe(expected);
  });
});
