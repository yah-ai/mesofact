// Adapter API surface. Read-only by design — mesofact has no write API.
// See `.yah/docs/architecture/mesofact.md` §"Adapter API surface".
//
// The design doc lists `get`/`query`/`fetch`/`list` on a single interface with
// per-backend annotations. We split them into BlobSource (r2) and
// KeyValueSource (sqlite/pg) so callers get type-safety on what they hold:
// `r2('assets').get(...)` is a type error, not a runtime trap.

import { currentTrackCtx } from "./track-ctx.js";

export type ListOpts = {
  limit?: number;
  cursor?: string;
  delimiter?: string;
};

export type R2Object = {
  key: string;
  size: number;
  last_modified: string;
  etag?: string;
};

// Common shape every adapter exposes.
export interface Source {
  readonly name: string;

  // Skip read-set tracking for the next call (e.g. fast-changing flags that
  // would over-purge Mode 1 HTML). The override resets after one read.
  noTrack(): this;

  // Override the next call's timeout. Defaults: sqlite 100ms, pg 500ms,
  // r2 2000ms. The override resets after one read.
  timeout(ms: number): this;
}

// Blob backends (r2): byte payloads addressed by key or key prefix.
export interface BlobSource extends Source {
  fetch(key: string): Promise<Uint8Array | null>;
  list(prefix: string, opts?: ListOpts): Promise<R2Object[]>;
}

// Row backends (sqlite, pg): tabular reads by id or query.
export interface KeyValueSource extends Source {
  get<T>(table: string, id: string): Promise<T | null>;
  query<T>(sql: string, params?: unknown[]): Promise<T[]>;
}

// Shared impl for `.noTrack()` / `.timeout(ms)` and tag emission. Adapters
// extend this and implement the read methods their backend supports.
export abstract class BaseSource implements Source {
  constructor(public readonly name: string) {}

  noTrack(): this {
    const ctx = currentTrackCtx();
    if (ctx) ctx.next.track = false;
    return this;
  }

  timeout(ms: number): this {
    const ctx = currentTrackCtx();
    if (ctx) ctx.next.timeout_ms = ms;
    return this;
  }

  // Consume per-call overrides applied by the most recent `.noTrack()` /
  // `.timeout(ms)` and return the effective settings for a single read. The
  // override slot resets after this call, so two chained reads only carry the
  // override on the first.
  protected consumeOverrides(defaultTimeoutMs: number): {
    track: boolean;
    timeout_ms: number;
  } {
    const ctx = currentTrackCtx();
    if (!ctx) return { track: true, timeout_ms: defaultTimeoutMs };
    const effective = {
      track: ctx.next.track,
      timeout_ms: ctx.next.timeout_ms ?? defaultTimeoutMs,
    };
    ctx.next = { track: true };
    return effective;
  }

  // Add a read-set tag to the ambient trackCtx, unless tracking was disabled
  // for this call or there is no ctx (e.g. test harness running render outside
  // `runInTrackCtx`).
  protected emitTag(tag: string, track: boolean): void {
    if (!track) return;
    currentTrackCtx()?.tags.add(tag);
  }
}
