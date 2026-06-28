#!/usr/bin/env bun
// Bun render-pool worker. UDS server, NDJSON framing, lifecycle messages,
// per-route concurrency + bounded queue, AsyncLocalStorage trackCtx around
// each render call.
//
// See `.yah/docs/architecture/mesofact.md` §"IPC protocol" + §"Concurrency
// per worker".

import { existsSync, unlinkSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { dirname, isAbsolute, resolve as resolvePath } from "node:path";
import { pathToFileURL } from "node:url";

import type {
  Manifest,
  ManifestRoute,
  RenderFn,
  RenderRequest,
  RenderResult,
} from "@mesofact/runtime";
import { SourceError, loadConfig, registerSourcesFromConfig, runInTrackCtx } from "@mesofact/runtime";

import {
  NdjsonDecoder,
  encode,
  type ErrorPayload,
  type OkMsg,
  type ErrMsg,
  type ProxyToWorker,
  type RenderMsg,
} from "./protocol.ts";
import { Pool, OverflowError, DrainingError } from "./pool.ts";

export type WorkerOptions = {
  socket: string;
  manifest: string;
  // Base directory for resolving relative entrypoint paths. Defaults to
  // the manifest file's parent dir.
  baseDir?: string;
  // Path to `mesofact.config.toml`. When set, adapters are registered at boot
  // so render entrypoints can reach `sqlite('db')` / `r2('assets')` at request
  // time. Credentials come from the worker's env (yubaba injects them).
  config?: string;
};

type RouteHandlers = Map<string, RenderFn>;

type Writer = (msg: OkMsg | ErrMsg | { id: 0; kind: "ready" | "pong"; [k: string]: unknown }) => void;

async function loadEntrypoint(absPath: string): Promise<RenderFn> {
  const mod = (await import(pathToFileURL(absPath).href)) as {
    default?: unknown;
    render?: unknown;
  };
  const fn = mod.default ?? mod.render;
  if (typeof fn !== "function") {
    throw new Error(
      `entrypoint ${absPath} must export a default or named \`render\` function`,
    );
  }
  return fn as RenderFn;
}

async function buildRouteHandlers(
  manifest: Manifest,
  baseDir: string,
  pool: Pool,
): Promise<RouteHandlers> {
  const handlers: RouteHandlers = new Map();
  for (const route of manifest.routes) {
    const abs = isAbsolute(route.render_entrypoint)
      ? route.render_entrypoint
      : resolvePath(baseDir, route.render_entrypoint);
    const fn = await loadEntrypoint(abs);
    handlers.set(route.route, fn);
    pool.configureRoute(route.route, route.concurrency);
  }
  return handlers;
}

function classifyError(err: unknown): ErrorPayload {
  if (err instanceof OverflowError) {
    return { code: "queue_overflow", message: err.message, retryable: true };
  }
  if (err instanceof DrainingError) {
    return { code: "draining", message: err.message, retryable: true };
  }
  if (err instanceof SourceError) {
    // SourceError subclasses carry their own kind. We mirror the discriminant
    // via constructor name so the proxy can map without importing the class.
    const code =
      err.constructor.name === "SourceUnavailableError"
        ? "source_unavailable"
        : err.constructor.name === "SourceTimeoutError"
          ? "source_timeout"
          : err.constructor.name === "SourceQueryError"
            ? "source_query"
            : err.constructor.name === "RowNotFoundError"
              ? "row_not_found"
              : "render_failed";
    return {
      code: code as ErrorPayload["code"],
      message: err.message,
      source: err.source,
      retryable: err.retryable,
    };
  }
  const message = err instanceof Error ? err.message : String(err);
  return { code: "render_failed", message, retryable: false };
}

async function handleRender(
  msg: RenderMsg,
  handlers: RouteHandlers,
  pool: Pool,
  write: Writer,
): Promise<void> {
  const fn = handlers.get(msg.route);
  if (!fn) {
    write({
      id: msg.id,
      kind: "err",
      error: {
        code: "route_unknown",
        message: `unknown route: ${msg.route}`,
        retryable: false,
      },
    });
    return;
  }

  try {
    const { value: result, ctx } = await pool.run(msg.route, () =>
      runInTrackCtx(() => invokeWithDeadline(fn, msg.req, msg.deadline_ms)),
    );
    const cacheTags = mergeTags(result.cache.tags, ctx.tags);
    write({
      id: msg.id,
      kind: "ok",
      html: result.html,
      ...(result.headers ? { headers: result.headers } : {}),
      cache: {
        ttl: result.cache.ttl,
        ...(cacheTags.length > 0 ? { tags: cacheTags } : {}),
      },
    });
  } catch (err) {
    write({ id: msg.id, kind: "err", error: classifyError(err) });
  }
}

function mergeTags(
  fromResult: readonly string[] | undefined,
  fromCtx: Set<string>,
): string[] {
  if (fromCtx.size === 0) return fromResult ? [...fromResult] : [];
  const merged = new Set<string>(fromCtx);
  if (fromResult) for (const t of fromResult) merged.add(t);
  return [...merged];
}

async function invokeWithDeadline(
  fn: RenderFn,
  req: RenderRequest,
  deadline_ms: number,
): Promise<RenderResult> {
  if (deadline_ms <= 0) return fn(req);
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race<RenderResult>([
      fn(req),
      new Promise<RenderResult>((_, reject) => {
        timer = setTimeout(
          () =>
            reject(
              Object.assign(new Error(`render exceeded ${deadline_ms}ms`), {
                code: "render_failed",
              }),
            ),
          deadline_ms,
        );
      }),
    ]);
  } finally {
    if (timer) clearTimeout(timer);
  }
}

