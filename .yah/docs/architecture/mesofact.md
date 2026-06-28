# mesofact — tri-mode web server

> **Name**: `mesofact` — the project lives at `yah/external/mesofact/`.

> **Status**: design draft, 2026-05-13 (revised 2026-05-14). Captures
> the render-axis design for a small Rust+Bun web server that handles
> static / SSR / SPA from one project with per-route mode selection,
> and the source-axis (global vs. scoped) it crosses against. Customer
> application-tier mappings (e.g. noisetable T0–T4) live in
> sibling case-study docs.
>
> **Shape**: external crate workspace under `yah/external/mesofact/`
> (same pattern as [`external/xlb/`](../../../../xlb/) — its own
> Cargo workspace, gitignored from yah, intended to move to its own
> repo later). yah consumes it as the first dogfood to build the
> yah.dev marketing page.
>
> **Why external**: the substrate is not yah-specific. yah is the
> first customer (marketing page), noisetable is the obvious second
> (published-package + per-project SSR), and the design is meant to
> be usable outside both. Same posture as xlb.

## Goal

One project, one render pipeline, one deployment story — serving
three traffic shapes:

| Mode | Shape | Freshness | Scale target | Mechanism |
|---|---|---|---|---|
| **1. Static** | Marketing, docs, landing | minutes–days | 10 M concurrent | **Push** rendered HTML to R2 behind a CDN |
| **2. SSR** | Status, dashboards, logged-in shells | seconds | 10 k–100 k concurrent | **Pull** through Rust proxy → Bun render pool, short TTL |
| **3. SPA** | The app itself | per-user, live | 1 k DAU realistically | Static shell + client-side fetch to API |

Per-route config picks the mode. Routes can move between modes
without rewriting them.

## Primary job (sharper framing)

**Coordinate per-route mode escalation between slow-moving and
fast-moving content that share a single base URL.** A `/` homepage
that wants to be static, a `/docs/*` tree that wants static-with-
tag-invalidation, a `/dashboard` view that wants SSR, and an `/app`
shell that wants SPA can all live in one project and ship as one
deployment — without picking one mode for the whole site.

The escalation between modes happens **on the same base URL via a
menu click or a navigation**. That continuity is the load-bearing
property. Switching to a desktop app (download + launch) is a
*hard* delivery boundary, not an escalation; that boundary is
outside mesofact's scope.

## Non-goals

- Not a framework. Routes are hand-written; no file-based routing magic.
- Not streaming SSR in v1. Render is request → full response.
- Not Bun-on-the-edge. Bun pool lives next to Rust proxy in one
  region; CDN handles the geography for Mode 1.
- Not a Next.js / Nuxt replacement. The market is "okay at all three,
  one server" — vendors prefer to sell you two products because the
  10 M-concurrent static tier and the 1 k-DAU app tier have very
  different cost structures.
- **Not a renderer-anywhere library.** Mesofact's surface is HTTP
  delivery. The render contract is a TS function so a render
  entrypoint can theoretically run anywhere, but the *coordination*
  mesofact provides — per-route mode dispatch, cache invalidation,
  publisher pipeline, manifest contract — is HTTP-bound. Rendering
  the same TS components inside a desktop app (Tauri, Electron,
  etc.) bypasses mesofact entirely; the shared-component package
  is the integration point, not mesofact. Desktop is a hard
  delivery boundary, not a fourth mode.

## Why not rari / Next / etc.

Investigated and rejected:

- **rari** — ships a SPA shell + SSR bundle the rari binary reads at
  request time; not per-route pre-rendered HTML. To get pure static
  HTML we'd fight the framework. Embedding rari to own the cache
  pulls in the full Deno tree and loses the seam.
- **Next.js long-TTL SSR as a static substitute** — even with ISR +
  in-memory cache you're paying a node process per region, cold starts,
  and cache-stampede risk on invalidation. R2/S3 behind a CDN is
  effectively free at 10 M concurrent and genuinely DDoS-shaped. The
  "Next is fine" benchmarks are mostly at 1 k–10 k RPS.
- **Cloudflare Pages + Workers** — closest commercial shape (R2 for
  static, Workers for SSR, SPA fallback is one config line). Rejected
  because we'd rent their runtime instead of owning the seam, and the
  per-route mode-switch ergonomics aren't first-class.

## Architecture

```
                    ┌──────────────────────┐
                    │  Bun build pipeline  │
                    │  (Vite + render fn)  │
                    └──────────┬───────────┘
                               │ emits
                ┌──────────────┼──────────────┐
                ▼              ▼              ▼
        manifest.json    static HTML     SSR bundle
                              │                │
                              ▼                │
                     ┌────────────────┐        │
                     │   R2 + CDN     │        │
                     └────────────────┘        │
                                               ▼
   ┌──────────────────┐         ┌─────────────────────┐
   │  Rust proxy      │────────▶│  Bun render pool    │
   │  (axum)          │  IPC    │  (long-lived procs) │
   │  - route table   │         └─────────────────────┘
   │  - response cache│
   │  - mode dispatch │
   └──────────────────┘
          ▲
          │ public traffic for Mode 2 + 3
```

## The shared seam: one render contract

```ts
// every route — static, SSR, or SPA shell — exports this
export async function render(req: RenderRequest): Promise<RenderResult> {
  return { html, headers, cache: { ttl, tags } }
}
```

- **Mode 1** calls `render()` at build/publish time; output pushed to R2.
- **Mode 2** calls `render()` per request via the Bun pool; Rust caches the result.
- **Mode 3** calls `render()` once at build to emit the shell; client takes over after hydration.

Same function, three drivers. **This is the load-bearing design choice.**

