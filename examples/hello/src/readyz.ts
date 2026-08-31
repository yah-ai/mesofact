// The dual-language seam, from the TSX side.
//
// mesofact already serves `/readyz` in Rust — it knows whether the SSR isolate
// booted, whether there is a tree to serve, and whether the process is
// draining. Declaring `hooks: { readyz: "src/readyz.ts" }` in
// `mesofact.routes.ts` opts this app into *contributing* to that answer.
//
// This is Mode 2 (W311 §2): an endpoint callback, not a route. mesofact owns
// the HTTP — the status code, the wire format, the path — and this module
// contributes a verdict. Claiming `/readyz` as a `mode:"ssr"` route still
// works and means the same thing (it is how the seam shipped), but the hook
// declaration is the one to reach for: the module never becomes a route, so
// it stays out of `ssr_prefixes`, the edge never forwards `/readyz` to the SSR
// origin on its account, and it is never shadowed by the Rust probe route
// mounted on the same path.
//
// The Rust handler invokes this while answering the probe and folds the status
// in as a check named `app`. 2xx = ready, anything else takes the pod out of
// rotation. The contribution is additive — a 200 from here cannot un-drain a
// terminating process or overrule a dead isolate, because those are the states
// where app code is the least reliable narrator.
//
// `defineReadyz` emits the same `[+]name ok` listing the Rust side emits under
// `?verbose`, so `curl /readyz?verbose` reads identically whichever language
// answered. Vanilla works too — it's a plain Fetch handler:
//
//   export default async () =>
//     ready() ? new Response("ok") : new Response("warming", { status: 503 });

import { defineReadyz } from "@mesofact/runtime";

// Stand-in for the thing a real app would gate on: a connection pool, a warmed
// cache, a migration that must finish before this replica takes traffic. The
// example has none, so it fakes one that is ready 2s after module init —
// enough to actually observe a 503 → 200 transition with `curl`.
const bootedAt = Date.now();
const WARMUP_MS = 2_000;

export default defineReadyz([
  {
    name: "warmup",
    check: () => Date.now() - bootedAt >= WARMUP_MS,
  },
]);
