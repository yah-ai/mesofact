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

const handlers = new Map();

globalThis.__mesofact_ssr = {
  async register(modUrl) {
    const mod = await import(modUrl);
    const fn = mod.default;
    if (typeof fn !== "function") {
      throw new Error(
        `SSR module ${modUrl}: default export must be a Fetch handler function (got ${typeof fn})`,
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
};