`cache.tags` carries the read-set tags the publisher uses to
invalidate Mode 1 HTML on backend change (see §"Adapter read-set
provenance"). Source-level generation tokens feed the Mode 2 cache
key separately (see §"Cache-key composition") — the contract never
mentions specific data-tier labels.

## Request context — what Rust pre-resolves

`RenderRequest` carries the raw HTTP shape *plus* a small set of
pre-resolved fields the Rust proxy fills in before invoking render
(user from session, project from URL/scope, region from local
deployment). The proxy owns these lookups because it already owns
cache + policy and will own home-region routing later (see §"Render
axis × source axis").

```ts
type RenderRequest = {
  // raw HTTP
  url: string
  params: Record<string, string>
  query: Record<string, string>
  headers: Record<string, string>
  cookies: Record<string, string>

  // Rust-resolved (proxy populates before calling render)
  user?: User           // session → user (auth middleware in Rust)
  project?: Project     // (scope, id) → home_region, generation
  region?: Region       // which region this Bun pool is running in

  // escape hatch for route-specific Rust middleware
  ctx?: Record<string, unknown>
}
```

Routes declare which fields they require in the manifest:

```json
{
  "route": "/p/:id",
  "requires": ["user", "project"]
}
```

- **Mode 2 SSR**: if a required field can't resolve (no session for
  `user`; unknown project for `project`), the Rust proxy returns 401 /
  404 / redirect — render is never invoked.
- **Mode 1 static**: `requires: ["user"]` is forbidden at build time
  and the publisher fails. A build can't know who the user is.
- **Mode 3 shell**: `requires` is usually empty or just `region`; the
  client fetches user/project after hydration.

The explicit `user` / `project` / `region` fields lock the
resolution boundary into the contract types so an FE consumer can
see what mesofact pre-resolves. `ctx` is an escape hatch for route-specific
middleware (feature flags, A/B bucket, …) without bloating the
top-level shape, and is *not* type-checked across the proxy↔render
boundary — its keys are a per-deployment convention.

## Data-source seam — adapters, not protocol

`render()` is server-side TS already running in the Bun pool — there
is no external client that needs to bridge sources at runtime, so
there is nothing for a GraphQL layer to centralize. Mesofact ships a
small set of typed data adapters as a TS package; render functions
import them directly:

```ts
import { sqlite, r2, pg } from '@mesofact/runtime'

export async function render(req: RenderRequest): Promise<RenderResult> {
  const project = await sqlite('project_db').get('config', req.params.id)
  const html = renderToString(<ProjectPage project={project} />)
  return {
    html,
    cache: { ttl: 60, tags: [`project:${project.id}`] },
  }
}
```

This is the **"opinionated but not a framework"** posture: render
functions are plain server-side code that can do anything, but the
only sanctioned way to reach a backend is through an adapter mesofact
ships. New backend kinds are an adapter PR, not a render-side
freelance integration.

### Adapter inventory (MVP)

| Adapter | Backend | When |
|---|---|---|
| `r2` | Cloudflare R2 / any S3 | static byte blobs, signed manifests, build-time reads |
| `sqlite` | Local file or LiteFS / Litestream replica | scoped per-project DBs; small global config |
| `pg` | Postgres (per-region) | mutable rows, larger relational data |
| `rpc` | HTTPS to a specific host (typically over a private mesh) | scoped data that lives on a single node with no regional replica; generation token comes from a *paired* roster source, not from the endpoint itself |

That is the lane. Other backends (globally-replicated SQL, message
queues, WS streams) are reached by other components — usually yubaba
out-of-band, or the SPA's own API client — not a render-time
adapter. The "let yubaba own the backend connections" rule means
mesofact never opens a new *kind* of connection on its own.

The `rpc` adapter is the asymmetric one: it doesn't talk to a backend
*kind* (Postgres, R2, …), it talks to a specific host whose address
comes from a **roster** source declared elsewhere in the config. The
canonical case is a hosted-per-tenant service: a yah camp's
`yah-camp` daemon, exposed by yubaba on a private mesh, with the
tenant→host map living in R2 as a signed roster manifest. See the
[yah case study](./mesofact-yah-case-study.md) (yah-remote-camp section).

### Adapters ↔ appliances

Each mesofact adapter wraps an **appliance** — yah's term for a
backend-service-tier component (the things applications talk to for
state, storage, or coordination). The adapter is mesofact's
TS-side interface; the appliance is the running service it talks to.

| Adapter | Appliance |
|---|---|
| `r2` | R2 (object store) |
| `sqlite` | SQLite (embedded; optionally Litestream-replicated) |
| `pg` | Postgres |
| `rpc` | any HTTP-API appliance (yah-camp, yubaba, future services) |

Future appliances (Redis, NATS, …) become future adapters in the
same shape. The adapter inventory is therefore the appliance
integration list.

### Yubaba owns config and credentials

Mesofact reads its data-source config from `mesofact.config.toml`.
Yubaba writes that file atomically and SIGHUPs the proxy + Bun pool
on change:

```toml
[sources.project_db]
kind = "sqlite"
scope = "project"                 # value depends on req.project.id
path = "/var/lib/yah/projects/{project_id}.db"
home_region = "${WARDEN_HOME_REGION}"

[sources.profile_pg]
kind = "pg"
scope = "global"
dsn_env = "PROFILE_PG_DSN"        # yubaba injects the secret as env

[sources.assets]
kind = "r2"
scope = "global"
bucket = "yah-assets"
endpoint_env = "R2_ENDPOINT"
```

`scope` defaults to `"global"`. A `"project"` or `"user"` source is
templated on `req.project.id` / `req.user.id`; the build refuses to
let Mode 1 routes read scoped sources.

Credentials are env vars yubaba injects into the proxy + Bun pool's
environment; mesofact never reads them off disk and never rotates
them. If yubaba fails over a source (new PG replica, new R2 region),
yubaba rewrites the config and SIGHUPs.

### Source read-set lives in the manifest

The build emits, per route, *which sources it touches* — not just
mode + cache policy. The manifest carries:

```json
{
  "route": "/p/:id",
  "mode": "ssr",
  "render_entrypoint": "dist/server/p_id.js",
  "cache_policy": { "ttl": 60 },
  "source_reads": ["project_db"]
}
```

This is a partial view; the full schema lives in §"Manifest schema".

Two payoffs:

1. **Build-time validation** — a Mode 1 route whose `source_reads`
   names any non-`global` source fails the build, before
   stale-forever HTML hits R2.
2. **Cache-key composition** — the proxy looks up each source's
   current generation (file mtime, replica LSN, bucket
   `Last-Modified`, or an adapter-supplied token) and folds it into
   the cache key. Backend bump ⇒ automatic miss, no manual purge.

### Adapter read-set provenance (Mode 1 tag inheritance)

For Mode 1 (and short-TTL Mode 2) invalidation to inherit from the
data the render actually read, adapters need to record *what they
read* so the publisher can tag the output. Threading that through
render code explicitly would be miserable, so it goes through
`AsyncLocalStorage` (Bun supports `node:async_hooks`):

```ts
// inside @mesofact/runtime
const trackCtx = new AsyncLocalStorage<{ tags: Set<string> }>()

// adapter implementation (sketch)
async function sqliteGet(source, table, id) {
  const row = await actualQuery(source, table, id)
  trackCtx.getStore()?.tags.add(`sqlite:${source}:${table}:${id}`)
  return row
}

// invocation, inside mesofact internals (build-time or SSR)
return trackCtx.run({ tags: new Set() }, async () => {
  const result = await render(req)
  return {
    ...result,
    cache: {
      ...result.cache,
      tags: [...(result.cache.tags ?? []), ...trackCtx.getStore()!.tags],
    },
  }
})
```

Render code stays innocent — `await sqlite('x').get(...)` works the
same; tags accumulate transparently. Tag taxonomy:

| Adapter | Tag shape |
|---|---|
| `sqlite` / `pg` | `<kind>:<source>:<table>:<id>` (row) or `<kind>:<source>:<table>` (table-wide) |
| `r2` | `r2:<bucket>:<key>` |

The publisher records `{route, tags}` per Mode 1 emission and
subscribes to backend change events (PG `LISTEN/NOTIFY`, R2 event
notifications, SQLite WAL tail / Litestream replication events). A
matching event re-runs the affected route's render and re-uploads.
Short-TTL Mode 2 entries get the same tag treatment so the in-memory
LRU can be invalidated proactively instead of waiting for TTL.

Adapters expose a `noTrack()` wrapper for reads that shouldn't be
tagged (feature flags, fast-changing counters that would over-purge):

```ts
const flag = await sqlite.noTrack()('flags').get('x')
```

**Default is tracked**, not untracked. Over-invalidation is wasteful
but recoverable (the publisher re-renders); a missed tag on a Mode 1
page becomes stale-forever public HTML, which is a support incident.
`noTrack` is opt-in for the narrow case where the value genuinely
shouldn't gate a re-render.

## yah/ui (and any FE) as a consumer

The render contract is the seam in both directions. A frontend
project — [yah/ui](../../../../packages/yah/ui/), the yah.dev
marketing page, a future customer's app — owns its own build (Vite /
Bun, Tailwind, design tokens, components). It exports a `render(req)`
entrypoint conforming to the contract:

