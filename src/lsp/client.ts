/**
 * JSON-RPC client for the language server. Transport is the Tauri backend:
 * `clangd_send` writes a message, `lattice://lsp-message` delivers one.
 * Knows nothing about Monaco.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export const MESSAGE_EVENT = "lattice://lsp-message";
export const EXIT_EVENT = "lattice://lsp-exit";

export interface ExitInfo {
  code: number | null;
  stderr: string;
}

interface CancellationLike {
  isCancellationRequested: boolean;
  onCancellationRequested: (listener: () => void) => { dispose(): void };
}

interface Pending {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
  timer?: ReturnType<typeof setTimeout>;
}

export class LspError extends Error {
  constructor(message: string, readonly code: number) {
    super(message);
  }
}

type NotificationHandler = (params: any) => void;
type RequestHandler = (params: any) => unknown | Promise<unknown>;

export class LspClient {
  private nextId = 1;
  private pending = new Map<number, Pending>();
  private notifications = new Map<string, NotificationHandler>();
  private requests = new Map<string, RequestHandler>();
  private unlisten: Promise<UnlistenFn>[] = [];
  private closed = false;

  /** Subscribe to server traffic. Do this *before* starting the server so nothing is missed. */
  constructor(onExit: (info: ExitInfo) => void) {
    this.unlisten.push(
      listen<string>(MESSAGE_EVENT, (e) => this.onMessage(e.payload)),
      listen<ExitInfo>(EXIT_EVENT, (e) => {
        if (this.closed) return;
        this.failPending(new Error("clangd exited"));
        onExit(e.payload);
      }),
    );
  }

  /** Resolves once the event subscriptions are live. */
  ready(): Promise<void> {
    return Promise.all(this.unlisten).then(() => undefined);
  }

  onNotification(method: string, handler: NotificationHandler) {
    this.notifications.set(method, handler);
  }

  /** Handle a server-to-client request; the return value becomes the response. */
  onRequest(method: string, handler: RequestHandler) {
    this.requests.set(method, handler);
  }

  /** Resolves `null` if cancelled, rejects on protocol error, timeout or closed client. */
  request<T>(method: string, params: unknown, opts: { token?: CancellationLike; timeoutMs?: number } = {}): Promise<T | null> {
    if (this.closed) return Promise.reject(new Error("language client is closed"));
    const { token } = opts;
    if (token?.isCancellationRequested) return Promise.resolve(null);
    const id = this.nextId++;
    return new Promise<T | null>((resolve, reject) => {
      const entry: Pending = { resolve: resolve as (v: unknown) => void, reject };
      let sub: { dispose(): void } | undefined;
      const done = () => {
        clearTimeout(entry.timer);
        sub?.dispose();
        this.pending.delete(id);
      };
      const wrapped: Pending = {
        resolve: (v) => (done(), entry.resolve(v)),
        reject: (e) => (done(), entry.reject(e)),
      };
      this.pending.set(id, wrapped);
      if (opts.timeoutMs) {
        entry.timer = setTimeout(() => wrapped.reject(new Error(`${method} timed out`)), opts.timeoutMs);
      }
      sub = token?.onCancellationRequested(() => {
        void this.send({ jsonrpc: "2.0", method: "$/cancelRequest", params: { id } }).catch(() => {});
        wrapped.resolve(null);
      });
      this.send({ jsonrpc: "2.0", id, method, params }).catch((e) => wrapped.reject(asError(e)));
    });
  }

  notify(method: string, params: unknown): Promise<void> {
    if (this.closed) return Promise.resolve();
    return this.send({ jsonrpc: "2.0", method, params });
  }

  dispose() {
    if (this.closed) return;
    this.closed = true;
    this.failPending(new Error("language client is closed"));
    for (const p of this.unlisten) void p.then((un) => un());
  }

  private send(message: object): Promise<void> {
    return invoke<void>("clangd_send", { message: JSON.stringify(message) });
  }

  private failPending(error: Error) {
    for (const p of [...this.pending.values()]) p.reject(error);
  }

  private onMessage(raw: string) {
    if (this.closed) return;
    let msg: { id?: number | string; method?: string; params?: unknown; result?: unknown; error?: { code: number; message: string } };
    try {
      msg = JSON.parse(raw);
    } catch {
      console.error("[clangd] unparsable message", raw.slice(0, 200));
      return;
    }
    if (msg.method !== undefined && msg.id !== undefined) {
      void this.answerRequest(msg.id, msg.method, msg.params);
    } else if (msg.method !== undefined) {
      try {
        this.notifications.get(msg.method)?.(msg.params);
      } catch (e) {
        console.error(`[clangd] handler for ${msg.method} threw`, e);
      }
    } else if (typeof msg.id === "number") {
      const p = this.pending.get(msg.id);
      if (!p) return;
      if (msg.error) p.reject(new LspError(msg.error.message, msg.error.code));
      else p.resolve(msg.result ?? null);
    }
  }

  private async answerRequest(id: number | string, method: string, params: unknown) {
    let response: object;
    try {
      const handler = this.requests.get(method);
      // Unknown requests (progress creation, capability registration, ...) are acknowledged.
      const result = handler ? await handler(params) : method === "workspace/configuration" ? [] : null;
      response = { jsonrpc: "2.0", id, result: result ?? null };
    } catch (e) {
      response = { jsonrpc: "2.0", id, error: { code: -32603, message: asError(e).message } };
    }
    if (!this.closed) await this.send(response).catch(() => {});
  }
}

function asError(e: unknown): Error {
  return e instanceof Error ? e : new Error(String(e));
}
