import { describe, expect, it } from "vitest";
import { DEFAULT_CONFIG, loadConfig, parseConfig } from "./config";

const reply = (body: string, init: ResponseInit = {}) =>
  (() =>
    Promise.resolve(
      new Response(body, { headers: { "content-type": "application/json" }, ...init }),
    )) as typeof fetch;

describe("config.json", () => {
  it("takes the API URL from the file and drops a trailing slash", async () => {
    const config = await loadConfig(reply('{"apiUrl":"https://bia.example.com/"}'));
    expect(config.apiUrl).toBe("https://bia.example.com");
  });

  it("allows an empty API URL for an API that serves the UI itself", () => {
    expect(parseConfig({ apiUrl: "" }).apiUrl).toBe("");
  });

  it("falls back to the local default when the file is missing", async () => {
    expect(await loadConfig(reply("", { status: 404 }))).toEqual(DEFAULT_CONFIG);
  });

  it("falls back when a static host answers with its HTML fallback page", async () => {
    const html = reply("<html></html>", { headers: { "content-type": "text/html" } });
    expect(await loadConfig(html)).toEqual(DEFAULT_CONFIG);
  });

  it("fails loudly on a malformed file", async () => {
    await expect(loadConfig(reply('{"apiUrl":3}'))).rejects.toThrow("apiUrl");
  });
});