```ts
// packages/yah/ui/src/server.tsx (illustrative)
export async function render(req: RenderRequest): Promise<RenderResult> {
  return { html: renderToString(<App {...props} />), headers: {}, cache: {...} }
}
```

Mesofact's build orchestrates Bun around that entrypoint: discovers
routes from a route-config file, wraps each render with the pool
protocol, emits HTML to R2 for Mode 1 routes, and the Rust proxy
boots with the resulting manifest. yah/ui does **not** import
mesofact at all (no `@mesofact/*` SDK leaking into the frontend
tree); the runtime adapters appear only inside server entrypoints,
which the build tree-shakes out of client bundles.

For Mode 3, the SPA shell mesofact emits is the JS bundle yah/ui
already builds — once hydrated, the SPA talks to whatever API it
wants (yubaba-managed service, third-party, …) and **mesofact is out
of the request path**. Mesofact does not proxy SPA → API traffic.

This is why the MVP DoD requires render-contract types to be
consumable by an unrelated TS project: it proves the seam is real
and mesofact is not quietly becoming a framework.

## IPC protocol (Rust ↔ Bun)

One Unix-domain socket per worker. Messages are NDJSON framed; the
*protocol* is fixed even though the framing may later move to a
length-prefixed binary format if profiling shows JSON cost > 10% of
render time.

```jsonc
// proxy → worker
{ "id": 42, "kind": "render", "route": "/p/:id",
  "req": { /* RenderRequest */ }, "deadline_ms": 2000 }

// worker → proxy
{ "id": 42, "kind": "ok", "html": "…", "headers": {…},
  "cache": { "ttl": 60, "tags": [...] } }
{ "id": 42, "kind": "err", "error": { "code": "source_timeout",
  "source": "project_db", "retryable": true } }

// lifecycle (id=0 reserved)
{ "id": 0, "kind": "ready", "manifest_version": "1", "build_id": "…" }
{ "id": 0, "kind": "ping" }   // proxy → worker, every 30s
{ "id": 0, "kind": "pong" }   // worker → proxy, must arrive within 5s
{ "id": 0, "kind": "drain" }  // proxy → worker, finish in-flight then exit
```

