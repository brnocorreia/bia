/** Reads the value at an RFC 6901 JSON Pointer, or `undefined` when the path does not exist. */
export function pointerGet(document: unknown, pointer: string): unknown {
  if (pointer === "") return document;
  let current: unknown = document;
  for (const raw of pointer.slice(1).split("/")) {
    const key = raw.replaceAll("~1", "/").replaceAll("~0", "~");
    if (Array.isArray(current)) current = current[Number(key)];
    else if (typeof current === "object" && current !== null)
      current = (current as Record<string, unknown>)[key];
    else return undefined;
  }
  return current;
}
