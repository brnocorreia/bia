// The browser's WebAuthn API speaks ArrayBuffers; the server's JSON speaks base64url strings.
// These pure functions convert between the two, so the ceremony code stays thin and testable.

/** Passkeys need WebAuthn in the browser, which only a secure context (HTTPS or localhost) has. */
export function passkeysSupported(
  env: { PublicKeyCredential?: unknown; isSecureContext?: boolean } = window,
): boolean {
  return env.PublicKeyCredential !== undefined && env.isSecureContext === true;
}

export const PASSKEYS_NEED_HTTPS =
  "Passkeys need this page to be served over HTTPS, or from localhost.";

export function decodeBase64Url(text: string): ArrayBuffer {
  const base64 = text.replace(/-/g, "+").replace(/_/g, "/");
  const binary = atob(base64.padEnd(Math.ceil(base64.length / 4) * 4, "="));
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes.buffer;
}

export function encodeBase64Url(buffer: ArrayBuffer): string {
  let binary = "";
  for (const byte of new Uint8Array(buffer)) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

interface CredentialDescriptorJson {
  id: string;
  [field: string]: unknown;
}

/** Drops `null`s, which the server never means as a value but the browser rejects. */
function withoutNulls<T>(value: T): T {
  if (Array.isArray(value)) return value.map(withoutNulls) as T;
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value)
        .filter(([, field]) => field !== null)
        .map(([key, field]) => [key, withoutNulls(field)]),
    ) as T;
  }
  return value;
}

function descriptors(list: CredentialDescriptorJson[] | undefined) {
  return list?.map((descriptor) => ({ ...descriptor, id: decodeBase64Url(descriptor.id) }));
}

/** The options `POST /v1/passkeys/registration/begin` returns, ready for `credentials.create`. */
export function creationOptions(json: unknown): PublicKeyCredentialCreationOptions {
  const { publicKey } = withoutNulls(json) as {
    publicKey: Record<string, unknown> & {
      challenge: string;
      user: { id: string };
      excludeCredentials?: CredentialDescriptorJson[];
    };
  };
  return {
    ...publicKey,
    challenge: decodeBase64Url(publicKey.challenge),
    user: { ...publicKey.user, id: decodeBase64Url(publicKey.user.id) },
    excludeCredentials: descriptors(publicKey.excludeCredentials),
  } as PublicKeyCredentialCreationOptions;
}

/** The options `POST /v1/session/passkey/begin` returns, ready for `credentials.get`. */
export function requestOptions(json: unknown): PublicKeyCredentialRequestOptions {
  const { publicKey } = withoutNulls(json) as {
    publicKey: Record<string, unknown> & {
      challenge: string;
      allowCredentials?: CredentialDescriptorJson[];
    };
  };
  return {
    ...publicKey,
    challenge: decodeBase64Url(publicKey.challenge),
    allowCredentials: descriptors(publicKey.allowCredentials),
  } as PublicKeyCredentialRequestOptions;
}

/** What `registration/finish` takes as `credential`. */
export function registrationJson(credential: PublicKeyCredential) {
  const response = credential.response as AuthenticatorAttestationResponse;
  return {
    id: credential.id,
    rawId: encodeBase64Url(credential.rawId),
    type: credential.type,
    response: {
      attestationObject: encodeBase64Url(response.attestationObject),
      clientDataJSON: encodeBase64Url(response.clientDataJSON),
      transports: response.getTransports?.(),
    },
    extensions: credential.getClientExtensionResults(),
  };
}

/** What `session/passkey/finish` takes as `credential`. */
export function assertionJson(credential: PublicKeyCredential) {
  const response = credential.response as AuthenticatorAssertionResponse;
  return {
    id: credential.id,
    rawId: encodeBase64Url(credential.rawId),
    type: credential.type,
    response: {
      authenticatorData: encodeBase64Url(response.authenticatorData),
      clientDataJSON: encodeBase64Url(response.clientDataJSON),
      signature: encodeBase64Url(response.signature),
      userHandle: response.userHandle ? encodeBase64Url(response.userHandle) : undefined,
    },
    extensions: credential.getClientExtensionResults(),
  };
}
