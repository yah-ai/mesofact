// Bootstrap script executed once per SSG runtime, before any module loads.
// Bare deno_core ships no web/node globals; the prerender contract only
// needs the small surface below (react-dom/server's browser build + typical
// render code). Anything bigger should be a deliberate decision, not an
// accretion — the dev/prod SSR tiers run real runtimes (Bun today, a fuller
// deno_core preset later); SSG is build-time rendering only.
"use strict";
((globalThis) => {
  const core = globalThis.Deno?.core;
  const print = (msg) => {
    try {
      core?.print(`${msg}\n`, false);
    } catch {
      /* no-op when core print is unavailable */
    }
  };
  if (globalThis.console === undefined) {
    const fmt = (args) =>
      args
        .map((a) => {
          if (typeof a === "string") return a;
          try {
            return JSON.stringify(a);
          } catch {
            return String(a);
          }
        })
        .join(" ");
    globalThis.console = {
      log: (...a) => print(fmt(a)),
      info: (...a) => print(fmt(a)),
      warn: (...a) => print(fmt(a)),
      error: (...a) => print(fmt(a)),
      debug: () => {},
    };
  }

  // Renders run with NODE_ENV=production (the Rolldown `define` already
  // inlined the hot paths; this covers dynamic reads).
  if (globalThis.process === undefined) {
    globalThis.process = { env: { NODE_ENV: "production" } };
  }

  // Minimal UTF-8 TextEncoder/TextDecoder — react-dom's browser build
  // references TextEncoder from its streaming paths even when only
  // renderToString is exercised.
  if (globalThis.TextEncoder === undefined) {
    globalThis.TextEncoder = class TextEncoder {
      get encoding() {
        return "utf-8";
      }
      encode(input = "") {
        const s = String(input);
        const out = [];
        for (const ch of s) {
          const cp = ch.codePointAt(0);
          if (cp < 0x80) out.push(cp);
          else if (cp < 0x800) out.push(0xc0 | (cp >> 6), 0x80 | (cp & 0x3f));
          else if (cp < 0x10000)
            out.push(0xe0 | (cp >> 12), 0x80 | ((cp >> 6) & 0x3f), 0x80 | (cp & 0x3f));
          else
            out.push(
              0xf0 | (cp >> 18),
              0x80 | ((cp >> 12) & 0x3f),
              0x80 | ((cp >> 6) & 0x3f),
              0x80 | (cp & 0x3f),
            );
        }
        return new Uint8Array(out);
      }
    };
  }
  if (globalThis.TextDecoder === undefined) {
    globalThis.TextDecoder = class TextDecoder {
      get encoding() {
        return "utf-8";
      }
      decode(input) {
        if (input === undefined) return "";
        const bytes = new Uint8Array(input.buffer ?? input);
        let out = "";
        let i = 0;
        while (i < bytes.length) {
          const b = bytes[i];
          let cp;
          if (b < 0x80) {
            cp = b;
            i += 1;
          } else if (b < 0xe0) {
            cp = ((b & 0x1f) << 6) | (bytes[i + 1] & 0x3f);
            i += 2;
          } else if (b < 0xf0) {
            cp = ((b & 0x0f) << 12) | ((bytes[i + 1] & 0x3f) << 6) | (bytes[i + 2] & 0x3f);
            i += 3;
          } else {
            cp =
              ((b & 0x07) << 18) |
              ((bytes[i + 1] & 0x3f) << 12) |
              ((bytes[i + 2] & 0x3f) << 6) |
              (bytes[i + 3] & 0x3f);
            i += 4;
          }
          out += String.fromCodePoint(cp);
        }
        return out;
      }
    };
  }

  if (globalThis.self === undefined) globalThis.self = globalThis;

  // queueMicrotask exists in V8 via the core; setTimeout does not exist in
  // bare deno_core. Render code must not depend on timers at build time —
  // fail loudly instead of hanging the build.
  if (globalThis.setTimeout === undefined) {
    globalThis.setTimeout = () => {
      throw new Error(
        "setTimeout is not available during mesofact SSG (build-time prerender); move timer-dependent work out of render()",
      );
    };
    globalThis.clearTimeout = () => {};
  }
})(globalThis);
