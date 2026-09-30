/**
 * Owns the clangd session for the editor: discovery, start/restart, document
 * sync, diagnostics, and the observable status the UI renders. Every failure
 * ends in a visible phase with a message; nothing fails silently.
 */
import { invoke } from "@tauri-apps/api/core";
import * as monaco from "monaco-editor/editor/editor.main";
import { isNative } from "../utils/native";
import { LspClient, type ExitInfo } from "./client";
import { editsForDocument, normalizeUri, sameUri, toLspRange, toMarker, toMonacoEdit } from "./convert";
import type * as lsp from "./protocol";
import { LANGUAGE, registerProviders } from "./providers";

export interface ClangdInfo {
  path: string;
  version: string;
  source: "settings" | "environment" | "path" | "knownLocation";
}
export interface ClangdStatus {
  available: boolean;
  clangd: ClangdInfo | null;
  configuredPath: string | null;
  problem: string | null;
  searched: string[];
}
interface LspSession {
  rootUri: string;
  documentUri: string;
  clangdVersion: string;
}

export type Phase = "unsupported" | "checking" | "unavailable" | "starting" | "ready" | "crashed" | "error";

export interface ClangdSnapshot {
  phase: Phase;
  /** Human-readable detail for every non-ready phase. */
  message: string | null;
  version: string | null;
  status: ClangdStatus | null;
  errors: number;
  warnings: number;
}

const INITIAL: ClangdSnapshot = { phase: "checking", message: null, version: null, status: null, errors: 0, warnings: 0 };
const MARKER_OWNER = "clangd";
const INITIALIZE_TIMEOUT_MS = 30_000;

const CLIENT_CAPABILITIES = {
  general: { positionEncodings: ["utf-16"] },
  workspace: { applyEdit: true, workspaceEdit: { documentChanges: true } },
  textDocument: {
    synchronization: { dynamicRegistration: false },
    completion: {
      contextSupport: true,
      completionItem: {
        snippetSupport: true,
        documentationFormat: ["markdown", "plaintext"],
        deprecatedSupport: true,
        tagSupport: { valueSet: [1] },
        labelDetailsSupport: true,
      },
    },
    hover: { contentFormat: ["markdown", "plaintext"] },
    signatureHelp: {
      contextSupport: true,
      signatureInformation: {
        documentationFormat: ["markdown", "plaintext"],
        parameterInformation: { labelOffsetSupport: true },
        activeParameterSupport: true,
      },
    },
    definition: { linkSupport: true },
    declaration: { linkSupport: true },
    typeDefinition: { linkSupport: true },
    implementation: { linkSupport: true },
    references: {},
    documentHighlight: {},
    documentSymbol: { hierarchicalDocumentSymbolSupport: true },
    rename: { prepareSupport: true },
    codeAction: {
      isPreferredSupport: true,
      codeActionLiteralSupport: {
        codeActionKind: { valueSet: ["quickfix", "refactor", "refactor.extract", "refactor.inline", "refactor.rewrite", "source"] },
      },
    },
    formatting: {},
    rangeFormatting: {},
    publishDiagnostics: { relatedInformation: true, tagSupport: { valueSet: [1, 2] }, versionSupport: true, codeDescriptionSupport: true },
  },
};

/** Everything created for one running server; disposed as a unit. */
interface Live {
  client: LspClient;
  model: monaco.editor.ITextModel;
  docUri: string;
  disposables: monaco.IDisposable[];
  externalModels: monaco.editor.ITextModel[];
  diagnostics: lsp.Diagnostic[];
}

const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));

