//! @yah:relay(R820, "SigV4-sign SSR-isolate r2() requests (needs deno_core 0.404→0.410 bump across mesofact-ssr's whole extension set)")
//! @yah:at(2026-08-13T19:12:16Z)
//! @yah:status(open)
//! @yah:assignee(agent:bundle-anthropic-miravel)
//! @yah:next("Spike the deno_core 0.404->0.410 bump in isolation (a throwaway branch/crate) to find the compatible version set for deno_webidl/deno_web/deno_fetch/deno_net/deno_fs/deno_permissions before touching mesofact-ssr for real.")
//! @yah:next("Once the extension set builds on deno_core 0.410, add deno_crypto + bind ext:deno_crypto's bootstrap JS to globalThis.crypto in ssr_bootstrap.js.")
//! @yah:next("Vendor aws4fetch (MIT, packages/mesofact-runtime/node_modules/aws4fetch/dist/aws4fetch.esm.mjs is ~292 lines, already a workspace npm dep) as a new js/aws4fetch.js loader specifier in mesofact-ssr, alongside the existing HARNESS_SPECIFIER/RUNTIME_SPECIFIER pattern in ssr.rs's SsrModuleLoader.")
//! @yah:next("Swap R2Adapter.send() in ssr_runtime_shim.js to sign via the vendored AwsClient using the accessKeyId/secretAccessKey R444 already threads onto R2SourceCoords/the adapter instance (unused today) instead of a bare fetch().")
//! @yah:gotcha("R444 shipped an unsigned-fetch R2Adapter in the SSR isolate — works against mesofact-dev's anonymous s3s-fs surface (AllowAllAccess, s3s.rs) but will get 403 against real Cloudflare R2 in production (mesofact serve), which enforces SigV4.")
//! @yah:gotcha("The real @mesofact/runtime R2Adapter (packages/mesofact-runtime/src/adapters/r2.ts) signs via aws4fetch, which needs crypto.subtle (HMAC-SHA256) -- WebCrypto isn't in the isolate's extension set (deno_webidl/deno_web/deno_fetch/deno_net/deno_fs/deno_permissions, all pinned to deno_core 0.404).")
//! @yah:gotcha("Verified during R444: adding deno_crypto = \"0.271\" to crates/mesofact-ssr/Cargo.toml and running `cargo check -p mesofact-ssr` fails -- deno_crypto 0.271.0 hard-pins deno_core = \"0.410.0\" (Cargo.toml.orig), which pulls a deno_v8 split generation incompatible with the 0.404 line ('either feature `v8` or `quickjs` must be enabled'). Fixing this needs bumping deno_core AND deno_webidl/deno_web/deno_fetch/deno_net/deno_fs/deno_permissions together to whatever versions pair with deno_core 0.410 -- a version-compat spike in its own right, out of R444's blast radius.")
//! @arch:see(.yah/docs/working/W174-mesofact-rust-native-pipeline.md)

// Request-time stand-in for "@mesofact/runtime" inside the deno_core SSR
// runtime (R637). SSR bundles keep the runtime `external` exactly like SSG
// bundles do (mesofact-build/src/bundle.rs:201), so `mode:"ssr"` entrypoints
// that import anything from the barrel — `r2`, `sqlite`, or just the pure
// hydration/head helpers — arrive at the isolate with a bare specifier the
// loader has to answer. Before this file existed the SSR loader answered only
// the SSG one, so registering such a bundle threw
// `Relative import path "@mesofact/runtime" not prefixed with / or ./ or ../`
// and `mesofact::ssr::spawn` failed the whole SSR child at startup.
//
// The pure value-level surface (escape rule, head weave, hydration tags,
// track-ctx) is identical at both tiers, so it is re-exported from the SSG
// shim rather than duplicated — keeping the W173 XSS escape rule pinned in one
// place. SOURCE ADAPTERS differ: at build time they are unavailable because
// the Rust-native pipeline reads declared `data_inputs`; at request time `r2`
// resolves against coordinates `mesofact::ssr::spawn` resolves from
// `mesofact.config.toml` + an env map and pushes in via
// `globalThis.__mesofact_ssr.registerR2Sources` (R444) — see ssr_harness.js.
export * from "mesofact:runtime-pure";
import { currentTrackCtx } from "mesofact:runtime-pure";

