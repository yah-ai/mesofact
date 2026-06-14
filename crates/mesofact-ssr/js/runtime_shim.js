// Build-time stand-in for "@mesofact/runtime" inside the deno_core SSG
// runtime. Server bundles keep the runtime `external` (same as the Bun
// pipeline — see packages/mesofact-build/src/bundle.ts), and this module is
// what that external specifier resolves to during build-time rendering.
//
// Scope: exactly the value-level surface route code may touch during a
// build-time render. The pure helpers (escape rule, tag shapes) are ports of
// packages/mesofact-runtime/src/hydration.ts and are pinned by W173 § "XSS
// escape rule" — keep them byte-identical with the TS originals so both
// pipelines emit the same HTML. Adapters are present but throw on use:
// build-time renders read declared data_inputs, not live sources (the
// source-derived prerender shape is unsupported in the Rust-native pipeline
// v1; see W174 amendment).

export const SPA_STATE_SCRIPT_ID = "__MESOFACT_STATE__";
export const SSR_DATA_SCRIPT_ID = "__mesofact_data__";
export const MANIFEST_VERSION = "1";
export const DEFAULT_RESILIENCE_TIMEOUT_MS = 30_000;

// `defineRoutes` is an identity here: the Rust pipeline re-validates the
// extracted config with the same rules (route_config.rs), so authoring
// errors still fail the build — at extraction instead of import time.
export function defineRoutes(config) {
  return config;
}

export function escapeJsonForScriptTag(value) {
  return JSON.stringify(value).replace(/[<>&\u2028\u2029]/g, (c) => {
    switch (c) {
      case "<":
        return "\\u003c";
      case ">":
        return "\\u003e";
      case "&":
        return "\\u0026";
      case "\u2028":
        return "\\u2028";
      case "\u2029":
        return "\\u2029";
      default:
        return c;
    }
  });
}

export function hydrationDataTag(data) {
  return `<script id="${SSR_DATA_SCRIPT_ID}" type="application/json">${escapeJsonForScriptTag(data)}</script>`;
}

export function hydrationScriptTag(src) {
  const safeSrc = src.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;");
  return `<script type="module" src="${safeSrc}"></script>`;
}

// Track-ctx: renders execute sequentially inside one V8 isolate, so a plain
// stack stands in for AsyncLocalStorage (node:async_hooks does not exist
// here). Adapter calls during an awaited render still see the right ctx.
const ctxStack = [];

export function currentTrackCtx() {
  return ctxStack[ctxStack.length - 1];
}

export function runInTrackCtx(fn) {
  const ctx = { tags: new Set(), next: { track: true } };
  ctxStack.push(ctx);
  return (async () => {
    try {
      const value = await fn();
      return { value, ctx };
    } finally {
      ctxStack.pop();
    }
  })();
}

class SourceUnavailableError extends Error {
  constructor(name, kind) {
    super(
      `source '${name}' (${kind}) is not available during build-time SSG: the Rust-native pipeline reads declared data_inputs only. Use data_inputs / prerender.from_data, or build with --legacy-bun for source-adapter renders.`,
    );
    this.name = "SourceUnavailableError";
  }
}

const throwingSource = (name, kind) => ({
  list: () => Promise.reject(new SourceUnavailableError(name, kind)),
  get: () => Promise.reject(new SourceUnavailableError(name, kind)),
  head: () => Promise.reject(new SourceUnavailableError(name, kind)),
  query: () => Promise.reject(new SourceUnavailableError(name, kind)),
  noTrack() {
    return this;
  },
  timeout() {
    return this;
  },
});

export function r2(name) {
  return throwingSource(name, "r2");
}

export function sqlite(name) {
  return throwingSource(name, "sqlite");
}

export function registerR2() {}
export function clearR2Registry() {}
export function registerSqlite() {}
export function registerSourcesFromConfig() {}
