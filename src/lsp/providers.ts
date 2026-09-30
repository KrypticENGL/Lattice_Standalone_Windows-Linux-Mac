/**
 * Registers Monaco language-feature providers that forward to clangd.
 * Only capabilities the server advertises are registered, and only the
 * editor's own document is served (models opened for definition peeks in
 * headers are read-only views that clangd has never seen).
 */
import type * as monaco from "monaco-editor";
import type { LspClient } from "./client";
import * as cv from "./convert";
import type * as lsp from "./protocol";

type Monaco = typeof monaco;

export const LANGUAGE = "cpp";
export const EXECUTE_COMMAND_ID = "lattice.clangd.executeCommand";

export interface ProviderContext {
  monaco: Monaco;
  client: LspClient;
  model: monaco.editor.ITextModel;
  docUri: string;
  caps: lsp.ServerCapabilities;
  /** Diagnostics clangd last published for the document. */
  diagnostics: () => lsp.Diagnostic[];
  /** Make an LSP URI openable in Monaco (creating a read-only model for files outside the editor). */
  resolveLocationUri: (uri: string) => Promise<monaco.Uri | null>;
}

const docId = (ctx: ProviderContext) => ({ uri: ctx.docUri });
const posParams = (ctx: ProviderContext, p: monaco.IPosition) => ({
  textDocument: docId(ctx),
  position: cv.toLspPosition(p),
});
const enabled = (cap: unknown) => !!cap;

