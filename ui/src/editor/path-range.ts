import { parseTree, type Node as JsonNode } from "jsonc-parser";
import { isMap, isNode, isPair, isScalar, isSeq, parseDocument } from "yaml";

export type Format = "json" | "yaml";

/** Offsets into the editor text; `end` is exclusive. */
export interface TextRange {
  start: number;
  end: number;
}

/** Decodes an RFC 6901 JSON Pointer into its path segments. */
export function pointerSegments(pointer: string): string[] {
  if (pointer === "") return [];
  return pointer
    .slice(1)
    .split("/")
    .map((raw) => raw.replaceAll("~1", "/").replaceAll("~0", "~"));
}

/**
 * Finds the text a Diagnostic's JSON Pointer points at. A path that does not exist yet (a
 * missing field) falls back to its nearest existing ancestor, so a Diagnostic always lands
 * somewhere. Returns `null` when the text does not parse at all.
 */
export function rangeForPath(text: string, format: Format, pointer: string): TextRange | null {
  const segments = pointerSegments(pointer);
  const locate = format === "json" ? jsonLocator(text) : yamlLocator(text);
  if (!locate) return null;
  for (let depth = segments.length; depth >= 0; depth--) {
    const found = locate(segments.slice(0, depth));
    if (found) return found;
  }
  return null;
}

function jsonLocator(text: string): ((path: string[]) => TextRange | null) | null {
  const errors: unknown[] = [];
  const root = parseTree(text, errors as never[]);
  if (!root || errors.length > 0) return null;
  return (path) => {
    let node: JsonNode | undefined = root;
    for (const segment of path) {
      if (!node) return null;
      if (node.type === "array") node = node.children?.[Number(segment)];
      else if (node.type === "object") {
        node = node.children?.find((property) => property.children?.[0]?.value === segment)
          ?.children?.[1];
      } else return null;
    }
    return node ? { start: node.offset, end: node.offset + node.length } : null;
  };
}

function yamlLocator(text: string): ((path: string[]) => TextRange | null) | null {
  const doc = parseDocument(text);
  if (doc.errors.length > 0 || !doc.contents) return null;
  return (path) => {
    let node: unknown = doc.contents;
    for (const segment of path) {
      if (isSeq(node)) node = node.items[Number(segment)];
      else if (isMap(node)) {
        const pair = node.items.find(
          (item) => isPair(item) && isScalar(item.key) && String(item.key.value) === segment,
        );
        node = pair?.value;
      } else return null;
    }
    if (!isNode(node) || !node.range) return null;
    const [start, valueEnd] = node.range;
    // A block node's range runs to the start of the next line; stop at its last content.
    let end = valueEnd;
    while (end > start && /\s/.test(text.charAt(end - 1))) end--;
    return { start, end };
  };
}
