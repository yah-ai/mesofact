// SSR bootstrap (W174 pillar 4 / R449-F2, lean since R750-F1). Runs once per
// SsrRuntime startup before any route module loads, as the first of the
// three scripts in `ssr::SSR_PRELUDE`:
//
//   1. this file        — timers, atob/btoa, process.env
//   2. js/bootstrap.js  — the SSG polyfills shared verbatim: console,
//                         TextEncoder/TextDecoder, self
//   3. js/ssr_fetch_shim.js — URL, URLSearchParams, Headers, Request,
//                         Response, fetch (over op_mesofact_fetch)
//
// Order matters: bootstrap.js installs a *throwing* setTimeout when none
// exists (correct for build-time SSG), so the real one must land first.
//
// No deno extension JS is loaded any more — everything here is either
// deno_core's own built-ins or plain JS. W225 §2b measured the surface.
"use strict";
((globalThis) => {
  const core = globalThis.Deno.core;

  // R746-S4: timers are legitimate in a per-request handler — a Promise.race
  // timeout, a retry delay — and the dispatch path drives the event loop
  // (`with_event_loop_promise`), so a pending timer actually fires. Backed by
  // deno_core's own timer wheel (core.createTimer), not deno_web.
  const timers = new Map();
  globalThis.setTimeout = (callback, delay = 0, ...args) => {
    if (typeof callback !== "function") {
      throw new TypeError("setTimeout: callback must be a function");
    }
    let id;
    const timer = core.createTimer(
      () => {
        timers.delete(id);
        callback(...args);
      },
      delay,
      undefined,
      false,
      true,
    );
    id = timer._timerId;
    timers.set(id, timer);
    return id;
  };
  globalThis.clearTimeout = (id) => {
    const timer = timers.get(id);
    if (timer) {
      timers.delete(id);
      core.cancelTimer(timer);
    }
  };
  // setInterval/clearInterval are deliberately NOT bound. A repeating timer
  // started by a request handler outlives its response and leaks into every
  // later request THIS isolate serves — and (R756-F2) a process now runs a
  // pool of isolates, round-robined per request, so module-level state of any
  // kind (not just timers) does not reliably survive from one request to the
  // next: consecutive requests may land on different isolates. A
  // request-scoped handler has no business scheduling repeating work or
  // caching state at module scope; if a real need appears, it belongs in Rust
  // beside the isolate pool, not inside any one isolate.

  // atob/btoa over Latin-1 strings, per the HTML spec.
  const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  globalThis.btoa = (input) => {
    const s = String(input);
    let out = "";
    for (let i = 0; i < s.length; i += 3) {
      const a = s.charCodeAt(i);
      const b = s.charCodeAt(i + 1);
      const c = s.charCodeAt(i + 2);
      if (a > 255 || b > 255 || c > 255) {
        throw new DOMExceptionLike("btoa: string contains characters outside Latin-1", "InvalidCharacterError");
      }
      const n = (a << 16) | ((b || 0) << 8) | (c || 0);
      out += B64[n >> 18] + B64[(n >> 12) & 63];
      out += i + 1 < s.length ? B64[(n >> 6) & 63] : "=";
      out += i + 2 < s.length ? B64[n & 63] : "=";
    }
    return out;
  };
  globalThis.atob = (input) => {
    let s = String(input).replace(/[\t\n\f\r ]/g, "");
    if (s.length % 4 === 0) s = s.replace(/==?$/, "");
    if (s.length % 4 === 1 || /[^A-Za-z0-9+/]/.test(s)) {
      throw new DOMExceptionLike("atob: invalid base64", "InvalidCharacterError");
    }
    let out = "";
    let bits = 0;
    let acc = 0;
    for (const ch of s) {
      acc = (acc << 6) | B64.indexOf(ch);
      bits += 6;
      if (bits >= 8) {
        bits -= 8;
        out += String.fromCharCode((acc >> bits) & 255);
      }
    }
    return out;
  };
  // No DOMException without deno_web; an Error carrying the same `name` is
  // what route code can observe.
  function DOMExceptionLike(message, name) {
    const e = new Error(message);
    e.name = name;
    return e;
  }

  // process.env shim — the Rust side folds the caller's env onto it (R444).
  // Routes that read it get undefined rather than a TypeError on `process`.
  if (globalThis.process === undefined) {
    globalThis.process = { env: { NODE_ENV: "production" } };
  }
})(globalThis);
