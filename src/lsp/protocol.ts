/** The slice of the Language Server Protocol that Lattice uses. */

export interface Position {
  line: number;
  character: number;
}
export interface Range {
  start: Position;
  end: Position;
}
export interface Location {
  uri: string;
  range: Range;
}
export interface LocationLink {
  originSelectionRange?: Range;
  targetUri: string;
  targetRange: Range;
  targetSelectionRange: Range;
}
export interface TextEdit {
  range: Range;
  newText: string;
}
export interface MarkupContent {
  kind: "plaintext" | "markdown";
  value: string;
}
export type MarkedString = string | { language: string; value: string };

export interface DiagnosticRelatedInformation {
  location: Location;
  message: string;
}
export interface Diagnostic {
  range: Range;
  severity?: 1 | 2 | 3 | 4;
  code?: string | number;
  codeDescription?: { href: string };
  source?: string;
  message: string;
  tags?: number[];
  relatedInformation?: DiagnosticRelatedInformation[];
  [extra: string]: unknown;
}
export interface PublishDiagnosticsParams {
  uri: string;
  version?: number;
  diagnostics: Diagnostic[];
}

export interface CompletionItem {
  label: string;
  labelDetails?: { detail?: string; description?: string };
  kind?: number;
  tags?: number[];
  detail?: string;
  documentation?: string | MarkupContent;
  deprecated?: boolean;
  preselect?: boolean;
  sortText?: string;
  filterText?: string;
  insertText?: string;
  insertTextFormat?: 1 | 2;
  textEdit?: TextEdit | { newText: string; insert: Range; replace: Range };
  additionalTextEdits?: TextEdit[];
}
export interface CompletionList {
  isIncomplete: boolean;
  items: CompletionItem[];
}

export interface ParameterInformation {
  label: string | [number, number];
  documentation?: string | MarkupContent;
}
export interface SignatureInformation {
  label: string;
  documentation?: string | MarkupContent;
  parameters?: ParameterInformation[];
  activeParameter?: number;
}
export interface SignatureHelp {
  signatures: SignatureInformation[];
  activeSignature?: number;
  activeParameter?: number;
}

export interface Hover {
  contents: string | MarkedString | MarkedString[] | MarkupContent;
  range?: Range;
}

export interface DocumentHighlight {
  range: Range;
  kind?: 1 | 2 | 3;
}

export interface DocumentSymbol {
  name: string;
  detail?: string;
  kind: number;
  tags?: number[];
  range: Range;
  selectionRange: Range;
  children?: DocumentSymbol[];
}
export interface SymbolInformation {
  name: string;
  kind: number;
  tags?: number[];
  containerName?: string;
  location: Location;
}

export interface WorkspaceEdit {
  changes?: Record<string, TextEdit[]>;
  documentChanges?: Array<{ textDocument: { uri: string }; edits: TextEdit[] } | { kind: string }>;
}
export interface Command {
  title: string;
  command: string;
  arguments?: unknown[];
}
export interface CodeAction {
  title: string;
  kind?: string;
  isPreferred?: boolean;
  edit?: WorkspaceEdit;
  command?: Command;
}

/** Server capabilities the client inspects (everything else is ignored). */
export interface ServerCapabilities {
  completionProvider?: { triggerCharacters?: string[] };
  hoverProvider?: boolean | object;
  signatureHelpProvider?: { triggerCharacters?: string[]; retriggerCharacters?: string[] };
  definitionProvider?: boolean | object;
  declarationProvider?: boolean | object;
  typeDefinitionProvider?: boolean | object;
  implementationProvider?: boolean | object;
  referencesProvider?: boolean | object;
  documentHighlightProvider?: boolean | object;
  documentSymbolProvider?: boolean | object;
  renameProvider?: boolean | { prepareProvider?: boolean };
  codeActionProvider?: boolean | object;
  documentFormattingProvider?: boolean | object;
  documentRangeFormattingProvider?: boolean | object;
  positionEncoding?: string;
}
