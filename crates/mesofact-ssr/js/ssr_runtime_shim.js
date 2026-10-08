//! @yah:relay(R820, "SigV4-sign SSR-isolate r2() requests (needs deno_core 0.404→0.410 bump across mesofact-ssr's whole extension set)")
//! @yah:at(2026-08-13T19:12:16Z)
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

// Default per-call timeouts, matching packages/mesofact-runtime's adapters.
const R2_TIMEOUT_MS = 2000;
const SQLITE_TIMEOUT_MS = 100;

const ops = globalThis.Deno.core.ops;

// R750-F3: every read goes through a Rust op against the SourceBackend the
// caller attached to this dispatch (signed S3 for r2, turso for sqlite —
// mesofact's `ssr` feature). Ops resolve to `{ok}` / `{err: {kind, message}}`;
// this maps the error kinds back onto the runtime's error classes.
async function callSource(op, kind, name, args) {
  const reply = await op({ source: name, ...args });
  if ("ok" in reply) return reply.ok;
  const { kind: errKind, message } = reply.err;
  if (errKind === "not_registered") throw new SourceNotRegisteredError(name, kind);
  if (errKind === "unavailable") {
    const e = new SourceUnavailableError(name);
    if (message) e.message += ` (${message})`;
    throw e;
  }
  throw new SourceQueryError(name, message);
}

class R2Adapter extends BaseSource {
  constructor({ name, bucket }) {
    super(name);
    this.bucket = bucket;
  }
  async fetch(key) {
    const { track, timeout_ms } = this.consumeOverrides(R2_TIMEOUT_MS);
    this.emitTag(`r2:${this.bucket}:${key}`, track);
    const bytes = await callSource(ops.op_mesofact_source_fetch, "r2", this.name, { key, timeout_ms });
    return bytes ?? null;
  }
  async list(prefix, opts = {}) {
    const { track, timeout_ms } = this.consumeOverrides(R2_TIMEOUT_MS);
    this.emitTag(`r2:${this.bucket}:${prefix}*`, track);
    return callSource(ops.op_mesofact_source_list, "r2", this.name, { prefix, opts, timeout_ms });
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

// Port of packages/mesofact-runtime/src/adapters/sqlite.ts's read surface and
// tag scheme; the SQL runs Rust-side.
class SqliteAdapter extends BaseSource {
  async get(table, id) {
    const { track, timeout_ms } = this.consumeOverrides(SQLITE_TIMEOUT_MS);
    this.emitTag(`sqlite:${this.name}:${table}:${id}`, track);
    return (
      (await callSource(ops.op_mesofact_source_get, "sqlite", this.name, {
        table: String(table),
        id: String(id),
        timeout_ms,
      })) ?? null
    );
  }
  async query(sql, params = []) {
    const { track, timeout_ms } = this.consumeOverrides(SQLITE_TIMEOUT_MS);
    const tables = extractTables(sql);
    if (tables.length === 0) this.emitTag(`sqlite:${this.name}`, track);
    else for (const t of tables) this.emitTag(`sqlite:${this.name}:${t}`, track);
    return callSource(ops.op_mesofact_source_query, "sqlite", this.name, { sql, params, timeout_ms });
  }
  fetch(_key) {
    return Promise.reject(new SourceQueryError(this.name, "sqlite sources do not support fetch()"));
  }
  list(_prefix) {
    return Promise.reject(new SourceQueryError(this.name, "sqlite sources do not support list()"));
  }
}

// Same FROM/JOIN table scrape as adapters/sqlite.ts, for read-set tags.
function extractTables(sql) {
  const out = new Set();
  for (const m of sql.matchAll(/\b(?:from|join)\s+["'`]?([A-Za-z_][\w$]*)/gi)) out.add(m[1]);
  return [...out];
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

// sqlite names have no isolate-side registry: the backend holds them, so an
// unknown name rejects with SourceNotRegisteredError on first read rather than
// throwing here.
export function sqlite(name) {
  return new SqliteAdapter(name);
}
