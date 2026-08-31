// SSR dispatch harness (W174 pillar 4 / R449-F2). Loaded once at SsrRuntime
// startup; installs `globalThis.__mesofact_ssr` — the call surface the Rust
// dispatch path uses.
//
// Lifecycle:
//   register(modUrl) — dynamic-imports the route's render_entrypoint and
//     stores its default export (the Fetch handler) keyed by URL. Idempotent;
//     re-registering replaces the prior handler.
//   dispatch(modUrl, requestInit) — looks up the registered handler, builds
//     a real Request from {method, url, headers, body}, awaits the handler's
//     Response, and returns {status, headers: [[k, v], ...], body: Uint8Array}.
//
// Errors thrown by the handler surface to Rust as the rejected promise, which
// the dispatch path turns into a 500 with the message in the body.
//
//   registerR2Sources(sources) — R444: forwards resolved R2 coordinates to
//     the `@mesofact/runtime` shim's source registry. Importing the shim here
//     (rather than waiting for the first route's `import "@mesofact/runtime"`)
//     forces its module instance to exist before any route registers, so
//     registration always lands regardless of which route imports the barrel
//     first — deno_core's module map caches by resolved specifier, so every
//     later `import "@mesofact/runtime"` (this harness's or a route bundle's)
//     resolves to this same instance.
//
//   invoke(modUrl, hookName, input) — Mode 2, the endpoint-callback verb
//     (R756-F3 / W311 §2). `input`/return are plain JSON on the Rust side —
//     no header vec, no byte body crossing the boundary. `modUrl` names a
//     module already through `register`; which module that is comes from the
//     Rust-side hook registry (`mesofact::ssr::SsrChild`), populated from the
//     manifest's `hooks` block — the `defineRoutes` declaration site R756-F6
//     added — or, for `readyz` only, from the route-claim opt-in that
//     predates it.
//
//     What varies per hook is only how the module's default export is
//     CALLED, which is what `HOOK_ADAPTERS` below encodes. Everything else —
//     which hooks exist, which module each one resolves to — is the
//     declaration site's business, not the harness's.

import * as mesofactRuntime from "mesofact-ssr:runtime";

const handlers = new Map();

// Mode 2 hook adapters (R756-F6). One entry per hook name the engine may
// invoke; the entry is the calling convention for that hook's module.
//
// A hook added from here on should be plain Mode 2 — `(input) => verdict`,
// JSON both ways, no web-platform objects constructed to move a verdict — and
// its entry is therefore the one-liner `(fn, input) => fn(input)`. `readyz` is
// the exception, and only because its public TSX contract shipped first
// (R756-F5's `defineReadyz` returns a Fetch handler, and a bare
// `async () => new Response("ok")` is documented as working); W311's non-goals
// put changing that out of scope, so the adaptation lives here rather than in
// app code.
//
// Adding a hook is three edits, one per contract it touches: the name in
// `@mesofact/runtime`'s HOOK_NAMES (what an app may declare), an entry here
// (how its module is called), and the Rust call site (when it runs).
const HOOK_ADAPTERS = {
  readyz: fetchHandlerHook,
};

/** `readyz`: the module is a Fetch handler, and the verdict is its status.
 *  Builds a minimal `Request` from `{method, url}` — no headers, no body,
 *  because `/readyz` never carried either — and cancels rather than buffers
 *  the response body nobody reads. */
async function fetchHandlerHook(fn, input) {
  const req = new Request(input.url, { method: input.method ?? "GET" });
  const resp = await fn(req);
  if (resp.body) {
    await resp.body.cancel();
  }
  return { status: resp.status };
}

globalThis.__mesofact_ssr = {
  async registerR2Sources(sources) {
    mesofactRuntime.registerR2Sources(sources);
  },

  async register(modUrl) {
    const mod = await import(modUrl);
    const fn = mod.default;
    if (typeof fn !== "function") {
      // Shared by route entrypoints and Mode 2 hook modules — what the
      // function is *called with* differs (`dispatch` passes a `Request`, a
      // hook goes through `HOOK_ADAPTERS`), but both need a callable default.
      throw new Error(
        `SSR module ${modUrl}: default export must be a function (got ${typeof fn})`,
      );
    }
    handlers.set(modUrl, fn);
  },

  async dispatch(modUrl, init) {
    const fn = handlers.get(modUrl);
    if (!fn) {
      throw new Error(`SSR module ${modUrl} not registered`);
    }
    const requestInit = {
      method: init.method,
      headers: init.headers,
    };
    // Request requires body to be absent on GET/HEAD; serializers on the Rust
    // side already enforce this, but be defensive.
    if (
      init.body !== undefined &&
      init.body !== null &&
      init.method !== "GET" &&
      init.method !== "HEAD"
    ) {
      requestInit.body = new Uint8Array(init.body);
    }
    const req = new Request(init.url, requestInit);
    const resp = await fn(req);
    const bodyBuf = await resp.arrayBuffer();
    const outHeaders = [];
    for (const [k, v] of resp.headers) {
      outHeaders.push([k, v]);
    }
    return {
      status: resp.status,
      headers: outHeaders,
      body: new Uint8Array(bodyBuf),
    };
  },

  async invoke(modUrl, hookName, input) {
    const fn = handlers.get(modUrl);
    if (!fn) {
      throw new Error(`SSR module ${modUrl} not registered`);
    }
    const adapter = Object.prototype.hasOwnProperty.call(HOOK_ADAPTERS, hookName)
      ? HOOK_ADAPTERS[hookName]
      : undefined;
    if (!adapter) {
      // Loud rather than "call it and hope": a hook that silently returns
      // garbage would be read as a verdict, and for `readyz` that means an
      // unready process reporting ready.
      throw new Error(
        `SSR module ${modUrl}: unknown hook "${hookName}" ` +
          `(known: ${Object.keys(HOOK_ADAPTERS).join(", ")})`,
      );
    }
    return adapter(fn, input);
  },
};
