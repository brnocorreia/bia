import { useEffect, useRef } from "react";
import type { Diagnostic } from "@/components/diagnostics-list";
import type { SyntaxError } from "./convert";
import { relaxForCompletion } from "./completion-schema";
import { monaco, MODEL_URI, setDefinitionSchema } from "./monaco";
import { rangeForPath, type Format } from "./path-range";

export interface EditorHandle {
  /** Moves the cursor to a Diagnostic's path and scrolls it into view. */
  reveal(path: string): void;
}

const MARKER_OWNER = "bia";

/**
 * Monaco bound to one text and a list of server Diagnostics. Controlled: `value` is the source
 * of truth and `onChange` reports edits. There is one model per format so completion follows the
 * JSON or YAML language service.
 */
export default function MonacoEditor({
  value,
  format,
  schema,
  diagnostics,
  syntaxError,
  onChange,
  onHandle,
}: {
  value: string;
  format: Format;
  schema: object | undefined;
  diagnostics: Diagnostic[];
  syntaxError: SyntaxError | null;
  onChange: (value: string) => void;
  onHandle?: (handle: EditorHandle) => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const editor = useRef<monaco.editor.IStandaloneCodeEditor | null>(null);
  const onChangeRef = useRef(onChange);
  useEffect(() => {
    onChangeRef.current = onChange;
  });

  useEffect(() => {
    if (!host.current) return;
    const instance = monaco.editor.create(host.current, {
      model: null,
      automaticLayout: true,
      minimap: { enabled: false },
      scrollBeyondLastLine: false,
      tabSize: 2,
      fixedOverflowWidgets: true,
      ariaLabel: "Definition editor",
    });
    editor.current = instance;
    const subscription = instance.onDidChangeModelContent(() => {
      onChangeRef.current(instance.getValue());
    });
    return () => {
      subscription.dispose();
      instance.dispose();
      for (const format of ["json", "yaml"] as const) {
        monaco.editor.getModel(MODEL_URI[format])?.dispose();
      }
      editor.current = null;
    };
  }, []);

  useEffect(() => {
    if (schema) setDefinitionSchema(relaxForCompletion(schema) as object);
  }, [schema]);

  // Swap to the model of `format`, and push `value` into it when it differs (a format switch
  // or a reload), without echoing the change back as an edit.
  useEffect(() => {
    const instance = editor.current;
    if (!instance) return;
    const uri = MODEL_URI[format];
    const model = monaco.editor.getModel(uri) ?? monaco.editor.createModel(value, format, uri);
    if (instance.getModel() !== model) instance.setModel(model);
    if (model.getValue() !== value) model.setValue(value);
  }, [format, value]);

  useEffect(() => {
    const model = monaco.editor.getModel(MODEL_URI[format]);
    if (!model) return;
    const markers: monaco.editor.IMarkerData[] = [];
    for (const d of diagnostics) {
      const range = rangeForPath(model.getValue(), format, d.path);
      if (!range) continue;
      const start = model.getPositionAt(range.start);
      const end = model.getPositionAt(Math.max(range.end, range.start + 1));
      markers.push({
        severity:
          d.severity === "error" ? monaco.MarkerSeverity.Error : monaco.MarkerSeverity.Warning,
        startLineNumber: start.lineNumber,
        startColumn: start.column,
        endLineNumber: end.lineNumber,
        endColumn: end.column,
        code: d.code,
        message: d.suggestion ? `${d.message}\nSuggestion: ${d.suggestion}` : d.message,
        source: "bia",
      });
    }
    if (syntaxError) {
      const start = model.getPositionAt(syntaxError.offset);
      const end = model.getPositionAt(syntaxError.offset + syntaxError.length);
      markers.push({
        severity: monaco.MarkerSeverity.Error,
        startLineNumber: start.lineNumber,
        startColumn: start.column,
        endLineNumber: end.lineNumber,
        endColumn: end.column,
        code: "syntax_error",
        message: syntaxError.message,
        source: "bia",
      });
    }
    monaco.editor.setModelMarkers(model, MARKER_OWNER, markers);
  }, [diagnostics, syntaxError, format, value]);

  useEffect(() => {
    onHandle?.({
      reveal(path) {
        const instance = editor.current;
        const model = instance?.getModel();
        if (!instance || !model) return;
        const range = rangeForPath(model.getValue(), format, path);
        if (!range) return;
        const position = model.getPositionAt(range.start);
        instance.setPosition(position);
        instance.revealPositionInCenter(position);
        instance.focus();
      },
    });
  }, [format, onHandle]);

  return <div ref={host} className="h-full min-h-[24rem] w-full" />;
}
