// Per-render ambient context. Adapters in this package read it to register
// read-set tags and honor `.noTrack()` / `.timeout(ms)` overrides. The worker
// re-exports it for backward compatibility with R005's surface.
//
// See `.yah/docs/architecture/mesofact.md` §"Adapter read-set provenance".

import { AsyncLocalStorage } from "node:async_hooks";

export type TrackCtx = {
  readonly tags: Set<string>;
  // Per-call overrides toggled by `Source.noTrack()` / `.timeout(ms)`. The
  // adapter consults these inside the same async chain and resets after the
  // single read they apply to.
  next: {
    track: boolean;
    timeout_ms?: number;
  };
};

const storage = new AsyncLocalStorage<TrackCtx>();

export function runInTrackCtx<T>(fn: () => Promise<T>): Promise<{ value: T; ctx: TrackCtx }> {
  const ctx: TrackCtx = { tags: new Set(), next: { track: true } };
  return storage.run(ctx, async () => ({ value: await fn(), ctx }));
}

export function currentTrackCtx(): TrackCtx | undefined {
  return storage.getStore();
}
