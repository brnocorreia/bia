import { describe, expect, it } from "vitest";
import { pointerGet } from "./json-pointer";

describe("pointerGet", () => {
  const doc = { steps: [{ id: "a" }, { id: "b" }], "a/b": { "m~n": 1 } };

  it("reads nested objects and arrays", () => {
    expect(pointerGet(doc, "/steps/1/id")).toBe("b");
  });
  it("unescapes ~1 and ~0", () => {
    expect(pointerGet(doc, "/a~1b/m~0n")).toBe(1);
  });
  it("returns the document for the empty pointer and undefined for a missing path", () => {
    expect(pointerGet(doc, "")).toBe(doc);
    expect(pointerGet(doc, "/steps/9/id")).toBeUndefined();
    expect(pointerGet(doc, "/nope/x")).toBeUndefined();
  });
});
