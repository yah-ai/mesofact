// IPC envelopes between the Rust proxy and a Bun worker.
// Wire format is NDJSON: each message is a single JSON object terminated by
// `\n`. id=0 is reserved for lifecycle messages (ready/ping/pong/drain).
// See `.yah/docs/architecture/mesofact.md` §"IPC protocol".

import type { RenderRequest } from "@mesofact/runtime";

export type ErrorCode =
  | "source_unavailable"
  | "source_timeout"
  | "source_query"
  | "row_not_found"
  | "render_failed"
  | "route_unknown"
  | "queue_overflow"
  | "draining";

export type ErrorPayload = {
  code: ErrorCode;
  message: string;
  source?: string;
  retryable: boolean;
};

export type RenderMsg = {
  id: number;
  kind: "render";
  route: string;
  req: RenderRequest;
  deadline_ms: number;
};

export type OkMsg = {
  id: number;
  kind: "ok";
  html: string;
  headers?: Record<string, string>;
  cache: { ttl: number; tags?: readonly string[] };
};

export type ErrMsg = {
  id: number;
  kind: "err";
  error: ErrorPayload;
};

export type ReadyMsg = {
  id: 0;
  kind: "ready";
  manifest_version: string;
  build_id: string;
};

export type PingMsg = { id: 0; kind: "ping" };
export type PongMsg = { id: 0; kind: "pong" };
export type DrainMsg = { id: 0; kind: "drain" };

export type ProxyToWorker = RenderMsg | PingMsg | DrainMsg;
export type WorkerToProxy = OkMsg | ErrMsg | ReadyMsg | PongMsg;

export function encode(msg: WorkerToProxy | ProxyToWorker): Uint8Array {
  return new TextEncoder().encode(JSON.stringify(msg) + "\n");
}

// Stateful line splitter — feed bytes, get back zero-or-more parsed messages.
// Buffers partial lines across calls. Throws on malformed JSON so the caller
// can decide whether to close the socket or skip the line.
export class NdjsonDecoder {
  private buf = "";
  private readonly decoder = new TextDecoder("utf-8", { fatal: false });

  push(chunk: Uint8Array): unknown[] {
    this.buf += this.decoder.decode(chunk, { stream: true });
    const out: unknown[] = [];
    let nl = this.buf.indexOf("\n");
    while (nl !== -1) {
      const line = this.buf.slice(0, nl);
      this.buf = this.buf.slice(nl + 1);
      if (line.length > 0) out.push(JSON.parse(line));
      nl = this.buf.indexOf("\n");
    }
    return out;
  }
}
