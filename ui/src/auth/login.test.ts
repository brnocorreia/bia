import { afterEach, describe, expect, it, vi } from "vitest";
import { configureApi } from "@/api/client";
import { confirmSession, signIn } from "./login";
import { session } from "./session";

/** Answers each request in turn with `[status, body]`. */
function stubReplies(...replies: [number, unknown][]) {
  const seen: Request[] = [];
  vi.stubGlobal("fetch", (request: Request) => {
    seen.push(request);
    const [status, body] = replies.shift() ?? [500, {}];
    return Promise.resolve(
      new Response(JSON.stringify(body), {
        status,
        headers: { "content-type": "application/json" },
      }),
    );
  });
  configureApi("http://bia.test");
  return seen;
}

afterEach(() => {
  vi.unstubAllGlobals();
  session.signedOut();
});

describe("signIn", () => {
  it("is signed in once the Session cookie is accepted", async () => {
    const seen = stubReplies([201, {}], [200, { scope: "admin" }]);

    expect(await signIn("a password")).toEqual({ ok: true });

    expect(seen.map((r) => `${r.method} ${new URL(r.url).pathname}`)).toEqual([
      "POST /v1/session",
      "GET /v1/tokens/me",
    ]);
    expect(session.getState()).toBe("signed-in");
  });

  it("says a wrong password is wrong and stays signed out", async () => {
    stubReplies([401, { code: "invalid_credentials", message: "the password is wrong" }]);

    expect(await signIn("a password")).toMatchObject({ ok: false, kind: "credentials" });
    expect(session.getState()).not.toBe("signed-in");
  });

  it("explains the same-site rule when the cookie is dropped right after signing in", async () => {
    stubReplies([201, {}], [401, { code: "missing_token", message: "" }]);

    const result = await signIn("a password");

    expect(result).toMatchObject({ ok: false, kind: "same-site" });
    expect(!result.ok && result.message).toMatch(/same-site/);
  });

  it("says the API is unreachable when the request fails", async () => {
    vi.stubGlobal("fetch", () => Promise.reject(new TypeError("fetch failed")));
    configureApi("http://bia.test");

    expect(await signIn("a password")).toMatchObject({ ok: false, kind: "other" });
  });
});

describe("confirmSession", () => {
  it("reports a server error without calling it a cookie problem", async () => {
    stubReplies([500, { code: "internal_error", message: "" }]);

    expect(await confirmSession()).toMatchObject({ ok: false, kind: "other" });
  });
});