**Worker lifecycle**: spawned at proxy boot, N workers (default = CPU
count / 2). Each worker holds one long-lived Bun process. On SIGHUP
the proxy spawns a parallel set of new workers loading the new
manifest, drains the old set, then SIGTERMs them. Crash detection
fails any in-flight requests on that worker (503 or stale-on-error)
and respawns.

**Concurrency per worker**: the proxy multiplexes M concurrent
renders over one socket per worker. M is per-route from manifest
(default 4); routes hitting per-project DBs can lower it to 1 to keep DB
connection counts bounded. Backpressure: bounded queue (default 64
per worker); overflow returns 503.

## Manifest schema

The manifest is the single document the build emits and the proxy
boots from. It is versioned independently of the mesofact binary.

```jsonc
{
  "version": "1",                              // major bumps force restart
  "build_id": "2026-05-14T10:30:00Z-abc1234",  // cache-bust + rolling deploy
  "routes": [
    {
      "route": "/p/:id",                       // path-to-regexp
      "mode": "static" | "ssr" | "spa",
      "render_entrypoint": "dist/server/p_id.js",
      "requires": ["user", "project"],         // proxy-resolved fields
      "source_reads": ["project_db"],          // build-validation + cache-key
      "cache_policy": {
        "ttl": 60,                             // all fields in seconds
        "swr": 300,                            // stale-while-revalidate
        "negative_ttl": 10,                    // 404/4xx cache
        "vary": ["accept-language"]            // extra cache-key inputs
      },
      "concurrency": 4,                        // per-worker render concurrency
      "hydration": {                           // Mode 3 only
        "script": "p_id.HASH.js",              // relative to /{build_id}/hydrate/
        "code_split": ["p_id.HASH.a.js", "…"]  // same base
      },
      "prerender": {                           // Mode 1 only — see below
        "from": "sqlite:project_db",
        "query": "SELECT id FROM projects WHERE published",
        "param": "id"
      }
    }
  ],
  "static_assets": [
    // key is relative to /{build_id}/assets/
    { "key": "css/app.HASH.css", "content_hash": "sha256-…",
      "content_type": "text/css", "immutable": true }
  ],
  "error_routes": {
    "404": "dist/server/_404.js",
    "5xx": "dist/server/_5xx.js"
  }
}
```

`prerender` for parametric Mode 1 routes is either a literal list of
param maps or a source-derived query the publisher runs at build
time. Non-parametric Mode 1 routes omit it.

## Adapter API surface

All adapters expose the same shape; specifics vary by backend. Read-
only by design — mesofact has no write API. Writes go through yubaba-
managed services or the FE's own client.

```ts
interface Source {
  // tracked reads (contribute to cache tags)
  get<T>(table: string, id: string): Promise<T | null>      // sqlite/pg
  query<T>(sql: string, params?: unknown[]): Promise<T[]>   // sqlite/pg
  fetch(key: string): Promise<Uint8Array | null>            // r2
  list(prefix: string, opts?: ListOpts): Promise<R2Object[]>// r2

  // single-call opt-out from read tracking
  noTrack(): this

  // per-call timeout override (ms)
  timeout(ms: number): this
}
```

Defaults: sqlite 100ms timeout, pg 500ms, r2 2000ms. Errors are
typed: `SourceUnavailableError`, `SourceTimeoutError`,
`SourceQueryError`, `RowNotFoundError`. Render functions can catch
and return fallback HTML — the contract types don't require it.

No transactions. A render that needs multiple reads in a consistent
snapshot does one composite query. This keeps the adapter small and
keeps render paths from accidentally holding connections open.

## Auth & session contract

Session resolution happens in the Rust proxy via a pluggable
`SessionResolver`:

```rust
trait SessionResolver: Send + Sync {
    async fn resolve(
        &self,
        headers: &HeaderMap,
        cookies: &CookieJar,
    ) -> Result<Option<User>, SessionError>;
}
```

MVP ships one impl: `CookieSessionResolver` reads a configurable
cookie name (default `mesofact_session`) and verifies the token with a
`cheers_core::Codec` (the shared auth contract; default `PasetoV4Codec`
— PASETO v4.local, encrypted + authenticated — keyed by a secret yubaba
injects). The verified `cheers_core::Claims` (`{sub, device, binding,
issued_at, expires_at}`) map onto mesofact's `{id, attrs}` render shape:
`sub` becomes `id`; the device binding + lifetimes ride through `attrs`.
JWT/OAuth resolvers — and the asymmetric edge verifier (cheers R019) —
slot in behind the same trait by injecting a different codec.

```ts
type User = { id: string; attrs: Record<string, unknown> }
```

`attrs` is opaque to mesofact — the resolver populates whatever the
app needs. For `requires: ["user"]` routes:

- Session resolves → populate `req.user`, invoke render.
- Session missing/expired → 302 to a configurable login URL with
  `?next=<original-url>`. Configurable per-route in manifest.

User identity contributes to the cache key implicitly when
`requires: ["user"]` is set (no need to set Vary on cookie).

**Cross-instance identity**: multiple mesofact instances under one
apex domain (e.g. `yah.com` + `camp.yah.dev` under `.yah.dev`) get
unified sessions for free by sharing the cookie name, cookie domain,
and codec key (all yubaba-injected). No SSO protocol needed —
`CookieSessionResolver` runs identically in each instance and they
all decode the same cookie. This is the common shape when one org
runs several services on shared substrate; see
[mesofact-yah-case-study.md](./mesofact-yah-case-study.md) for the
worked example across three yah services.

## Cache-key composition

Mode 2 LRU key is `SHA-256(...)` of, in order:

1. `build_id` (invalidates whole cache on deploy)
2. `route_pattern` (not the resolved URL — the pattern)
3. resolved param map, JSON, keys sorted
4. query string, sorted
5. each `vary` header value, sorted by header name
6. `source_generations` — map of `source_name → generation`, for
   every source in `source_reads`. For `scope: "project"` sources
   the generation token comes from the project's `(home_region,
   generation)` record so a project migration invalidates without
   manual purge.
7. `user.id` or `"_anon"` — only if `requires ∋ user`

Source generations come from:

| Source | Generation token | Refresh |
|---|---|---|
| `sqlite` (global) | file mtime, or Litestream replica LSN | adapter exposes; cached 1s |
| `sqlite` (project) | externally-supplied project-generation token | cached 1s per project |
| `pg` | `pg_last_wal_replay_lsn()` | polled per source every 1s |
| `r2` | bucket `Last-Modified` (object) or per-key `Last-Modified` (manifest) | polled per source every 5s |
| `rpc` | generation field from a paired roster source (declared via `generation_from = "<source_name>"`) | inherits paired source's refresh cadence |

The proxy caches generations with a 1s TTL so cache misses don't
amplify into N source pings.

Generation tokens are not required to come from the same backend the
data lives on. A `scope: "project"` source can declare
`generation_from = "<roster_source>"` and the proxy will read the
token from that paired source instead of polling the data source.
This is how the substrate avoids requiring a global ACID database:
the roster can be an R2-stored signed manifest (or set of manifests,
one per entry) the deployment's
control plane writes to, and the cache-key composition stays
correct without any new infrastructure.

## Mode 2 caching beyond TTL

Cache states:

- **fresh** (`age < ttl`) — serve from cache.
- **stale** (`ttl ≤ age < ttl + swr`) — serve from cache, kick off
  async re-render. Next request after re-render sees fresh.
- **expired** (`age ≥ ttl + swr`) — synchronous miss.

Negative caching: non-2xx responses cached for `negative_ttl`
(default 10s). 5xx never cached.

Vary: `cache_policy.vary` adds explicit header values to the key.
`requires: ["user"]` adds user-id implicitly; nothing else from
cookies enters the key by default.

On-error fallback: render throws during a stale-window request →
serve the stale entry + `X-Mesofact-Stale: true`. Render throws with
no stale entry → 503 + `Retry-After: <ttl>`.

## Failure modes

| Failure | Mode 1 | Mode 2 | Mode 3 |
|---|---|---|---|
| Bun worker crash mid-render | Publisher retries (max 3); build fails the route after | Fail in-flight 503; respawn; serve stale if available | n/a (shell already built) |
| Adapter timeout | Same as crash | Render gets `SourceTimeoutError`; fallback HTML or rethrow → stale/503 | n/a |
| Adapter unavailable | Same | Same | n/a |
| User/project resolution fails | n/a (scoped sources forbidden in Mode 1) | Proxy 503 + Retry-After; render never invoked | n/a |
| Scoped source generation bump mid-request | n/a | Generation bump detected; 409 + new generation header; client retries | n/a |
| Malformed manifest on SIGHUP | Old manifest stays live; alert; no traffic disruption | Same | Same |
| Worker doesn't ack ping (5s) | Kill + respawn; in-flight fail | Same | n/a |
| R2 publish failure | Old build_id stays live; alert | Same | Same |

## Multi-tenancy posture

**One mesofact instance is single-tenant.** One manifest, one
adapter-config set, one R2 publish target, one CDN tag namespace.
Multi-tenant deployments run multiple mesofact instances, one per
tenant, fronted by Cloudflare DNS/Tunnel rules.

"Single-tenant per instance" is *per service*, not *per company* —
one org commonly operates several instances on shared substrate
(e.g. `yah.dev` marketing, `yah.com` platform, `camp.yah.dev`
camps are three mesofact instances run by one team; see
[mesofact-yah-case-study.md](./mesofact-yah-case-study.md)). The
sharing happens at the yubaba/cookie-domain layer, not inside
mesofact.

Reasons this is the MVP posture, not per-request tenant resolution:

- Adapter config (esp. scoped SQLite paths, credentials) is tenant-
  specific. Reloading per-request adds complexity for marginal gain.
- Cache-invalidation tag namespaces scope naturally per-tenant
  when each tenant has its own instance.
- Yubaba already places per-tenant workloads via raft; mesofact rides
  that placement instead of duplicating tenant routing.

Per-request tenant resolution can be added later by replacing the
single adapter-config block with a `tenant_id → config` map and
extending `RenderRequest` with `tenant`. The contract types are
forward-compatible; the runtime isn't yet.

Mode 1 publish targets (R2 bucket + CDN tag namespace) **must** be
tenant-scoped — cross-tenant cache poisoning is the failure to
prevent.

## Bundle splitting & hydration boundary (Mode 3)

Mode 3 entrypoints export *both* a server-side render function and a
client-side hydration entry. The build emits two bundle trees (Vite's
default ssr/client split). The render result names the client bundle
that hydrates this page:

```ts
export async function render(req: RenderRequest): Promise<RenderResult> {
  const initial = await fetchInitialState(req)
  return {
    html: renderToString(<App initial={initial} />),
    headers: { 'content-type': 'text/html' },
    cache: { ttl: 0 },
    hydration: {
      script: 'p_id.HASH.js',  // relative to /{build_id}/hydrate/
      initial_state: initial,  // serialized into <script id="__MESOFACT_STATE__">
    },
  }
}
```

Client side reads `__MESOFACT_STATE__`, calls `hydrateRoot()`. This
is a 6-line pattern documented in the contract types; mesofact does
not ship a runtime helper.

Code-splitting is whatever Vite emits — the build manifest lists the
code-split chunks per entry in `hydration.code_split`, and mesofact
serves them through R2 + CDN with immutable cache headers.

After hydration, Mode 3 traffic does not return to mesofact. The SPA
talks to whatever API it picked, on whatever protocol it picked, with
whatever client it picked. That boundary is the entire point.

## Components (MVP)

1. **Manifest format** — versioned JSON emitted by the build:
   per-route entries (mode, entrypoint, `requires`, `source_reads`,
   `cache_policy`, `concurrency`, optional `hydration` for Mode 3,
   optional `prerender` for Mode 1) plus a `static_assets` table and
   `error_routes`. Rust reads at boot; rebuild = SIGHUP. See
   §"Manifest schema" for the full shape.
2. **Rust proxy** (`axum`) — route table from manifest, in-memory LRU
   response cache (key composition in §"Cache-key composition"), mode
   dispatch (static → 302/proxy to CDN or local fallback; SSR → Bun
   pool; SPA → return shell). Owns user/project/region resolution
   and session handling.
3. **Bun render pool** — N long-lived Bun workers, one Unix-domain
   socket per worker, NDJSON-framed envelope (full protocol in
   §"IPC protocol"). No HTTP between Rust and Bun.
4. **Publisher** — `mesofact publish` (or `yah web publish` when
   consumed by yah): runs the build, walks Mode 1 routes, calls
   `render()` for each, uploads HTML to R2 with cache-bust headers,
   purges CDN tags. The same publisher pipeline can emit signed
   non-HTML artifacts (e.g. registry manifests) as a second artifact
   kind — same R2 + tag-purge contract.
5. **Dev server** — single `bun run dev` that serves all three modes
   locally without the Rust proxy in the loop (proxy is prod-only;
   dev hits Bun directly).

## Render axis × source axis

mesofact's substrate cares about two source properties, both
declared in adapter config (not inferred per-render):

- **`scope`** — `"global"` (no per-request key — everyone reads the
  same value) or `"project"` / `"user"` (templated on
  `req.project.id` / `req.user.id`, value differs per request).
- **`generation`** — opaque token the adapter exposes when its
  underlying state changes (file mtime, replica LSN, bucket
  `Last-Modified`, externally-supplied project-generation token).
  Feeds the cache key.

Render × source compatibility:

| | **global, immutable** | **global, mutable** | **scoped (project/user)** |
|---|---|---|---|
| **Mode 1** static | ideal (long CDN TTL) | tag-driven re-render on backend change | **forbidden** — build can't enumerate keys |
| **Mode 2** SSR | fine (short TTL) | TTL + SWR; tag invalidation optional | fine; generation in cache key |
| **Mode 3** SPA shell | shell rarely reads sources | same | usually no — client fetches after hydrate |

The Mode 1 + scoped prohibition is enforced at build time: a route's
`source_reads` cannot include any source whose `scope` is not
`global`. That replaces ad-hoc per-application tier rules with one
substrate-level rule.

Cross-region routing for scoped sources is the load-bearing case.
For MVP the Bun pool is single-region; cross-region `scope: "project"`
reads go over RPC inside the adapter (data follows Bun — slow but
correct). The seam for multi-region pools is already there: the
proxy knows each source's scope and could route to the home-region
pool once it exists.

**Caveat for single-host scoped data**: when the data has no regional
replica — it lives on exactly one specific node, e.g. a yah camp's
`yah-camp` daemon — both options collapse. "Bun follows data" still
requires RPC to the one node; "data follows Bun" can't happen because
there's nothing to follow. The `rpc` adapter is the right shape there,
and the cross-region cost is paid every read regardless of pool
placement. Multi-region pools help latency for nearby data only.

Per-application mappings live in sibling case studies:

- [mesofact-noisetable-case-study.md](./mesofact-noisetable-case-study.md) — noisetable's T0–T4 tier model.
- [mesofact-yah-case-study.md](./mesofact-yah-case-study.md) — yah's three services (marketing, platform, remote-camp) on one substrate; the `rpc`-adapter + R2-as-roster shape lives here.

## First dogfood: yah.dev marketing page

yah consumes this crate to build [yah.dev](https://yah.dev) (or
wherever the marketing page lives). Everything Mode 1, push-to-R2,
no scoped-source dependencies — minimum useful exercise of the
publisher + Rust proxy fallback path.

Second target (not MVP): camp.yah.dev as a Mode 3 SPA sharing the
same build pipeline + design tokens.

## Build pipeline

Seven phases, all run by `mesofact build`:

1. **TS build** — Bun + Vite bundles every `render_entrypoint` to
   ESM (server tree). For Mode 3 routes, a separate client tree is
   emitted with code-split chunks. Outputs `dist/server/`,
   `dist/client/`, plus Vite's `manifest.json`.
2. **Route discovery** — reads a single `mesofact.routes.ts` config
   file. Not file-based routing; the route table is an explicit data
   structure. Each entry maps a pattern → entrypoint + mode +
   `requires` + `cache_policy`.
3. **Source inference** — static analysis of each server
   entrypoint's adapter imports populates `source_reads`. A
   `// @mesofact-sources project_db, assets` comment can override
   when static analysis is wrong (third-party module re-exporting an
   adapter, etc.).