class SourceError extends Error {
  constructor(message, source, retryable) {
    super(message);
    this.name = new.target.name;
    this.source = source;
    this.retryable = retryable;
  }
}

class SourceUnavailableError extends SourceError {
  constructor(source) {
    super(`source unavailable: ${source}`, source, true);
  }
}

class SourceQueryError extends SourceError {
  constructor(source, message) {
    super(`source query error (${source}): ${message}`, source, false);
  }
}

// Not-registered is a distinct case from "adapter errored" — same wording
// packages/mesofact-runtime/src/adapters/r2.ts uses, so a route author sees
// the same message in dev SSR as they would from the Bun worker in prod.
class SourceNotRegisteredError extends Error {
  constructor(name, kind) {
    super(`${kind} source not registered: ${name} (declare it in mesofact.config.toml under [sources.${name}])`);
    this.name = "SourceNotRegisteredError";
  }
}

// Port of packages/mesofact-runtime/src/source.ts BaseSource — `.noTrack()` /
// `.timeout(ms)` chain modifiers + read-set tag emission, sharing the same
// `currentTrackCtx()` ambient stack `runInTrackCtx` (re-exported above) pushes
// onto per render.
class BaseSource {
  constructor(name) {
    this.name = name;
  }

  noTrack() {
    const ctx = currentTrackCtx();
    if (ctx) ctx.next.track = false;
    return this;
  }

  timeout(ms) {
    const ctx = currentTrackCtx();
    if (ctx) ctx.next.timeout_ms = ms;
    return this;
  }

  consumeOverrides(defaultTimeoutMs) {
    const ctx = currentTrackCtx();
    if (!ctx) return { track: true, timeout_ms: defaultTimeoutMs };
    const effective = {
      track: ctx.next.track,
      timeout_ms: ctx.next.timeout_ms ?? defaultTimeoutMs,
    };
    ctx.next = { track: true };
    return effective;
  }

  emitTag(tag, track) {
    if (!track) return;
    currentTrackCtx()?.tags.add(tag);
  }
}

const DEFAULT_TIMEOUT_MS = 2000;

// Read-only R2 (S3-compatible) adapter. Ported from
// packages/mesofact-runtime/src/adapters/r2.ts, minus SigV4 request signing —
// unsigned requests are enough for mesofact-dev's anonymous s3s-fs surface
// (s3s.rs AllowAllAccess) but not real Cloudflare R2 in production. See the
// R820 annotation at the top of this file for the deno_core version blocker.
class R2Adapter extends BaseSource {
  constructor({ name, bucket, endpoint, accessKeyId, secretAccessKey }) {
    super(name);
    this.bucket = bucket;
    this.endpoint = endpoint.replace(/\/$/, "");
    // Not yet used for signing (see file-top R820 annotation) — kept on the
    // instance so wiring SigV4 later is a local change to `send()`, not
    // another pass threading credentials through Rust.
    this.accessKeyId = accessKeyId;
    this.secretAccessKey = secretAccessKey;
  }

  async fetch(key) {
    const { track, timeout_ms } = this.consumeOverrides(DEFAULT_TIMEOUT_MS);
    this.emitTag(`r2:${this.bucket}:${key}`, track);
    const url = `${this.endpoint}/${this.bucket}/${encodeKey(key)}`;
    const res = await this.send(url, { method: "GET" }, timeout_ms);
    if (res.status === 404) return null;
    if (!res.ok) {
      throw new SourceQueryError(this.name, `r2 GET ${key} → HTTP ${res.status}`);
    }
    return new Uint8Array(await res.arrayBuffer());
  }