export function registerProviders(ctx: ProviderContext): monaco.IDisposable[] {
  const { monaco: m, client, caps } = ctx;
  const own = (model: monaco.editor.ITextModel) => model === ctx.model;
  const out: monaco.IDisposable[] = [];
  const add = (d: monaco.IDisposable) => out.push(d);

  // ---- completion -------------------------------------------------------
  if (caps.completionProvider) {
    add(m.languages.registerCompletionItemProvider(LANGUAGE, {
      triggerCharacters: caps.completionProvider.triggerCharacters,
      async provideCompletionItems(model, position, context, token) {
        if (!own(model)) return undefined;
        const res = await client.request<lsp.CompletionItem[] | lsp.CompletionList>(
          "textDocument/completion",
          {
            ...posParams(ctx, position),
            // Monaco: 0 invoke, 1 trigger character, 2 incomplete re-query. LSP is 1-based.
            context: { triggerKind: context.triggerKind + 1, triggerCharacter: context.triggerCharacter },
          },
          { token },
        );
        if (!res) return undefined;
        const list = Array.isArray(res) ? { isIncomplete: false, items: res } : res;
        const word = model.getWordUntilPosition(position);
        const fallback: monaco.IRange = {
          startLineNumber: position.lineNumber,
          endLineNumber: position.lineNumber,
          startColumn: word.startColumn,
          endColumn: word.endColumn,
        };
        return {
          incomplete: list.isIncomplete,
          suggestions: list.items.map((it): monaco.languages.CompletionItem => {
            let range: monaco.IRange | monaco.languages.CompletionItemRanges = fallback;
            let text = it.insertText ?? it.label;
            const te = it.textEdit;
            if (te) {
              text = te.newText;
              range = "range" in te
                ? cv.toMonacoRange(te.range)
                : { insert: cv.toMonacoRange(te.insert), replace: cv.toMonacoRange(te.replace) };
            }
            const ld = it.labelDetails;
            return {
              label: ld ? { label: it.label, detail: ld.detail, description: ld.description } : it.label,
              kind: cv.completionKind(m, it.kind),
              detail: it.detail,
              documentation: cv.toMarkdown(it.documentation),
              tags: it.tags?.includes(1) || it.deprecated ? [m.languages.CompletionItemTag.Deprecated] : undefined,
              sortText: it.sortText,
              filterText: it.filterText,
              preselect: it.preselect,
              insertText: text,
              insertTextRules: it.insertTextFormat === 2
                ? m.languages.CompletionItemInsertTextRule.InsertAsSnippet
                : undefined,
              range,
              additionalTextEdits: it.additionalTextEdits?.map(cv.toMonacoEdit),
            };
          }),
        };
      },
    }));
  }

  // ---- hover ------------------------------------------------------------
  if (enabled(caps.hoverProvider)) {
    add(m.languages.registerHoverProvider(LANGUAGE, {
      async provideHover(model, position, token) {
        if (!own(model)) return undefined;
        const res = await client.request<lsp.Hover>("textDocument/hover", posParams(ctx, position), { token });
        const contents = res ? cv.hoverContents(res.contents) : [];
        if (!res || contents.length === 0) return undefined;
        return { contents, range: res.range ? cv.toMonacoRange(res.range) : undefined };
      },
    }));
  }

  // ---- signature help ---------------------------------------------------
  if (caps.signatureHelpProvider) {
    add(m.languages.registerSignatureHelpProvider(LANGUAGE, {
      signatureHelpTriggerCharacters: caps.signatureHelpProvider.triggerCharacters,
      signatureHelpRetriggerCharacters: caps.signatureHelpProvider.retriggerCharacters,
      async provideSignatureHelp(model, position, token, context) {
        if (!own(model)) return undefined;
        const res = await client.request<lsp.SignatureHelp>(
          "textDocument/signatureHelp",
          {
            ...posParams(ctx, position),
            context: {
              triggerKind: context.triggerKind, // Monaco and LSP agree: 1 invoke, 2 trigger char, 3 content change
              triggerCharacter: context.triggerCharacter,
              isRetrigger: context.isRetrigger,
            },
          },
          { token },
        );
        if (!res || res.signatures.length === 0) return undefined;
        const activeSignature = res.activeSignature ?? 0;
        return {
          value: {
            activeSignature,
            activeParameter: res.signatures[activeSignature]?.activeParameter ?? res.activeParameter ?? 0,
            signatures: res.signatures.map((s) => ({
              label: s.label,
              documentation: cv.toMarkdown(s.documentation),
              activeParameter: s.activeParameter,
              parameters: (s.parameters ?? []).map((p) => ({
                label: p.label,
                documentation: cv.toMarkdown(p.documentation),
              })),
            })),
          },
          dispose() {},
        };
      },
    }));
  }

  // ---- navigation -------------------------------------------------------
  const locations = async (
    method: string,
    params: object,
    token: monaco.CancellationToken,
  ): Promise<monaco.languages.Location[] | undefined> => {
    const res = await client.request<lsp.Location | lsp.Location[] | lsp.LocationLink[]>(method, params, { token });
    if (!res) return undefined;
    const list = Array.isArray(res) ? res : [res];
    const resolved = await Promise.all(
      list.map(async (l): Promise<monaco.languages.Location | null> => {
        const link = "targetUri" in l;
        const uri = await ctx.resolveLocationUri(link ? l.targetUri : l.uri);
        if (!uri) return null;
        return { uri, range: cv.toMonacoRange(link ? l.targetSelectionRange : l.range) };
      }),
    );
    return resolved.filter((l): l is monaco.languages.Location => l !== null);
  };

  const navigation: Array<[unknown, string, (h: (model: monaco.editor.ITextModel, pos: monaco.Position, token: monaco.CancellationToken) => Promise<monaco.languages.Location[] | undefined>) => monaco.IDisposable]> = [
    [caps.definitionProvider, "textDocument/definition", (provideDefinition) => m.languages.registerDefinitionProvider(LANGUAGE, { provideDefinition })],
    [caps.declarationProvider, "textDocument/declaration", (provideDeclaration) => m.languages.registerDeclarationProvider(LANGUAGE, { provideDeclaration })],
    [caps.typeDefinitionProvider, "textDocument/typeDefinition", (provideTypeDefinition) => m.languages.registerTypeDefinitionProvider(LANGUAGE, { provideTypeDefinition })],
    [caps.implementationProvider, "textDocument/implementation", (provideImplementation) => m.languages.registerImplementationProvider(LANGUAGE, { provideImplementation })],
  ];
  for (const [cap, method, register] of navigation) {
    if (!enabled(cap)) continue;
    add(register((model, pos, token) => (own(model) ? locations(method, posParams(ctx, pos), token) : Promise.resolve(undefined))));
  }

  if (enabled(caps.referencesProvider)) {
    add(m.languages.registerReferenceProvider(LANGUAGE, {
      provideReferences(model, pos, context, token) {
        if (!own(model)) return undefined;
        return locations(
          "textDocument/references",
          { ...posParams(ctx, pos), context: { includeDeclaration: context.includeDeclaration } },
          token,
        );
      },
    }));
  }

  if (enabled(caps.documentHighlightProvider)) {
    add(m.languages.registerDocumentHighlightProvider(LANGUAGE, {
      async provideDocumentHighlights(model, pos, token) {
        if (!own(model)) return undefined;
        const res = await client.request<lsp.DocumentHighlight[]>("textDocument/documentHighlight", posParams(ctx, pos), { token });
        return res?.map((h) => ({
          range: cv.toMonacoRange(h.range),
          kind: h.kind === 3 ? m.languages.DocumentHighlightKind.Write
            : h.kind === 2 ? m.languages.DocumentHighlightKind.Read
            : m.languages.DocumentHighlightKind.Text,
        }));
      },
    }));
  }

  if (enabled(caps.documentSymbolProvider)) {
    const convert = (s: lsp.DocumentSymbol): monaco.languages.DocumentSymbol => ({
      name: s.name,
      detail: s.detail ?? "",
      kind: (s.kind - 1) as monaco.languages.SymbolKind, // LSP kinds are Monaco's + 1
      tags: s.tags?.includes(1) ? [m.languages.SymbolTag.Deprecated] : [],
      range: cv.toMonacoRange(s.range),
      selectionRange: cv.toMonacoRange(s.selectionRange),
      children: s.children?.map(convert),
    });
    add(m.languages.registerDocumentSymbolProvider(LANGUAGE, {
      async provideDocumentSymbols(model, token) {
        if (!own(model)) return undefined;
        const res = await client.request<Array<lsp.DocumentSymbol | lsp.SymbolInformation>>(
          "textDocument/documentSymbol",
          { textDocument: docId(ctx) },
          { token },
        );
        return res?.map((s) =>
          "location" in s
            ? convert({ name: s.name, kind: s.kind, tags: s.tags, detail: s.containerName, range: s.location.range, selectionRange: s.location.range })
            : convert(s),
        );
      },
    }));
  }

  // ---- rename -----------------------------------------------------------
  if (enabled(caps.renameProvider)) {
    const canPrepare = typeof caps.renameProvider === "object" && !!caps.renameProvider.prepareProvider;
    add(m.languages.registerRenameProvider(LANGUAGE, {
      async provideRenameEdits(model, position, newName, token) {
        if (!own(model)) return undefined;
        const res = await client.request<lsp.WorkspaceEdit>(
          "textDocument/rename",
          { ...posParams(ctx, position), newName },
          { token },
        );
        return {
          edits: cv.editsForDocument(res ?? undefined, ctx.docUri).map((e) => ({
            resource: model.uri,
            textEdit: cv.toMonacoEdit(e),
            versionId: undefined,
          })),
        };
      },
      async resolveRenameLocation(model, position, token) {
        if (!own(model)) return { range: new m.Range(1, 1, 1, 1), text: "", rejectReason: "Not the active document." };
        if (canPrepare) {
          const res = await client.request<lsp.Range | { range: lsp.Range; placeholder: string }>(
            "textDocument/prepareRename",
            posParams(ctx, position),
            { token },
          ).catch(() => null);
          if (!res) return { range: new m.Range(1, 1, 1, 1), text: "", rejectReason: "This symbol cannot be renamed." };
          const range = cv.toMonacoRange("range" in res ? res.range : res);
          return { range, text: "placeholder" in res ? res.placeholder : model.getValueInRange(range) };
        }
        const w = model.getWordAtPosition(position);
        if (!w) return { range: new m.Range(1, 1, 1, 1), text: "", rejectReason: "No symbol at the cursor." };
        return { range: new m.Range(position.lineNumber, w.startColumn, position.lineNumber, w.endColumn), text: w.word };
      },
    }));
  }

  // ---- quick fixes and refactorings -------------------------------------
  if (enabled(caps.codeActionProvider)) {
    add(m.editor.registerCommand(EXECUTE_COMMAND_ID, (_accessor, command: string, args: unknown[]) => {
      void client.request("workspace/executeCommand", { command, arguments: args });
    }));
    add(m.languages.registerCodeActionProvider(LANGUAGE, {
      async provideCodeActions(model, range, _context, token) {
        if (!own(model)) return undefined;
        const lspRange = cv.toLspRange(range);
        const overlapping = ctx.diagnostics().filter((d) => rangesTouch(d.range, lspRange));
        const res = await client.request<Array<lsp.CodeAction | lsp.Command>>(
          "textDocument/codeAction",
          { textDocument: docId(ctx), range: lspRange, context: { diagnostics: overlapping } },
          { token },
        );
        const actions: monaco.languages.CodeAction[] = [];
        for (const a of res ?? []) {
          // A bare Command has `command` as a string; a CodeAction has an optional Command object.
          const isBare = typeof (a as lsp.Command).command === "string";
          const action = isBare ? ({ title: a.title, command: a as lsp.Command } as lsp.CodeAction) : (a as lsp.CodeAction);
          const edits = cv.editsForDocument(action.edit, ctx.docUri);
          actions.push({
            title: action.title,
            kind: action.kind ?? "quickfix",
            isPreferred: action.isPreferred,
            edit: edits.length
              ? { edits: edits.map((e) => ({ resource: model.uri, textEdit: cv.toMonacoEdit(e), versionId: undefined })) }
              : undefined,
            command: action.command
              ? { id: EXECUTE_COMMAND_ID, title: action.command.title, arguments: [action.command.command, action.command.arguments ?? []] }
              : undefined,
          });
        }
        return { actions, dispose() {} };
      },
    }));
  }

  // ---- formatting -------------------------------------------------------
  const options = (o: monaco.languages.FormattingOptions) => ({ tabSize: o.tabSize, insertSpaces: o.insertSpaces });
  if (enabled(caps.documentFormattingProvider)) {
    add(m.languages.registerDocumentFormattingEditProvider(LANGUAGE, {
      async provideDocumentFormattingEdits(model, o, token) {
        if (!own(model)) return undefined;
        const res = await client.request<lsp.TextEdit[]>("textDocument/formatting", { textDocument: docId(ctx), options: options(o) }, { token });
        return res?.map(cv.toMonacoEdit);
      },
    }));
  }
  if (enabled(caps.documentRangeFormattingProvider)) {
    add(m.languages.registerDocumentRangeFormattingEditProvider(LANGUAGE, {
      async provideDocumentRangeFormattingEdits(model, range, o, token) {
        if (!own(model)) return undefined;
        const res = await client.request<lsp.TextEdit[]>(
          "textDocument/rangeFormatting",
          { textDocument: docId(ctx), range: cv.toLspRange(range), options: options(o) },
          { token },
        );
        return res?.map(cv.toMonacoEdit);
      },
    }));
  }

  return out;
}

function rangesTouch(a: lsp.Range, b: lsp.Range): boolean {
  const before = (x: lsp.Position, y: lsp.Position) => x.line < y.line || (x.line === y.line && x.character < y.character);
  return !before(a.end, b.start) && !before(b.end, a.start);
}