export class ClangdController {
  private snapshot: ClangdSnapshot = INITIAL;
  private listeners = new Set<() => void>();
  private editor: monaco.editor.IStandaloneCodeEditor | null = null;
  private fileName = "main.cpp";
  private generation = 0;
  private live: Live | null = null;
  /** Start/stop steps run one at a time so they can never interleave. */
  private queue: Promise<void> = Promise.resolve();

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => void this.listeners.delete(listener);
  };
  getSnapshot = () => this.snapshot;

  private set(patch: Partial<ClangdSnapshot>) {
    this.snapshot = { ...this.snapshot, ...patch };
    this.listeners.forEach((l) => l());
  }

  /** Bind clangd to `editor`'s model. Returns the detach function. */
  attach(editor: monaco.editor.IStandaloneCodeEditor, fileName: string): () => void {
    this.editor = editor;
    this.fileName = fileName;
    this.restart();
    const opener = monaco.editor.registerEditorOpener({
      // Definitions outside the document (std headers) open as a peek, since
      // the standalone editor cannot switch files.
      openCodeEditor: (source, resource) => {
        if (source !== this.editor || !this.live || sameUri(resource.toString(), this.live.model.uri.toString())) return false;
        if (!this.live.externalModels.some((m) => m.uri.toString() === resource.toString())) return false;
        source.trigger("lattice", "editor.action.peekDefinition", null);
        return true;
      },
    });
    const onDispose = editor.onDidDispose(() => detach());
    const detach = () => {
      opener.dispose();
      onDispose.dispose();
      if (this.editor !== editor) return;
      this.editor = null;
      const gen = ++this.generation;
      this.enqueue(async () => {
        if (gen === this.generation) await this.teardown();
      });
    };
    return detach;
  }

  /** Stop and start clangd again (also picks up a changed path or a newly installed clangd). */
  restart() {
    const gen = ++this.generation;
    this.enqueue(async () => {
      await this.teardown();
      if (gen === this.generation) await this.start(gen);
    });
  }

  /** Save a new clangd path (`null` restores auto-discovery), then restart. Throws with the reason if rejected. */
  async configure(path: string | null): Promise<void> {
    await invoke<ClangdStatus>("clangd_set_path", { path });
    this.restart();
  }

  async rescan(): Promise<void> {
    if (isNative()) await invoke<ClangdStatus>("clangd_rescan");
    this.restart();
  }

  private enqueue(step: () => Promise<void>) {
    this.queue = this.queue.then(step).catch((e) => console.error("[clangd] lifecycle step failed", e));
  }

  private async start(gen: number) {
    const stale = () => gen !== this.generation;
    const editor = this.editor;
    const model = editor?.getModel();
    if (!editor || !model) return;
    if (!isNative()) {
      this.set({ ...INITIAL, phase: "unsupported", message: "Code intelligence needs the Lattice desktop app; it is not available in a plain browser." });
      return;
    }

    this.set({ ...INITIAL, phase: "checking" });
    let status: ClangdStatus;
    try {
      status = await invoke<ClangdStatus>("clangd_status");
    } catch (e) {
      if (!stale()) this.set({ phase: "error", message: `Could not check for clangd: ${errorText(e)}` });
      return;
    }
    if (stale()) return;
    if (!status.available) {
      this.set({ phase: "unavailable", status, message: status.problem ?? "clangd is not available." });
      return;
    }

    this.set({ phase: "starting", status, message: null, version: status.clangd?.version ?? null });
    const client = new LspClient((info) => this.onExit(gen, info));
    let session: LspSession;
    try {
      await client.ready();
      session = await invoke<LspSession>("clangd_start", { fileName: this.fileName });
    } catch (e) {
      client.dispose();
      if (!stale()) this.set({ phase: "error", message: errorText(e) });
      return;
    }
    if (stale()) return client.dispose();

    const live: Live = {
      client,
      model,
      docUri: session.documentUri,
      disposables: [],
      externalModels: [],
      diagnostics: [],
    };
    this.live = live;
    this.wireServerMessages(live, editor);

    try {
      const init = await client.request<{ capabilities: lsp.ServerCapabilities }>(
        "initialize",
        {
          processId: null,
          clientInfo: { name: "Lattice" },
          rootUri: session.rootUri,
          workspaceFolders: [{ uri: session.rootUri, name: "editor" }],
          capabilities: CLIENT_CAPABILITIES,
        },
        { timeoutMs: INITIALIZE_TIMEOUT_MS },
      );
      if (stale()) return;
      await client.notify("initialized", {});
      await client.notify("textDocument/didOpen", {
        textDocument: { uri: live.docUri, languageId: "cpp", version: model.getVersionId(), text: model.getValue() },
      });

      live.disposables.push(
        model.onDidChangeContent((e) => {
          void client.notify("textDocument/didChange", {
            textDocument: { uri: live.docUri, version: e.versionId },
            // Monaco orders changes so they can be applied one after another, as LSP requires.
            contentChanges: e.changes.map((c) => ({ range: toLspRange(c.range), rangeLength: c.rangeLength, text: c.text })),
          });
        }),
        ...registerProviders({
          monaco,
          client,
          model,
          docUri: live.docUri,
          caps: init?.capabilities ?? {},
          diagnostics: () => live.diagnostics,
          resolveLocationUri: (uri) => this.resolveLocation(live, uri),
        }),
      );
      // clangd's answers replace Monaco's word-guessing suggestions.
      editor.updateOptions({ wordBasedSuggestions: "off" });
      this.set({ phase: "ready", message: null, version: session.clangdVersion });
    } catch (e) {
      if (stale()) return;
      const message = `clangd failed to initialise: ${errorText(e)}`;
      await this.teardown();
      this.set({ phase: "error", message });
    }
  }

  private wireServerMessages(live: Live, editor: monaco.editor.IStandaloneCodeEditor) {
    const { client, model } = live;
    client.onNotification("textDocument/publishDiagnostics", (p: lsp.PublishDiagnosticsParams) => {
      if (!sameUri(p.uri, live.docUri)) return;
      // Diagnostics for an older text would land on the wrong lines; clangd republishes for the current one.
      if (p.version !== undefined && p.version !== model.getVersionId()) return;
      live.diagnostics = p.diagnostics;
      const resolve = (uri: string) => (sameUri(uri, live.docUri) ? model.uri : monaco.Uri.parse(uri));
      monaco.editor.setModelMarkers(model, MARKER_OWNER, p.diagnostics.map((d) => toMarker(monaco, model, d, resolve)));
      this.set({
        errors: p.diagnostics.filter((d) => d.severity === 1).length,
        warnings: p.diagnostics.filter((d) => d.severity === 2).length,
      });
    });
    client.onNotification("window/logMessage", (p: { message: string }) => console.debug("[clangd]", p.message));
    client.onNotification("window/showMessage", (p: { message: string }) => console.info("[clangd]", p.message));
    // Server-initiated edits (refactorings run through `workspace/executeCommand`).
    client.onRequest("workspace/applyEdit", (p: { edit: lsp.WorkspaceEdit }) => {
      const edits = editsForDocument(p.edit, live.docUri).map(toMonacoEdit);
      if (edits.length) {
        editor.executeEdits("clangd", edits.map((e) => ({ range: e.range, text: e.text })));
        editor.pushUndoStop();
      }
      return { applied: true };
    });
  }

  private async resolveLocation(live: Live, uri: string): Promise<monaco.Uri | null> {
    if (sameUri(uri, live.docUri)) return live.model.uri;
    const target = monaco.Uri.parse(uri);
    if (monaco.editor.getModel(target)) return target;
    try {
      const text = await invoke<string>("read_source_file", { uri });
      // These are views for peeking only; clangd never receives them.
      live.externalModels.push(monaco.editor.createModel(text, LANGUAGE, target));
      return target;
    } catch (e) {
      console.warn("[clangd] cannot open", normalizeUri(uri), errorText(e));
      return null;
    }
  }

  private onExit(gen: number, info: ExitInfo) {
    if (gen !== this.generation) return;
    const detail = info.stderr.trim();
    this.disposeLive();
    this.set({
      phase: "crashed",
      errors: 0,
      warnings: 0,
      message:
        `clangd stopped unexpectedly${info.code === null ? "" : ` (exit code ${info.code})`}.` + (detail ? `\n${detail}` : ""),
    });
  }

  private disposeLive() {
    const live = this.live;
    this.live = null;
    if (!live) return;
    live.disposables.forEach((d) => d.dispose());
    live.externalModels.forEach((m) => m.dispose());
    if (!live.model.isDisposed()) monaco.editor.setModelMarkers(live.model, MARKER_OWNER, []);
    this.editor?.updateOptions({ wordBasedSuggestions: "matchingDocuments" }); // Monaco default
    live.client.dispose();
  }

  private async teardown() {
    this.disposeLive();
    if (isNative()) await invoke("clangd_stop").catch(() => {});
    this.snapshot = { ...this.snapshot, errors: 0, warnings: 0 };
    this.listeners.forEach((l) => l());
  }
}

export const clangd = new ClangdController();
