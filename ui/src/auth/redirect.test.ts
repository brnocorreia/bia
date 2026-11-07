import { describe, expect, it } from "vitest";
import { safeRedirect } from "./redirect";

describe("safeRedirect", () => {
  it("keeps a path on this site, with its query", () => {
    expect(safeRedirect("/oauth/consent?request=abc")).toBe("/oauth/consent?request=abc");
  });

  it("drops anything that could leave the site", () => {
    for (const target of [
      "https://evil.example/",
      "//evil.example",
      "/\\evil.example",
      "oauth",
      4,
    ]) {
      expect(safeRedirect(target)).toBeUndefined();
    }
  });
});
