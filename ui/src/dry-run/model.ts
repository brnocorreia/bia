/** A Step a Mock can target: one that calls an Action, nested Blocks included. */
export interface MockableStep {
  id: string;
  action: string;
}

// Parts of a Definition whose objects are data: an `id` and `action` in them are not a Step's.
const NOT_STEPS = new Set(["params", "input_schema", "triggers", "limits"]);

/**
 * The Steps of a Definition that accept a Mock, in document order. `flow.*` Steps and Blocks
 * are left out because the API refuses a Mock on them (`invalid_mock`, ADR-0046).
 */
export function collectMockableSteps(definition: unknown): MockableStep[] {
  const found: MockableStep[] = [];
  const walk = (node: unknown) => {
    if (Array.isArray(node)) {
      node.forEach(walk);
    } else if (typeof node === "object" && node !== null) {
      const record = node as Record<string, unknown>;
      if (
        typeof record.id === "string" &&
        typeof record.action === "string" &&
        !record.action.startsWith("flow.")
      ) {
        found.push({ id: record.id, action: record.action });
      }
      for (const [key, value] of Object.entries(record)) {
        if (!NOT_STEPS.has(key)) walk(value);
      }
    }
  };
  if (typeof definition === "object" && definition !== null) {
    for (const [key, value] of Object.entries(definition)) {
      if (key === "steps" || key === "on_error") walk(value);
    }
  }
  return found;
}

/** What a user typed: the Run input and, per Step id, a Mock as JSON text in the API's format. */
export interface DryRunDraft {
  input: string;
  mocks: Record<string, string>;
}

export const EMPTY_DRAFT: DryRunDraft = { input: "{}", mocks: {} };

export type Built =
  { ok: true; input: unknown; mocks: Record<string, unknown> } | { ok: false; errors: string[] };

/** Parses the draft into a request body, naming every field that is not valid JSON. */
export function buildRequest(draft: DryRunDraft): Built {
  const errors: string[] = [];
  const parse = (text: string, label: string): unknown => {
    try {
      return JSON.parse(text);
    } catch {
      errors.push(`${label} is not valid JSON.`);
      return undefined;
    }
  };
  const input = draft.input.trim() === "" ? {} : parse(draft.input, "The input");
  const mocks: Record<string, unknown> = {};
  for (const [id, text] of Object.entries(draft.mocks)) {
    const value = parse(text, `The Mock for ${id}`);
    if (value !== undefined) mocks[id] = value;
  }
  return errors.length > 0 ? { ok: false, errors } : { ok: true, input, mocks };
}

const FAILURE = { class: "retryable", message: "Mocked failure" };

/** Starting points for a Mock, in the API's format. */
export const mockTemplates = {
  output: (output: unknown) => ({ output }),
  error: () => ({ error: FAILURE }),
  attempts: (output: unknown) => ({ attempts: [{ error: FAILURE }, { output }] }),
};

export const pretty = (value: unknown) => JSON.stringify(value, null, 2);

const storageKey = (workflowId: string | undefined) => `bia.dryRun.${workflowId ?? "(new)"}`;

/** The last draft for a Workflow in this browser; a convenience, so any failure means no draft. */
export function loadDraft(workflowId: string | undefined): DryRunDraft {
  try {
    const raw = window.localStorage.getItem(storageKey(workflowId));
    if (raw) {
      const parsed = JSON.parse(raw) as Partial<DryRunDraft>;
      if (typeof parsed.input === "string" && typeof parsed.mocks === "object" && parsed.mocks) {
        return { input: parsed.input, mocks: parsed.mocks };
      }
    }
  } catch {
    // Unavailable or corrupt storage: start empty.
  }
  return EMPTY_DRAFT;
}

export function saveDraft(workflowId: string | undefined, draft: DryRunDraft) {
  try {
    window.localStorage.setItem(storageKey(workflowId), JSON.stringify(draft));
  } catch {
    // Not remembered; nothing depends on it.
  }
}
