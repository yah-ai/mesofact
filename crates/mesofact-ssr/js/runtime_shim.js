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

// Port of packages/mesofact-runtime/src/head.ts (W270 §4). Keep byte-identical
// with the TS original so both pipelines weave the same head bytes.
function escapeHtmlText(value) {
  return value.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function escapeHtmlAttr(value) {
  return escapeHtmlText(value).replace(/"/g, "&quot;");
}

function metaTag(attr, key, content) {
  return `<meta ${attr}="${key}" content="${escapeHtmlAttr(content)}">`;
}

export function renderHead(head) {
  const tags = [];

  if (head.title !== undefined) tags.push(`<title>${escapeHtmlText(head.title)}</title>`);
  if (head.description !== undefined) tags.push(metaTag("name", "description", head.description));
  if (head.canonical !== undefined) {
    tags.push(`<link rel="canonical" href="${escapeHtmlAttr(head.canonical)}">`);
  }
  if (head.noindex) tags.push(`<meta name="robots" content="noindex">`);

  const og = head.og;
  if (og) {
    if (og.title !== undefined) tags.push(metaTag("property", "og:title", og.title));
    if (og.description !== undefined) {
      tags.push(metaTag("property", "og:description", og.description));
    }
    if (og.type !== undefined) tags.push(metaTag("property", "og:type", og.type));
    if (og.url !== undefined) tags.push(metaTag("property", "og:url", og.url));
    if (og.image !== undefined) tags.push(metaTag("property", "og:image", og.image));
    if (og.siteName !== undefined) tags.push(metaTag("property", "og:site_name", og.siteName));
  }

  const tw = head.twitter;
  if (tw) {
    if (tw.card !== undefined) tags.push(metaTag("name", "twitter:card", tw.card));
    if (tw.title !== undefined) tags.push(metaTag("name", "twitter:title", tw.title));
    if (tw.description !== undefined) {
      tags.push(metaTag("name", "twitter:description", tw.description));
    }
    if (tw.image !== undefined) tags.push(metaTag("name", "twitter:image", tw.image));
    if (tw.site !== undefined) tags.push(metaTag("name", "twitter:site", tw.site));
    if (tw.creator !== undefined) tags.push(metaTag("name", "twitter:creator", tw.creator));
  }

  for (const link of head.links ?? []) {
    tags.push(`<link rel="${escapeHtmlAttr(link.rel)}" href="${escapeHtmlAttr(link.href)}">`);
  }

  return tags.join("");
}

export function weaveHead(html, head) {
  const markup = renderHead(head);
  if (markup === "") return html;
  const idx = html.toLowerCase().lastIndexOf("</head>");
  if (idx === -1) return markup + html;
  return html.slice(0, idx) + markup + html.slice(idx);
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
      `source '${name}' (${kind}) is not available during build-time SSG: the Rust-native pipeline reads declared data_inputs only. Use data_inputs / prerender.from_data for source-adapter renders.`,
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
