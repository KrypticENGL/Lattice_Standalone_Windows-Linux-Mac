/**
 * Pure LSP <-> Monaco conversions. LSP positions are UTF-16 code units, like
 * Monaco columns, so only the 0/1 base differs.
 */
import type * as monaco from "monaco-editor";
import type * as lsp from "./protocol";

type Monaco = typeof monaco;

export const toMonacoRange = (r: lsp.Range): monaco.IRange => ({
  startLineNumber: r.start.line + 1,
  startColumn: r.start.character + 1,
  endLineNumber: r.end.line + 1,
  endColumn: r.end.character + 1,
});

export const toLspRange = (r: monaco.IRange): lsp.Range => ({
  start: { line: r.startLineNumber - 1, character: r.startColumn - 1 },
  end: { line: r.endLineNumber - 1, character: r.endColumn - 1 },
});

export const toLspPosition = (p: monaco.IPosition): lsp.Position => ({
  line: p.lineNumber - 1,
  character: p.column - 1,
});

/** Compare `file://` URIs the way Windows sees them (clangd may lowercase the drive or encode `:`). */
export function normalizeUri(uri: string): string {
  let s = uri;
  try {
    s = decodeURIComponent(uri);
  } catch {
    /* keep raw */
  }
  return s.replace(/^file:\/+/i, "").replace(/\\/g, "/").toLowerCase();
}

export const sameUri = (a: string, b: string) => normalizeUri(a) === normalizeUri(b);

export function toMarkdown(doc: string | lsp.MarkupContent | undefined): string | monaco.IMarkdownString | undefined {
  if (doc === undefined || doc === null) return undefined;
  if (typeof doc === "string") return doc || undefined;
  if (!doc.value) return undefined;
  return doc.kind === "markdown" ? { value: doc.value } : doc.value;
}

/** Hover contents in any of the LSP's shapes -> markdown strings. */
export function hoverContents(contents: lsp.Hover["contents"]): monaco.IMarkdownString[] {
  const items = Array.isArray(contents) ? contents : [contents];
  const out: monaco.IMarkdownString[] = [];
  for (const c of items) {
    if (typeof c === "string") {
      if (c) out.push({ value: c });
    } else if ("kind" in c) {
      if (c.value) out.push(c.kind === "markdown" ? { value: c.value } : { value: "```text\n" + c.value + "\n```" });
    } else if (c.value) {
      out.push({ value: "```" + c.language + "\n" + c.value + "\n```" });
    }
  }
  return out;
}

export function completionKind(m: Monaco, kind: number | undefined): monaco.languages.CompletionItemKind {
  const K = m.languages.CompletionItemKind;
  const map: Record<number, monaco.languages.CompletionItemKind> = {
    1: K.Text, 2: K.Method, 3: K.Function, 4: K.Constructor, 5: K.Field, 6: K.Variable, 7: K.Class,
    8: K.Interface, 9: K.Module, 10: K.Property, 11: K.Unit, 12: K.Value, 13: K.Enum, 14: K.Keyword,
    15: K.Snippet, 16: K.Color, 17: K.File, 18: K.Reference, 19: K.Folder, 20: K.EnumMember,
    21: K.Constant, 22: K.Struct, 23: K.Event, 24: K.Operator, 25: K.TypeParameter,
  };
  return (kind !== undefined && map[kind]) || K.Text;
}

export function severity(m: Monaco, s: lsp.Diagnostic["severity"]): monaco.MarkerSeverity {
  switch (s) {
    case 1: return m.MarkerSeverity.Error;
    case 2: return m.MarkerSeverity.Warning;
    case 4: return m.MarkerSeverity.Hint;
    default: return m.MarkerSeverity.Info;
  }
}

/** Zero-width ranges draw nothing in Monaco, so widen them to the word or one character. */
export function visibleRange(model: monaco.editor.ITextModel, r: monaco.IRange): monaco.IRange {
  if (r.startLineNumber !== r.endLineNumber || r.startColumn !== r.endColumn) return r;
  const w = model.getWordAtPosition({ lineNumber: r.startLineNumber, column: r.startColumn });
  if (w) return { ...r, startColumn: w.startColumn, endColumn: w.endColumn };
  const max = model.getLineMaxColumn(r.startLineNumber);
  return r.startColumn < max
    ? { ...r, endColumn: r.startColumn + 1 }
    : { ...r, startColumn: Math.max(1, r.startColumn - 1) };
}

export function toMarker(
  m: Monaco,
  model: monaco.editor.ITextModel,
  d: lsp.Diagnostic,
  resolveUri: (uri: string) => monaco.Uri,
): monaco.editor.IMarkerData {
  const code = d.code === undefined ? undefined : String(d.code);
  return {
    ...visibleRange(model, toMonacoRange(d.range)),
    severity: severity(m, d.severity),
    message: d.message,
    source: d.source ?? "clangd",
    code: code && d.codeDescription?.href ? { value: code, target: m.Uri.parse(d.codeDescription.href) } : code,
    tags: (d.tags ?? []).flatMap((t) =>
      t === 1 ? [m.MarkerTag.Unnecessary] : t === 2 ? [m.MarkerTag.Deprecated] : [],
    ),
    relatedInformation: d.relatedInformation?.map((r) => ({
      resource: resolveUri(r.location.uri),
      message: r.message,
      ...toMonacoRange(r.location.range),
    })),
  };
}

/** Text edits that apply to `docUri`, from either WorkspaceEdit shape. Edits to other files are dropped. */
export function editsForDocument(edit: lsp.WorkspaceEdit | undefined, docUri: string): lsp.TextEdit[] {
  if (!edit) return [];
  const out: lsp.TextEdit[] = [];
  for (const [uri, edits] of Object.entries(edit.changes ?? {})) {
    if (sameUri(uri, docUri)) out.push(...edits);
  }
  for (const c of edit.documentChanges ?? []) {
    if ("edits" in c && sameUri(c.textDocument.uri, docUri)) out.push(...c.edits);
  }
  return out;
}

export const toMonacoEdit = (e: lsp.TextEdit): monaco.languages.TextEdit => ({
  range: toMonacoRange(e.range),
  text: e.newText,
});
