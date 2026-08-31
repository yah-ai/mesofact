// Mode 2 hook declaration — W311 §2 "Open Decision 1", answered by R756-F6.
//
// # What a hook is
//
// mesofact's TSX override surface has two modes (W311 §2). Mode 1 is the
// general API: `mode:"ssr"` route, `Request -> Response`, the app owns the
// wire. Mode 2 is the endpoint callback: a small `input -> verdict`, and
// **Rust** owns the HTTP. R756-F3 shipped Mode 2's wire verb
// (`__mesofact_ssr.invoke(bundle, hookName, input)`) but hardcoded its single
// hook name, because `/readyz` — Mode 2's only consumer — could reuse the
// route-claim opt-in it already had. A hook that is *not* a path (`onRequest`,
// `onError`, an auth gate) has no route to claim, so it needs a declaration
// site. This is that site.
//
// # Why a top-level `hooks` key, and not a per-route `middleware` field
//
// W311 named two candidates and deliberately picked neither. The `hooks` key
// wins on the doc's own terms:
//
//   - A per-route `middleware` field is a *chain*, and "a general TS
//     middleware chain" is an explicit W311 non-goal — it has ordering
//     semantics (what runs before what, who may short-circuit, how errors
//     propagate) that Mode 2 does not have and does not want.
//   - Mode 2's defining property is that Rust owns the HTTP. A hook is
//     therefore engine-addressed — the engine decides when to call `readyz`,
//     not a route table — so the declaration belongs beside `routes`, not
//     inside one.
//   - Hook names are a **closed set** for the same reason: only the engine
//     invokes a hook, so a name the engine does not know is dead code and a
//     silent typo. Adding one is three deliberate edits — a name here, an
//     adapter in `crates/mesofact-ssr/js/ssr_harness.js`, and the Rust call
//     site that decides when it runs — and each is a real contract statement.
//
// # Adding a hook
//
// Add the name to `HOOK_NAMES`, teach `ssr_harness.js` how to call it (the
// default is plain Mode 2 — `(input) => verdict`, JSON both ways), and write
// the Rust call site. `hooks` in the manifest carries name → bundled module;
// `mesofact::ssr::spawn` registers each one on every isolate in the pool and
// `SsrChild::invoke_hook(name, input)` calls it.

/** Every hook name the engine knows how to invoke.
 *
 *  `readyz` is the one hook that predates this declaration site, so it has two
 *  ways in (see {@link HOOK_ROUTE_CLAIMS}) and its module's contract is a
 *  Fetch handler rather than a plain Mode 2 function. Hooks added from here on
 *  are `(input) => verdict`. */
export const HOOK_NAMES = ["readyz"] as const;

export type HookName = (typeof HOOK_NAMES)[number];

/** The `hooks` block of `defineRoutes` — hook name → entrypoint path, relative
 *  to the project root, bundled to `dist/server/hooks/<name>.js`. */
export type HooksConfig = { readonly [K in HookName]?: string };

/** Hooks that may *alternatively* be declared by claiming a route, which is
 *  how `/readyz` shipped (R756-F5) before hooks existed.
 *
 *  Route-claiming still works and is still the shortest path for an app that
 *  wants its readiness handler reachable as an ordinary Fetch handler. The
 *  `hooks` declaration is the better one for everything else: the module is
 *  not a route, so it never enters `ssr_prefixes`, is never forwarded to the
 *  SSR origin by the edge Worker, and is never shadowed by the Rust probe
 *  route it would otherwise collide with. Declaring both is rejected — two
 *  opt-ins for one verdict is ambiguous, not additive. */
export const HOOK_ROUTE_CLAIMS: { readonly [K in HookName]?: string } = {
  readyz: "/readyz",
};

export function isHookName(name: string): name is HookName {
  return (HOOK_NAMES as readonly string[]).includes(name);
}
