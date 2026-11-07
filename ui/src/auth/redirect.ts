/**
 * The place to go back to after signing in, if `target` is a path on this site. Anything else
 * (another origin, a protocol-relative `//host`, a backslash trick) is ignored, so a sign-in
 * link cannot send a person elsewhere.
 */
export function safeRedirect(target: unknown): string | undefined {
  if (typeof target !== "string") return undefined;
  if (!target.startsWith("/") || target.startsWith("//") || target.includes("\\")) {
    return undefined;
  }
  return target;
}
