/**
 * The Definition schema as the editor's language service should see it. Completion offers only
 * the `oneOf` variants a half-written object could still satisfy, and a Trigger with no `expr`
 * yet fails the `cron` variant, so `cron` would never be proposed for `type`. Dropping
 * `required` keeps every variant eligible. Validation is the server's job (its Diagnostics are
 * the only ones shown), so nothing is lost.
 */
export function relaxForCompletion(schema: unknown): unknown {
  if (Array.isArray(schema)) return schema.map(relaxForCompletion);
  if (typeof schema !== "object" || schema === null) return schema;
  const relaxed: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(schema)) {
    // `required` is the keyword only where its value is a list of names; a property called
    // "required" inside `properties` holds a sub-schema, which is an object.
    if (key === "required" && Array.isArray(value)) continue;
    relaxed[key] = relaxForCompletion(value);
  }
  return relaxed;
}
