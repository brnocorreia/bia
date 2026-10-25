import { parse as parseJsonc, printParseErrorCode, type ParseError } from "jsonc-parser";
import { isNode, parseDocument, stringify, visit } from "yaml";
import type { Format } from "./path-range";

export interface SyntaxError {
  message: string;
  /** Offset of the problem in the text. */
  offset: number;
  length: number;
}

export type Parsed = { ok: true; value: unknown } | { ok: false; error: SyntaxError };

/** Parses editor text. JSON is strict: the server accepts only JSON, so no comments or commas. */
export function parseText(text: string, format: Format): Parsed {
  if (format === "json") {
    const errors: ParseError[] = [];
    const value = parseJsonc(text, errors, { allowTrailingComma: false, disallowComments: true });
    const first = errors[0];
    if (first) {
      return {
        ok: false,
        error: {
          message: printParseErrorCode(first.error),
          offset: first.offset,
          length: Math.max(1, first.length),
        },
      };
    }
    return { ok: true, value };
  }
  const doc = parseDocument(text);
  const first = doc.errors[0];
  if (first) {
    return {
      ok: false,
      error: {
        message: first.message.split("\n")[0] ?? first.message,
        offset: first.pos[0],
        length: Math.max(1, first.pos[1] - first.pos[0]),
      },
    };
  }
  return { ok: true, value: doc.toJS() };
}

/** Renders a Definition in the editor's format. */
export function formatText(value: unknown, format: Format): string {
  return format === "json" ? `${JSON.stringify(value, null, 2)}\n` : stringify(value);
}

/**
 * Whether YAML text carries comments. The server stores JSON only, so a comment cannot survive
 * saving or switching to JSON; the editor warns instead of dropping them silently.
 */
export function yamlHasComments(text: string): boolean {
  const doc = parseDocument(text);
  let found = Boolean(doc.commentBefore || doc.comment);
  visit(doc, (_key, node) => {
    if (isNode(node) && (node.comment || node.commentBefore)) found = true;
  });
  return found || /^\s*#/m.test(text);
}
