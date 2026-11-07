import { describe, expect, it } from "vitest";
import {
  assertionJson,
  creationOptions,
  decodeBase64Url,
  encodeBase64Url,
  passkeysSupported,
  registrationJson,
  requestOptions,
} from "./webauthn";

const bytes = (...values: number[]) => new Uint8Array(values).buffer;

describe("base64url", () => {
  it("round-trips bytes, with the URL alphabet and no padding", () => {
    const original = bytes(251, 255, 190, 0, 1);

    const text = encodeBase64Url(original);

    expect(text).toBe("-_--AAE");
    expect(new Uint8Array(decodeBase64Url(text))).toEqual(new Uint8Array(original));
  });
});

describe("passkeysSupported", () => {
  it("needs WebAuthn and a secure context", () => {
    expect(passkeysSupported({ PublicKeyCredential: class {}, isSecureContext: true })).toBe(true);
    expect(passkeysSupported({ PublicKeyCredential: class {}, isSecureContext: false })).toBe(
      false,
    );
    expect(passkeysSupported({ isSecureContext: true })).toBe(false);
  });
});

describe("options", () => {
  it("decodes the challenge, the user id and the excluded credentials", () => {
    const options = creationOptions({
      publicKey: {
        challenge: "AQID",
        rp: { name: "bia", id: "localhost" },
        user: { id: "BAUG", name: "admin", displayName: "bia admin" },
        excludeCredentials: [{ type: "public-key", id: "BwgJ" }],
        hints: null,
      },
    });

    expect(new Uint8Array(options.challenge as ArrayBuffer)).toEqual(new Uint8Array([1, 2, 3]));
    expect(new Uint8Array(options.user.id as ArrayBuffer)).toEqual(new Uint8Array([4, 5, 6]));
    expect(new Uint8Array(options.excludeCredentials![0]!.id as ArrayBuffer)).toEqual(
      new Uint8Array([7, 8, 9]),
    );
    expect(options.rp.id).toBe("localhost");
    expect("hints" in options).toBe(false);
  });

  it("decodes the challenge and the allowed credentials of a sign-in", () => {
    const options = requestOptions({
      publicKey: {
        challenge: "AQID",
        rpId: "localhost",
        allowCredentials: [{ type: "public-key", id: "BwgJ" }],
      },
    });

    expect(new Uint8Array(options.challenge as ArrayBuffer)).toEqual(new Uint8Array([1, 2, 3]));
    expect(new Uint8Array(options.allowCredentials![0]!.id as ArrayBuffer)).toEqual(
      new Uint8Array([7, 8, 9]),
    );
    expect(options.rpId).toBe("localhost");
  });
});

describe("credentials", () => {
  const common = {
    id: "AQID",
    rawId: bytes(1, 2, 3),
    type: "public-key",
    getClientExtensionResults: () => ({ credProps: { rk: true } }),
  };

  it("encodes a registration as the server takes it", () => {
    const json = registrationJson({
      ...common,
      response: {
        attestationObject: bytes(4),
        clientDataJSON: bytes(5),
        getTransports: () => ["internal"],
      },
    } as unknown as PublicKeyCredential);

    expect(json).toEqual({
      id: "AQID",
      rawId: "AQID",
      type: "public-key",
      response: { attestationObject: "BA", clientDataJSON: "BQ", transports: ["internal"] },
      extensions: { credProps: { rk: true } },
    });
  });

  it("encodes an assertion, leaving out a missing user handle", () => {
    const json = assertionJson({
      ...common,
      response: {
        authenticatorData: bytes(6),
        clientDataJSON: bytes(5),
        signature: bytes(7),
        userHandle: null,
      },
    } as unknown as PublicKeyCredential);

    expect(json.response).toEqual({
      authenticatorData: "Bg",
      clientDataJSON: "BQ",
      signature: "Bw",
      userHandle: undefined,
    });
  });
});