4. **Build-time validation** — rejects any Mode 1 route whose
   `source_reads` includes a non-`global` source, or whose
   `requires` includes `user`. Logs the offending route + the import
   chain that introduced the forbidden source.
5. **Mode 1 prerender** — for each Mode 1 route, expand its
   `prerender` set (literal list or source-derived query), invoke
   render per param map. Writes HTML to a staging dir keyed by
   resolved URL. Read-set tags collected via `AsyncLocalStorage` and
   stored in `tag-index.json`.
6. **Manifest emission** — writes `manifest.json` with the route
   table, static-asset table, build_id, and version.
7. **Publish** (separate step, `mesofact publish`) — uploads HTML +
   static assets + JS bundles to R2 under
   `/{build_id}/`, then atomically swaps `/manifest.json` to point
   at the new build. CDN tags purged in the same step.

Dev mode (`mesofact dev`): runs Bun directly with Vite HMR; proxy is
out of the loop; phases 5 and 7 are skipped. Adapter calls hit the
same yubaba-configured sources as prod (or pointed at fixtures via
config).

## Publisher tag-subscription

Where the listener runs: `mesofact-publisher` is a long-lived daemon
co-located with the proxy (same yubaba-managed unit). It maintains:

- One Postgres `LISTEN` connection per `pg` source declared in any
  route's `source_reads`.
