import { describe, expect, it } from "vitest";
import { buildRequest, collectMockableSteps } from "./model";

describe("collectMockableSteps", () => {
  it("finds Action Steps in Blocks and on_error, and skips flow.* Steps and params", () => {
    const steps = collectMockableSteps({
      id: "w",
      triggers: [{ id: "t", type: "manual" }],
      steps: [
        { id: "fetch", action: "http.request", params: { url: "x" } },
        { id: "wait", action: "flow.sleep", params: { duration: "1s" } },
        {
          id: "pick",
          action: "flow.branch",
          when: "true",
          then: [{ id: "inner", action: "transform.map", params: { id: "a", action: "b" } }],
        },
      ],
      on_error: [{ id: "alert", action: "http.request", params: {} }],
    });
    expect(steps.map((s) => s.id)).toEqual(["fetch", "inner", "alert"]);
  });

  it("returns nothing for an unparsed Definition", () => {
    expect(collectMockableSteps(undefined)).toEqual([]);
  });
});

describe("buildRequest", () => {
  it("keeps Mocks in the API's format", () => {
    const built = buildRequest({ input: '{"a":1}', mocks: { s: '{"output":{"x":2}}' } });
    expect(built).toEqual({ ok: true, input: { a: 1 }, mocks: { s: { output: { x: 2 } } } });
  });

  it("names every field that is not JSON", () => {
    const built = buildRequest({ input: "{", mocks: { s: "nope" } });
    expect(built).toEqual({
      ok: false,
      errors: ["The input is not valid JSON.", "The Mock for s is not valid JSON."],
    });
  });
});
