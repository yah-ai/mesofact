// SSR bootstrap (W174 pillar 4 / R449-F2). Runs once per SsrRuntime startup
// before any route module loads. Materialises the Fetch API globals out of
// deno_web + deno_fetch's lazy-loaded JS modules.
//
// Each `core.loadExtScript("ext:<crate>/<file>.js")` returns the file's IIFE
// export object; the cascade of internal `loadExtScript` calls inside those
// IIFEs wires up the transitive dependency graph (e.g. 23_request.js pulls
// in 00_infra.js, 01_console.js, 01_dom_exception.js automatically). We only
// touch the top-level entry points the dev SSR contract needs:
//
//   - URL / URLSearchParams           (ext:deno_web/00_url.js)
//   - TextEncoder / TextDecoder       (ext:deno_web/08_text_encoding.js)
//   - DOMException                    (ext:deno_web/01_dom_exception.js)
//   - AbortController / AbortSignal   (ext:deno_web/03_abort_signal.js)
//   - Event / EventTarget             (ext:deno_web/02_event.js)
//   - ReadableStream + friends        (ext:deno_web/06_streams.js)
//   - Blob / File / FormData          (ext:deno_web/09_file.js, ext:deno_fetch/21_formdata.js)
//   - Headers / Request / Response    (ext:deno_fetch/{20,23}.js)
//   - fetch                           (ext:deno_fetch/26_fetch.js)
//
// `process.env` is installed as an empty record so route code that reads
// env vars (e.g. ISSUE_TRACKER_URL) gets `undefined` cleanly rather than a
// TypeError. The Rust side will populate it before any route runs.
"use strict";
((globalThis) => {
  const core = globalThis.Deno.core;
  const load = (specifier) => core.loadExtScript(specifier);

  // Order matters only for IIFE-evaluation side effects; transitive deps
  // resolve via the loadExtScript memoisation inside each file.
  // deno_fetch/26_fetch.js destructures internals.__telemetry at module
  // load. The real deno_telemetry crate ships its bootstrap as .ts which
  // needs swc transpile we don't want — install minimal no-op stubs so the
  // destructure resolves to functions/values that do nothing. Outbound
  // fetch() still works; only tracing is disabled.
  const noop = () => {};
  const noopRestore = () => {};
  const noopSpan = {
    end: noop,
    setAttribute: noop,
    setAttributes: noop,
    addEvent: noop,
    setStatus: noop,
    updateName: noop,
    recordException: noop,
    isRecording: () => false,
  };
  const noopTracer = { startSpan: () => noopSpan };
  globalThis.__bootstrap.internals.__telemetry = {
    builtinTracer: () => noopTracer,
    ContextManager: class { active() { return null; } with(_ctx, fn) { return fn(); } },
    enterSpan: () => noopRestore,
    restoreSnapshot: () => noopRestore,
    TRACING_ENABLED: false,
    PROPAGATORS: [],
  };
  globalThis.__bootstrap.internals.__telemetryUtil = {
    updateSpanFromRequest: noop,
    updateSpanFromResponse: noop,
  };

  load("ext:deno_web/00_infra.js");
  load("ext:deno_web/01_dom_exception.js");
  load("ext:deno_web/02_event.js");
  load("ext:deno_web/03_abort_signal.js");
  load("ext:deno_web/05_base64.js");
  load("ext:deno_web/06_streams.js");
  load("ext:deno_web/08_text_encoding.js");
  load("ext:deno_web/09_file.js");
  load("ext:deno_web/10_filereader.js");
  load("ext:deno_web/15_performance.js");

  const url = load("ext:deno_web/00_url.js");
  const encoding = load("ext:deno_web/08_text_encoding.js");
  const dom = load("ext:deno_web/01_dom_exception.js");
  const event = load("ext:deno_web/02_event.js");
  const abort = load("ext:deno_web/03_abort_signal.js");
  const streams = load("ext:deno_web/06_streams.js");
  const file = load("ext:deno_web/09_file.js");
  const filereader = load("ext:deno_web/10_filereader.js");
  const performance = load("ext:deno_web/15_performance.js");
  const headers = load("ext:deno_fetch/20_headers.js");
  const formdata = load("ext:deno_fetch/21_formdata.js");
  const request = load("ext:deno_fetch/23_request.js");
  const response = load("ext:deno_fetch/23_response.js");
  const fetch_ = load("ext:deno_fetch/26_fetch.js");

  // Web primitives.
  globalThis.URL = url.URL;
  globalThis.URLSearchParams = url.URLSearchParams;
  globalThis.TextEncoder = encoding.TextEncoder;
  globalThis.TextDecoder = encoding.TextDecoder;
  globalThis.TextEncoderStream = encoding.TextEncoderStream;
  globalThis.TextDecoderStream = encoding.TextDecoderStream;
  globalThis.DOMException = dom.DOMException;
  globalThis.Event = event.Event;
  globalThis.EventTarget = event.EventTarget;
  globalThis.AbortController = abort.AbortController;
  globalThis.AbortSignal = abort.AbortSignal;
  globalThis.ReadableStream = streams.ReadableStream;
  globalThis.WritableStream = streams.WritableStream;
  globalThis.TransformStream = streams.TransformStream;
  globalThis.Blob = file.Blob;
  globalThis.File = file.File;
  globalThis.FileReader = filereader.FileReader;
  globalThis.performance = performance.performance;

  // Fetch surface.
  globalThis.Headers = headers.Headers;
  globalThis.FormData = formdata.FormData;
  globalThis.Request = request.Request;
  globalThis.Response = response.Response;
  globalThis.fetch = fetch_.fetch;

  // process.env shim — populated by the Rust side via execute_script if the
  // dev workload declares env vars. Routes that read it get undefined rather
  // than a TypeError on `process` itself.
  if (globalThis.process === undefined) {
    globalThis.process = { env: { NODE_ENV: "production" } };
  }
  if (globalThis.self === undefined) globalThis.self = globalThis;
})(globalThis);