- An HTTP endpoint receiving R2 event notifications.
- A SQLite WAL tail / Litestream replication-event subscription per
  `sqlite` source.

Tag index: at manifest load, the publisher builds a reverse index
from `tag_prefix → routes_that_emitted_it_at_last_publish`. Held in
memory; persisted to R2 as `tag-index.json` next to the manifest for
restart recovery. On a tag event, the publisher looks up matching
routes (longest-prefix match) and enqueues them for re-render on a
bounded worker pool (default 4 concurrent).

Re-render output replaces the route's HTML at the same R2 key and
the publisher issues a CDN purge for that tag.

Fault recovery:

- **Publisher down**: PG `LISTEN` channels buffer in WAL; R2 event
  notifications retain at-least-once for 24h. On restart, publisher
  resubscribes; missed events within retention replay normally.
- **Beyond retention**: `mesofact-publisher reconcile` re-renders
  every Mode 1 route. Slow, last-resort, manual.
- **Render failure during re-render**: route stays at previous
  version; alert; next event retries.

## Static asset handling

CSS, fonts, images, client JS bundles are emitted by the Bun build
with content hashes (Vite default). The build's manifest enumerates
them in `static_assets`; `mesofact publish` uploads them to R2
alongside HTML.

Path layout under R2:

- `/{build_id}/assets/{name}.{hash}.{ext}` — assets, keyed by
  `build_id` so prior deploys keep working until pruned.
- `/{build_id}/html/{route_key}.html` — Mode 1 HTML.
- `/{build_id}/hydrate/{name}.{hash}.js` — Mode 3 client bundles.
- `/manifest.json` at root — single pointer to active `build_id`.
- `/tag-index.json` at root — for publisher restart.

CDN config (Cloudflare, set per path-prefix):

| Path | Cache | Purge |
|---|---|---|
| `/{build_id}/assets/*` | 1y immutable | never (content-hashed) |
| `/{build_id}/hydrate/*` | 1y immutable | never |
| `/{build_id}/html/*` | tag-keyed, long TTL | by tag on event |
| `/manifest.json` | no-cache | on every publish |
| `/tag-index.json` | no-cache | on every publish |

Mode 2 and Mode 3 responses reference asset URLs via the active
`build_id` so a deploy can't half-swap (proxy on new manifest,
clients on old assets).

## Versioning & rolling deploy

A deploy is atomic at the `manifest.json` pointer in R2. The proxy
fetches the pointer on SIGHUP and on a 30s heartbeat poll.

Rolling sequence:

1. Publisher uploads new HTML + assets to `/{new_build_id}/`.
2. Publisher uploads `manifest.json` with `build_id = new_build_id`.
3. Yubaba SIGHUPs proxy (or proxy picks up via heartbeat).
4. Proxy spawns N new Bun workers loading new server bundles.
5. New traffic routes to new workers; old workers receive `drain`,
   finish in-flight requests, exit.
6. Old `{build_id}/` tree persists ≥24h; `mesofact prune` GC's it
   after retention.

Render-contract `version` mismatch: bumping `version` major forces a
hard restart (no graceful drain). Minor bumps are backwards-
compatible; the proxy refuses to load a manifest whose major doesn't
match the binary.

Roll-back: `mesofact publish --pin {build_id}` rewrites
`manifest.json` to point at an older still-retained build. CDN purges
the `html/*` paths to evict cached new-build HTML.

## Observability

Tracing:

- The proxy generates a W3C `traceparent` per request (or accepts an
  inbound one).
- Trace context passes to the Bun worker as `req.ctx.trace`.
- Adapters emit a child span per backend call (table or key in span
  name; full SQL/args not recorded, to avoid leaking scoped data).
- Spans exported via OTLP to a yubaba-managed collector. No mesofact-
  specific tracing UI.

Metrics (Prometheus exposition at `/metrics`, scraped by yubaba):

| Metric | Labels |
|---|---|
| `mesofact_requests_total` | `route, mode, status` |
| `mesofact_render_duration_seconds` | `route` (histogram) |
| `mesofact_cache_total` | `route, state` (state ∈ fresh/stale/miss) |
| `mesofact_worker_pool` | `state` (ready/busy/restarting) (gauge) |
| `mesofact_publisher_lag_seconds` | `source` (gauge) |
| `mesofact_adapter_errors_total` | `adapter, source, kind` |
| `mesofact_publisher_rerenders_total` | `route, reason` |

Logs: structured JSON. One line per request at proxy (URL, status,
duration, cache state, user-id or `_anon`, trace ID). One line per
render error at Bun (route, error kind, stack head). No request
bodies, no response bodies, no full SQL.

## Crate layout (proposed)

Mirrors `external/xlb/`:

```
yah/external/mesofact/
├── Cargo.toml            # workspace
├── crates/
│   ├── mesofact/           # the Rust proxy library + binary
│   └── mesofact-publisher/ # the publisher (R2 upload, CDN purge)
├── packages/
│   └── mesofact-runtime/   # published as @mesofact/runtime — adapters,
│                           # render contract types, and Bun render-pool worker
└── examples/
    └── yah-dev/          # the marketing page, drives MVP
```

yah consumes via:

- `yah` CLI gains a `web` subcommand wrapping the publisher
- desktop bundles the proxy binary if we want a local-preview mode
  (defer)

