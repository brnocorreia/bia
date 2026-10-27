import { describe, expect, it } from "vitest";
import { ApiError } from "./request";
import { setupErrorMessage } from "./setup";

const refusal = (code: string, message = "") => new ApiError(403, { code, message } as never);

describe("setupErrorMessage", () => {
  it("tells a wrong code, an expired one and a finished setup apart", () => {
    expect(setupErrorMessage(refusal("invalid_setup_code"))).toMatch(/code is wrong/);
    expect(setupErrorMessage(refusal("setup_code_expired"))).toMatch(/before the server restarted/);
    expect(setupErrorMessage(refusal("already_set_up"))).toMatch(/--reset-admin/);
  });

  it("shows the API's own message for anything else, such as the password rule", () => {
    const short = refusal("password_too_short", "the password must be at least 12 characters");
    expect(setupErrorMessage(short)).toBe("the password must be at least 12 characters");
  });

  it("says the API is unreachable when there is no answer", () => {
    expect(setupErrorMessage(new TypeError("fetch failed"))).toMatch(/Cannot reach the API/);
  });
});
