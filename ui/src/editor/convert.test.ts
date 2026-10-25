import { describe, expect, it } from "vitest";
import { formatText, parseText, yamlHasComments } from "./convert";

const definition = {
  schema_version: 1,
  id: "demo",
  steps: [{ id: "a", action: "flow.sleep", params: { duration: "1s" } }],
};

describe("switching format", () => {
  it("keeps the Definition through JSON to YAML and back", () => {
    const yaml = formatText(definition, "yaml");
    const fromYaml = parseText(yaml, "yaml");
    expect(fromYaml).toEqual({ ok: true, value: definition });
    const json = formatText((fromYaml as { value: unknown }).value, "json");
    expect(parseText(json, "json")).toEqual({ ok: true, value: definition });
  });
});

describe("parseText", () => {
  it("reports where JSON is broken", () => {
    const parsed = parseText('{ "a": }', "json");
    expect(parsed.ok).toBe(false);
    if (!parsed.ok) expect(parsed.error.offset).toBeGreaterThan(0);
  });

  it("refuses JSON comments and trailing commas", () => {
    expect(parseText('{ "a": 1, }', "json").ok).toBe(false);
    expect(parseText('// no\n{ "a": 1 }', "json").ok).toBe(false);
  });

  it("reports where YAML is broken", () => {
    expect(parseText("a: [", "yaml").ok).toBe(false);
  });
});

describe("yamlHasComments", () => {
  it("detects leading, inline and nested comments", () => {
    expect(yamlHasComments("# top\nid: a\n")).toBe(true);
    expect(yamlHasComments("id: a # why\n")).toBe(true);
    expect(yamlHasComments("steps:\n  # first\n  - id: a\n")).toBe(true);
  });

  it("is false for plain YAML", () => {
    expect(yamlHasComments("id: a\nsteps:\n  - id: b\n")).toBe(false);
  });
});
