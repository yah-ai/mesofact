# @mesofact/edge

The manifest-driven Cloudflare Worker that fronts every mesofact site. This is
the **versioned serving artifact** — yah's cloud reconciler sources the built
`dist/router.bundle.js` from here instead of maintaining a camp-local worker
(W270 §3). It supersedes the old `oss/yubaba/crates/cloud/worker/router.ts`.

## What it does

Configured per site by plain-text bindings **plus the published manifest** (read
lazily, only on a static miss):

- **Static / SPA / SSR routing** — assets fetched from `ASSET_ORIGIN`; SPA/SSR
  fall back to the `index.html` shell; SSR prefixes proxied to `SSR_ORIGIN`
  (segment-aware match, W173) with W181 retry/timeout resilience.
- **Backend API routing** — `/api/issues*` → `ISSUES_ORIGIN`, `/api/releases*`
  → `MESOFACT_BACKEND_ORIGIN` (R455-T4).
- **Instance-addressed routes** — a path matching a `prerender: { deferred: true }`
  route (R595-F2) resolves through the **pointer store**: `present` → the
  render-root bytes with immutable cache headers, `deleted` → `410`, `absent` →
  the manifest's `error_routes.404` page. The pointer key is the request path
  minus its leading slash (`/c/abc` → `c/abc`); the publisher flips the same key.
- **`error_routes`** — the manifest's branded `404` / `5xx` pages are served
  (falling back to a legacy `404.html`, then plaintext), retiring the old
  hardcoded plaintext 404.

## Bindings

| Binding | Meaning |
|---|---|
| `ASSET_ORIGIN` | base URL for build-output static assets (no trailing slash) |
| `POINTER_ORIGIN` | base URL for pointer reads (`p/<key>`); defaults to `ASSET_ORIGIN` |
| `UPLOAD_ORIGIN` | `/uploads/*` origin (reserved seam; absent → 404) |
| `WORKER_MODE` | `static` \| `spa` \| `ssr` |
| `SSR_ORIGIN` | SSR proxy origin (empty for non-SSR modes) |
| `SSR_PREFIXES` | JSON array of SSR prefixes (escape hatch; normally manifest-derived) |
| `SSR_RESILIENCE` | JSON `{ [prefix]: ResiliencePolicy }` (W181 v1) |
| `MESOFACT_BACKEND_ORIGIN` | almanac surface for `/api/releases*` |
| `ISSUES_ORIGIN` | issue-tracker surface for `/api/issues*` |

## Build & test

```sh
bun run typecheck      # tsc --noEmit
bun run build          # emits dist/router.bundle.js
bun test               # miniflare (workerd) integration tests
```

`src/pointer.ts` is a lockstep TS mirror of
`oss/mesofact/crates/mesofact-publisher/src/pointer.rs` — the pointer record
shape, key rules, and version must change in both.

## Consumed by yah

`scripts/sync-mesofact-edge-worker.sh` (in the yah monorepo root) builds this
package and vendors `dist/router.bundle.js` into
`oss/yubaba/crates/cloud/worker/router.bundle.js`, which the reconciler embeds
via `include_str!`. This keeps yubaba standalone-exportable across the OSS mirror
boundary while mesofact remains the source of truth.
