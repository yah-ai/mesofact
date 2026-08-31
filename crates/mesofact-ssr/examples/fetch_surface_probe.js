// R637 — Fetch-surface probe. Executed once, AFTER ssr_bootstrap.js has
// materialised the Fetch globals and BEFORE the harness module loads, so every
// subsequent touch of the Fetch API by the harness bridge or by real route code
// is recorded.
//
// What it records, per member:
//   - `new Foo(<argshape>)`             — constructor calls, with arg shapes
//   - `Foo.prototype.bar()`             — method calls
//   - `Foo.prototype.baz`               — accessor (getter) reads
//   - `globalThis.Foo`                  — the global binding being read at all
//   - phases it was touched in, and up to 8 distinct call sites (stack frames)
//
// The call site is what separates "the Rust↔JS bridge needs this" from "the
// route author's code needs this": frames in `mesofact-ssr:harness` are the
// bridge, frames in a `file:///…/dist/server/*.js` are real route code.
"use strict";
((g) => {
  const PROBE_SCRIPT = "mesofact-ssr:probe";
  const hits = new Map();
  let phase = "boot";

  function site() {
    const stack = new Error().stack || "";
    for (const line of stack.split("\n").slice(1)) {
      if (line.includes(PROBE_SCRIPT)) continue;
      return line.trim().replace(/^at\s+/, "");
    }
    return "<unknown>";
  }

  function rec(key) {
    let e = hits.get(key);
    if (!e) {
      e = { count: 0, sites: new Set(), phases: new Set() };
      hits.set(key, e);
    }
    e.count++;
    e.phases.add(phase);
    if (e.sites.size < 8) e.sites.add(site());
  }

  // Arg shapes matter more than arg counts: `new Response(<ReadableStream>, {status,headers})`
  // is a materially bigger op-layer obligation than `new Response(<string>, {status})`.
  function describe(v) {
    if (v === undefined) return "undefined";
    if (v === null) return "null";
    const t = typeof v;
    if (t === "string" || t === "number" || t === "boolean" || t === "function") return `<${t}>`;
    if (ArrayBuffer.isView(v)) return `<${v.constructor?.name ?? "TypedArray"}>`;
    if (v instanceof ArrayBuffer) return "<ArrayBuffer>";
    const ctor = v.constructor?.name;
    if (ctor && ctor !== "Object") return `<${ctor}>`;
    const keys = Object.keys(v).sort();
    return keys.length ? `{${keys.join(",")}}` : "{}";
  }

  function argShape(args) {
    return Array.prototype.map.call(args, describe).join(", ");
  }

  function instrumentProto(name, ctor) {
    const proto = ctor.prototype;
    if (!proto) return;
    const keys = [
      ...Object.getOwnPropertyNames(proto),
      ...Object.getOwnPropertySymbols(proto),
    ];
    for (const k of keys) {
      if (k === "constructor") continue;
      const d = Object.getOwnPropertyDescriptor(proto, k);
      if (!d || !d.configurable) continue;
      const label = `${name}.prototype.${typeof k === "symbol" ? String(k) : k}`;
      if (typeof d.value === "function") {
        const orig = d.value;
        const wrapper = function (...args) {
          rec(`${label}()`);
          return orig.apply(this, args);
        };
        try {
          Object.defineProperty(wrapper, "name", { value: orig.name, configurable: true });
        } catch { /* name is not always redefinable */ }
        Object.defineProperty(proto, k, {
          configurable: true,
          enumerable: d.enumerable,
          writable: d.writable,
          value: wrapper,
        });
      } else if (typeof d.get === "function") {
        const og = d.get;
        const os = d.set;
        Object.defineProperty(proto, k, {
          configurable: true,
          enumerable: d.enumerable,
          get() {
            rec(label);
            return og.call(this);
          },
          set: os
            ? function (v) {
                rec(`${label} (set)`);
                return os.call(this, v);
              }
            : undefined,
        });
      }
    }
  }

  // Reading the global itself is recorded too — that is the cheapest signal for
  // "does route code even know this name exists".
  function defineRecordedGlobal(name, value) {
    let v = value;
    try {
      Object.defineProperty(g, name, {
        configurable: true,
        get() {
          rec(`globalThis.${name}`);
          return v;
        },
        set(nv) {
          v = nv;
        },
      });
    } catch {
      // Non-configurable global (deno_core installs a few); leave the original
      // binding in place — prototype instrumentation still records real use.
      g[name] = v;
    }
  }

  function instrumentClass(name) {
    const orig = g[name];
    if (typeof orig !== "function") {
      hits.set(`ABSENT globalThis.${name}`, { count: 0, sites: new Set(), phases: new Set() });
      return;
    }
    instrumentProto(name, orig);
    const proxy = new Proxy(orig, {
      construct(target, args) {
        rec(`new ${name}(${argShape(args)})`);
        return Reflect.construct(target, args, target);
      },
      apply(target, thisArg, args) {
        rec(`${name}(${argShape(args)}) [called]`);
        return Reflect.apply(target, thisArg, args);
      },
    });
    defineRecordedGlobal(name, proxy);
  }

  function instrumentFunction(name) {
    const orig = g[name];
    if (typeof orig !== "function") {
      hits.set(`ABSENT globalThis.${name}`, { count: 0, sites: new Set(), phases: new Set() });
      return;
    }
    const wrapped = function (...args) {
      rec(`${name}(${argShape(args)})`);
      return orig.apply(this, args);
    };
    defineRecordedGlobal(name, wrapped);
  }

  // The Fetch/Web surface ssr_bootstrap.js installs, plus the ambient globals a
  // route could reach for instead.
  const CLASSES = [
    "Request",
    "Response",
    "Headers",
    "FormData",
    "Blob",
    "File",
    "FileReader",
    "URL",
    "URLSearchParams",
    "AbortController",
    "AbortSignal",
    "TextEncoder",
    "TextDecoder",
    "TextEncoderStream",
    "TextDecoderStream",
    "ReadableStream",
    "WritableStream",
    "TransformStream",
    "Event",
    "EventTarget",
    "DOMException",
  ];
  const FUNCTIONS = ["fetch", "atob", "btoa", "structuredClone", "queueMicrotask", "setTimeout"];
  const VALUES = ["process", "performance", "console", "crypto", "Deno"];

  for (const c of CLASSES) instrumentClass(c);
  for (const f of FUNCTIONS) instrumentFunction(f);
  for (const v of VALUES) {
    if (g[v] === undefined) {
      hits.set(`ABSENT globalThis.${v}`, { count: 0, sites: new Set(), phases: new Set() });
      continue;
    }
    defineRecordedGlobal(v, g[v]);
  }

  g.__probe = {
    setPhase(p) {
      phase = p;
    },
    dump() {
      const out = [];
      for (const [key, e] of hits) {
        out.push({
          member: key,
          count: e.count,
          phases: [...e.phases].sort(),
          sites: [...e.sites].sort(),
        });
      }
      out.sort((a, b) => (a.member < b.member ? -1 : a.member > b.member ? 1 : 0));
      return out;
    },
  };
})(globalThis);