  async list(prefix, opts = {}) {
    const { track, timeout_ms } = this.consumeOverrides(DEFAULT_TIMEOUT_MS);
    this.emitTag(`r2:${this.bucket}:${prefix}*`, track);
    const params = new URLSearchParams({ "list-type": "2", prefix });
    if (opts.limit !== undefined) params.set("max-keys", String(opts.limit));
    if (opts.cursor) params.set("continuation-token", opts.cursor);
    if (opts.delimiter) params.set("delimiter", opts.delimiter);
    const url = `${this.endpoint}/${this.bucket}?${params.toString()}`;
    const res = await this.send(url, { method: "GET" }, timeout_ms);
    if (!res.ok) {
      throw new SourceQueryError(this.name, `r2 LIST ${prefix} → HTTP ${res.status}`);
    }
    return parseListV2(await res.text());
  }

  async send(url, init, timeout_ms) {
    let timer;
    try {
      return await Promise.race([
        fetch(url, init).catch((err) => {
          throw new SourceUnavailableError(this.name, { cause: err });
        }),
        new Promise((_, reject) => {
          timer = setTimeout(() => reject(new SourceUnavailableError(this.name)), timeout_ms);
        }),
      ]);
    } finally {
      if (timer) clearTimeout(timer);
    }
  }

  get(_table, _id) {
    return Promise.reject(new SourceQueryError(this.name, "r2 sources do not support get()"));
  }

  query(_sql, _params) {
    return Promise.reject(new SourceQueryError(this.name, "r2 sources do not support query()"));
  }

  head() {
    return Promise.reject(new SourceQueryError(this.name, "r2 sources do not support head()"));
  }
}

// `encodeURIComponent` re-encodes `/`, which we want to preserve so S3 sees
// path-style keys correctly.
function encodeKey(key) {
  return key
    .split("/")
    .map((segment) => encodeURIComponent(segment))
    .join("/");
}

// Minimal ListBucketResult v2 parser — same regex approach as r2.ts (S3's XML
// is a fixed shape; no XML parser dependency needed for the fields the
// R2Object contract exposes).
function parseListV2(xml) {
  const out = [];
  for (const match of xml.matchAll(/<Contents>([\s\S]*?)<\/Contents>/g)) {
    const inner = match[1];
    const key = pick(inner, "Key");
    const sizeStr = pick(inner, "Size");
    const last_modified = pick(inner, "LastModified");
    const etagRaw = pick(inner, "ETag");
    if (key === undefined || sizeStr === undefined || last_modified === undefined) continue;
    out.push({
      key,
      size: Number.parseInt(sizeStr, 10),
      last_modified,
      ...(etagRaw ? { etag: etagRaw.replace(/^"|"$/g, "") } : {}),
    });
  }
  return out;
}

function pick(haystack, tag) {
  const m = haystack.match(new RegExp(`<${tag}>([\\s\\S]*?)</${tag}>`));
  return m?.[1];
}

// Per-isolate registry, mirroring packages/mesofact-runtime/src/adapters/r2.ts.
// Populated once at SsrRuntime boot by `globalThis.__mesofact_ssr.registerR2Sources`
// (ssr_harness.js), which `mesofact::ssr::spawn` calls with coordinates already
// resolved from `mesofact.config.toml` + env — never touched from inside here.
const r2Registry = new Map();

export function registerR2Sources(sources) {
  for (const s of sources) {
    r2Registry.set(s.name, new R2Adapter(s));
  }
}

export function r2(name) {
  const adapter = r2Registry.get(name);
  if (!adapter) throw new SourceNotRegisteredError(name, "r2");
  return adapter;
}

export function sqlite(name) {
  // sqlite reads a local file — out of R444's scope (env/R2 plumbing only).
  // Keeping the throwing stub means a route declaring [sources.<x>] kind =
  // "sqlite" still fails loudly and specifically rather than silently.
  return {
    name,
    get: () => Promise.reject(new SourceUnavailableError(name)),
    query: () => Promise.reject(new SourceUnavailableError(name)),
    fetch: () => Promise.reject(new SourceUnavailableError(name)),
    list: () => Promise.reject(new SourceUnavailableError(name)),
    head: () => Promise.reject(new SourceUnavailableError(name)),
    noTrack() {
      return this;
    },
    timeout() {
      return this;
    },
  };
}
