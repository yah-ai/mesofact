// @mesofact/edge — `cache_policy` → response headers, at the edge (R749-B4).
//
// Byte-for-byte twin of `crates/mesofact-core/src/cache_policy.rs`; the two are
// held to one fixture by `tests/fixtures/cache-policy/parity.json`. Keep them in
// step — a declared TTL that means one thing behind the Worker and another
// behind passway is the silent no-op the whole R749 relay exists to forbid.
//
//   | declared        | emitted                                    |
//   |-----------------|--------------------------------------------|
//   | ttl: N          | Cache-Control: public, max-age=N           |
//   | swr: N          | , stale-while-revalidate=N                 |
//   | vary: [h, …]    | Vary: h, …                                 |
//   | requires: [...] | `private` instead of `public`              |
//
// `negative_ttl` has no consumer here: this module is only reached for a page
// the build tree actually served (a 200). The 404/410 path at the edge is
// `errorResponse`, which serves a *different* route's bytes — the branded error
// page — so stamping the requested route's negative TTL onto it would attach
// one route's policy to another route's response. The Rust table does apply it,
// because there the miss is still that route's own handler answering.
//
// WHY THE EDGE DERIVES RATHER THAN PASSING THE OBJECT'S HEADER THROUGH.
// `routePage` serves bytes from an IMMUTABLE url (`<build_id>/html/x.html`) at a
// MUTABLE one (`/x`, whose content changes when the build pointer moves).
// Forwarding the object's own `Cache-Control` would let a client hold a stale
// page across a revalidate — the exact freshness bug the build-pointer
// indirection exists to fix, reintroduced one layer down. The manifest is
// already in hand on this path (it is what resolved the pointer), so deriving
// from the declaration costs nothing and cannot inherit a publisher default.

/** `cache_policy` as declared on a route — mirrors `CachePolicyConfig` in
 *  `packages/mesofact-runtime/src/routes.ts`. */
export interface EdgeCachePolicy {
  ttl?: number;
  swr?: number;
  negative_ttl?: number;
  vary?: readonly string[];
}

/** The derived headers for one route, or `null` when it declared nothing that
 *  produces a rule. */
export interface DerivedCacheHeaders {
  cacheControl: string;
  vary?: string;
}

/**
 * `Cache-Control` (+ `Vary`) for a 2xx, from a route's declaration.
 *
 * `ttl: 0` with no `swr` is the inert policy every generated route file writes
 * and yields `null` — the caller keeps whatever it would have sent. `ttl: 0`
 * *with* `swr` is a real declaration ("always revalidate, serve stale while you
 * do"), not the absence of one.
 */
export function derivePageCacheHeaders(
  policy: EdgeCachePolicy | undefined,
  gated: boolean,
): DerivedCacheHeaders | null {
  if (!policy) return null;
  const ttl = typeof policy.ttl === "number" ? policy.ttl : 0;
  const swr = typeof policy.swr === "number" ? policy.swr : undefined;
  if (ttl === 0 && swr === undefined) return null;

  // A gated route marked `public` is a cache-poisoning bug: a shared cache
  // would serve one user's rendered page to the next.
  const scope = gated ? "private" : "public";
  let cacheControl = `${scope}, max-age=${ttl}`;
  if (swr !== undefined) cacheControl += `, stale-while-revalidate=${swr}`;

  const vary =
    Array.isArray(policy.vary) && policy.vary.length > 0
      ? policy.vary.join(", ")
      : undefined;

  return vary === undefined ? { cacheControl } : { cacheControl, vary };
}