export async function runWorker(opts: WorkerOptions): Promise<void> {
  const manifestPath = resolvePath(opts.manifest);
  const baseDir = opts.baseDir ?? dirname(manifestPath);
  const manifest = JSON.parse(await readFile(manifestPath, "utf8")) as Manifest;

  // Register data-source adapters before loading entrypoints so a render's
  // first `sqlite('db')` / `r2('assets')` resolves. A declared source with a
  // missing credential env var throws here — fail fast at boot, not at request.
  if (opts.config) {
    registerSourcesFromConfig(loadConfig(resolvePath(opts.config)));
  }

  const pool = new Pool();
  const handlers = await buildRouteHandlers(manifest, baseDir, pool);

  // UDS path cleanup — Bun.listen does not unlink stale sockets.
  if (existsSync(opts.socket)) unlinkSync(opts.socket);

  // Tracks full handler lifecycle (acquire + render + write). Drain awaits
  // these directly so responses always flush before process.exit. Tracking at
  // the pool level instead would resolve drain before the write completed.
  const inFlightHandlers = new Set<Promise<void>>();
  let drainStarted = false;
  let listener: ReturnType<typeof Bun.listen> | undefined;

  const startDrain = async (): Promise<void> => {
    if (drainStarted) return;
    drainStarted = true;
    listener?.stop();
    // pool.drain rejects anything queued (over the concurrency cap). The
    // in-flight set carries the rest — those promises only settle after the
    // response is written.
    await pool.drain();
    await Promise.allSettled([...inFlightHandlers]);
    if (existsSync(opts.socket)) {
      try {
        unlinkSync(opts.socket);
      } catch {
        /* ignore */
      }
    }
    process.exit(0);
  };

  process.on("SIGTERM", () => {
    void startDrain();
  });
  process.on("SIGINT", () => {
    void startDrain();
  });

  type SocketState = { decoder: NdjsonDecoder; readySent: boolean };

  listener = Bun.listen<SocketState>({
    unix: opts.socket,
    socket: {
      open(socket) {
        socket.data = { decoder: new NdjsonDecoder(), readySent: false };
        const ready = {
          id: 0 as const,
          kind: "ready" as const,
          manifest_version: manifest.version,
          build_id: manifest.build_id,
        };
        socket.write(encode(ready));
        socket.data.readySent = true;
      },
      data(socket, chunk) {
        const write: Writer = (msg) => {
          socket.write(encode(msg as never));
        };
        let parsed: unknown[];
        try {
          parsed = socket.data.decoder.push(chunk);
        } catch (err) {
          // Malformed JSON — protocol violation. Close the socket.
          socket.end();
          process.stderr.write(`worker: ndjson parse error: ${String(err)}\n`);
          return;
        }
        for (const raw of parsed) {
          const msg = raw as ProxyToWorker;
          if (msg.kind === "ping") {
            write({ id: 0, kind: "pong" });
          } else if (msg.kind === "drain") {
            void startDrain();
          } else if (msg.kind === "render") {
            const p = handleRender(msg, handlers, pool, write).catch((err) => {
              process.stderr.write(
                `worker: handler crash for id=${msg.id}: ${err instanceof Error ? err.stack : String(err)}\n`,
              );
            });
            inFlightHandlers.add(p);
            void p.finally(() => inFlightHandlers.delete(p));
          } else {
            // Unknown kind — log and ignore. Kind is a discriminant; the
            // proxy is the source of truth for adding new ones.
            process.stderr.write(
              `worker: unknown message kind: ${String((msg as { kind?: unknown }).kind)}\n`,
            );
          }
        }
      },
      close() {
        // Single proxy expected; if it drops, exit so the pool re-spawns us.
        // Tests reconnect, so only exit when not in test-injected scenarios:
        // a fresh client can still reconnect because the listener is alive.
      },
    },
  });
}

function parseArgs(argv: string[]): WorkerOptions {
  const opts: Partial<WorkerOptions> = {};
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]!;
    const eq = a.indexOf("=");
    const [flag, valFromEq] =
      eq === -1 ? [a, undefined] : [a.slice(0, eq), a.slice(eq + 1)];
    const take = (): string => valFromEq ?? argv[++i] ?? "";
    if (flag === "--socket") opts.socket = take();
    else if (flag === "--manifest") opts.manifest = take();
    else if (flag === "--cwd") opts.baseDir = take();
    else if (flag === "--config") opts.config = take();
    else if (flag === "--help" || flag === "-h") {
      process.stdout.write(
        "usage: bun worker.ts --socket <path> --manifest <path> [--cwd <dir>] [--config <toml>]\n",
      );
      process.exit(0);
    }
  }
  if (!opts.socket || !opts.manifest) {
    process.stderr.write("worker: --socket and --manifest are required\n");
    process.exit(2);
  }
  return opts as WorkerOptions;
}

// Run when invoked directly (`bun src/worker.ts ...`). The check uses
// import.meta.main, Bun's idiomatic equivalent of `require.main === module`.
if (import.meta.main) {
  runWorker(parseArgs(process.argv.slice(2))).catch((err) => {
    process.stderr.write(`worker: fatal: ${err instanceof Error ? err.stack : String(err)}\n`);
    process.exit(1);
  });
}
