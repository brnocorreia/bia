import { describe, expect, it } from "vitest";
import { pointerSegments, rangeForPath } from "./path-range";

const definition = {
  schema_version: 1,
  id: "demo",
  steps: [
    { id: "a", action: "flow.sleep", params: { duration: "1s" } },
    {
      id: "b",
      action: "flow.if",
      params: { condition: "true", then: [{ id: "c", action: "http.get", params: {} }] },
    },
  ],
};
const json = JSON.stringify(definition, null, 2);
const yaml = `schema_version: 1
id: demo
steps:
  - id: a
    action: flow.sleep
    params:
      duration: 1s
  - id: b
    action: flow.if
    params:
      condition: "true"
      then:
        - id: c
          action: http.get
          params: {}
`;

const slice = (text: string, range: { start: number; end: number } | null) =>
  range && text.slice(range.start, range.end);

describe("pointerSegments", () => {
  it("decodes escapes", () => {
    expect(pointerSegments("/a~1b/c~0d/0")).toEqual(["a/b", "c~d", "0"]);
    expect(pointerSegments("")).toEqual([]);
  });
});

describe.each([
  ["json", json],
  ["yaml", yaml],
] as const)("rangeForPath in %s", (format, text) => {
  it("finds a top-level value", () => {
    expect(slice(text, rangeForPath(text, format, "/id"))).toMatch(/^"?demo"?$/);
  });

  it("finds a value inside a Step", () => {
    expect(slice(text, rangeForPath(text, format, "/steps/0/action"))).toMatch(/^"?flow\.sleep"?$/);
  });

  it("finds a Step in a nested Block", () => {
    const range = rangeForPath(text, format, "/steps/1/params/then/0/action");
    expect(slice(text, range)).toMatch(/^"?http\.get"?$/);
  });

  it("selects a whole Step for the Step's own path", () => {
    const selected = slice(text, rangeForPath(text, format, "/steps/1"));
    expect(selected).toContain("flow.if");
    expect(selected).toContain("http.get");
    expect(selected).not.toContain("flow.sleep");
  });

  it("falls back to the nearest ancestor for a missing field", () => {
    const range = rangeForPath(text, format, "/steps/0/params/missing");
    expect(slice(text, range)).toContain("duration");
  });

  it("selects the whole document for the empty path", () => {
    const range = rangeForPath(text, format, "");
    expect(range?.start).toBeLessThanOrEqual(1);
    expect(slice(text, range)).toContain("steps");
  });

  it("returns null for text that does not parse", () => {
    expect(rangeForPath(format === "json" ? "{ nope" : "a: [", format, "/id")).toBeNull();
  });
});
