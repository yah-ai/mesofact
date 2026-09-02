# cache_policy parity fixture

`parity.json` is the **one** description of what a route's declared
`cache_policy` must turn into, exercised against every tier that turns it into
anything:

| Tier | Suite | Entry point |
|---|---|---|
| `mesofact serve` / the W272 bundle tier | `crates/mesofact-core/tests/cache_policy_parity.rs` | `CachePolicyTable::apply` (the axum middleware's whole body) |
| `mesofact publish` → CDN object | same file | `CachePolicyTable::cache_control_for` |
| the Cloudflare Worker | `packages/mesofact-edge/tests/cache-policy-parity.test.ts` | `pageCacheHeaders` → `withPageCache` |

Why one fixture rather than three suites that happen to agree: a declared TTL
that means one thing at the origin, another on the object, and a third at the
edge is not a policy — it is three numbers with one name. That is not
hypothetical. Before R749-B4 the publisher picked `Cache-Control` from the path
prefix (`html/` → `max-age=86400`) and the Worker overwrote every page with a
flat `no-cache`, so a route declaring `{ ttl: 3600 }` was honoured by both
servers and by neither thing in front of them, with no error anywhere. R749-T1
had already closed the same shape one tier over. A single fixture asserted by
all three is what makes the next divergence a test failure instead of a
production surprise.

Shape:

```jsonc
{
  "routes": [ { "route", "requires"?, "cache_policy" }, … ],   // the manifest slice
  "cases":  [ { "name", "path", "cache_control": string|null,  // a 2xx page
                "vary"?: string } ],
  "negative_cases": [ { "name", "path", "status", "cache_control" } ]
}
```

`cache_control: null` means the tier must emit **nothing** — the tier's own
default stands. That is a real assertion, not a skipped one: `{ ttl: 0 }` is the
inert policy every generated route file writes, and a tier that turned it into
`max-age=0` would make every site's pages uncacheable the moment this table
started being read.

## Why `negative_cases` is Rust-only

`negative_ttl` describes the lifetime of *this route's* miss. Both Rust tiers can
honour that, because there the miss is the route's own handler answering. The
Worker's 404/410 path (`errorResponse`) serves a **different route's** bytes —
the manifest's branded `error_routes` page — so stamping the requested route's
negative TTL onto it would attach one route's policy to another route's
response. The edge derives page headers only for a 200. Documented at the top of
`packages/mesofact-edge/src/cache-policy.ts`; if the edge ever grows a per-route
miss response, these cases move up into `cases`.
