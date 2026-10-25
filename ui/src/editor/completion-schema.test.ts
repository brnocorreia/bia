import { describe, expect, it } from "vitest";
import { relaxForCompletion } from "./completion-schema";

describe("relaxForCompletion", () => {
  it("drops required lists at every depth, including inside oneOf", () => {
    const schema = {
      type: "object",
      required: ["id"],
      properties: { required: { type: "boolean" } },
      oneOf: [{ required: ["expr"], properties: { type: { const: "cron" } } }],
    };
    expect(relaxForCompletion(schema)).toEqual({
      type: "object",
      properties: { required: { type: "boolean" } },
      oneOf: [{ properties: { type: { const: "cron" } } }],
    });
  });
});