## Open questions

Genuinely-deferred items the refining team should ticket as
"resolve before milestone X":

- **High-write-rate dashboards in Mode 2 vs. Mode 3** (UX question,
  not substrate). Mode 2 with 5s TTL vs. Mode 3 with WS streaming.
  Defer until the first such dashboard ships.
- **Session resolver impls beyond `CookieSessionResolver`** — JWT and
  OAuth resolvers are out of MVP scope but the trait is stable.
- **Render-time access to resolution data beyond `user`/`project`/`region`** —
  if a render needs more proxy-resolved context, the seam is "Rust
  passes resolved values on `req`," not "Bun calls back into Rust."
- **Litestream vs. LiteFS for the `sqlite` adapter's replicated
  shape** — replica/LSN semantics are subtly different. Pick during
  the first scoped-source SSR dogfood.
- **Tag-index restart-recovery durability** — `tag-index.json` in R2
  is at-most-once across rapid restarts. If a publisher restart loses
  a few seconds of registered tags, the next event won't trigger a
  re-render. Worst case is mitigated by the 24h reconcile, but a
  durable queue would close the gap. Probably overkill for MVP.

## Decisions (closed)

### Data plane

- **Data-entry shape**: typed TS adapters (`r2`, `sqlite`, `pg`,
  `rpc`), yubaba-injected config + credentials. Not a GraphQL
  bridge. The render function imports adapters directly. See
  §"Data-source seam". The `rpc` adapter covers scoped data that
  lives on a single specific host; its generation token comes from
  a paired roster source rather than the host itself, which lets
  the substrate stay correct without a global ACID database.
- **Adapter API surface**: read-only, no transactions, typed errors,
  per-call timeout override, `noTrack()` opt-out from read tracking.
  See §"Adapter API surface".
- **Resolution owner**: Rust proxy owns `user` / `project` /
  `region` lookups before invoking render.
- **Props/data fetching split**: render fetches data via injected
  adapters; Rust pre-resolves `user` / `project` / `region` context
  and hands it on `RenderRequest`.
- **Mode 1 tag inheritance**: adapters track reads via
  `AsyncLocalStorage` and emit `<kind>:<source>:<table>:<id>` tags
  the publisher subscribes to (PG `LISTEN/NOTIFY`, R2 events, SQLite
  WAL tail). Default is tracked; `noTrack()` is opt-in.

### Contract & shape

- **`RenderRequest` shape**: explicit `user` / `project` / `region`
  optionals + opaque `ctx` escape hatch. Routes declare `requires`
  in the manifest; the proxy refuses to invoke render when a
  required field doesn't resolve. See §"Request context".
- **Manifest schema**: versioned doc with route table + static-asset
  table + error routes + `build_id`. See §"Manifest schema".
- **Cache-key composition**: 7-input SHA-256, including `build_id`,
  route pattern, params, query, vary, per-source generations
  (scoped sources fold in their project/user-keyed generation token),
  user id. See §"Cache-key composition".
- **Auth contract**: pluggable `SessionResolver`; MVP ships
  `CookieSessionResolver`. See §"Auth & session contract".
- **Bundle splitting / hydration boundary (Mode 3)**: server entry
  returns `hydration.script` + `initial_state`; client reads
  `__MESOFACT_STATE__` and calls `hydrateRoot()`. No runtime helper
  shipped.

### Runtime & deployment

- **FE projects don't import mesofact**: yah/ui-style projects export
  a `render(req)` entrypoint; mesofact's build wraps it. No
  `@mesofact/*` SDK in client bundles.
- **SPA mode is shell + bundle only**: after hydration, mesofact is
  out of the SPA's request path.
- **IPC protocol**: one Unix-domain socket per worker; NDJSON
  envelope with a fixed message shape; lifecycle via id=0 messages.
  Framing is replaceable, the *protocol* is fixed.
- **Mode 2 caching**: TTL + SWR + negative TTL + Vary. On-error
  fallback serves stale within SWR window; 503 + Retry-After
  otherwise.
- **Multi-tenancy**: single-tenant per instance for MVP. Per-request
  tenant resolution is a forward-compatible extension; not in scope.
- **Failure modes**: per-mode/per-failure matrix specced. See
  §"Failure modes".
- **Build pipeline**: 7 phases, all driven by `mesofact build`. Not
  file-based routing; `mesofact.routes.ts` is the explicit route
  table. Build-time validation enforces "no Mode 1 route reads a
  non-`global` source and no Mode 1 route requires `user`."
- **Publisher tag-subscription**: long-lived daemon co-located with
  proxy; PG LISTEN + R2 events + SQLite WAL tail; reverse-index
  persisted to R2 for restart.
- **Static asset layout**: `/{build_id}/{assets,html,hydrate}/...`
  paths in R2; `manifest.json` at root as the atomic pointer.
- **Versioning & rolling deploy**: atomic at the `manifest.json`
  pointer; SIGHUP + 30s heartbeat; new workers spawned then old
  drained; old build retained ≥24h.
- **Observability**: W3C `traceparent`, OTLP export, Prometheus
  `/metrics`, structured JSON logs. No mesofact-specific UI; yubaba's
  observability stack consumes everything.

## MVP definition of done

- One route of each mode in production at yah.dev (or staging).
- Mode 1 serves from R2/CDN with correct cache headers under `curl -I`.
- Mode 2 renders through Bun pool with sub-50 ms p50 on miss,
  sub-5 ms on hit.
- Mode 3 boots the SPA, fetches an API, renders.
- `mesofact publish` is one command and idempotent.
- Render contract types published from `packages/mesofact-runtime`
  (as `@mesofact/runtime`) and consumable by an unrelated TS project
  (proves the seam is real).
