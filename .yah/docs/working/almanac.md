# Almanac

A pattern for **periodically-refreshed reference data about the external world**, sitting in the gray area between mesofact's static (Mode 1) and SSR (Mode 2) routes.

An almanac is *not* a route, a service, or a workload — it is the **versioned normalized artifact** produced by a periodic ingest job and consumed by one or more surfaces (webview routes, RPC clients, other almanacs).

> First case study: an OpenRouter pricing puller. Source notes in [`visiting/reference/openrouter-pricing-puller.md`](../../../../visiting/reference/openrouter-pricing-puller.md) (yah root).

---

## 1. What an almanac is

Every almanac has:

1. **An external source** we don't own (HTTP API, scraped page, feed).
2. **A cadence** independent of human edits — needs cron, jitter, backoff.
3. **A normalization step** — raw schema → opinionated schema we control. This is the curation value.
4. **A versioned artifact** — generation tokens; deprecation grace windows; last-good-cache served when the source is down.
5. **Zero or more consumers** — webview routes, RPC clients, other almanacs joining at render time.

What an almanac is *not*:

- Not user-generated content (forms, comments, sessions).
- Not live operational data (current request, presence, ticks). That's a different surface.
- Not the workload that produces it. The workload is owned by yubaba.
- Not coupled to a particular transport. The same artifact feeds webview and RPC.

---

## 2. Why this pattern earns a name

Mesofact's existing design covers two extremes — push-to-CDN static and on-request SSR — plus a publisher tag-subscription mechanism that re-renders Mode 1 routes when their roster source changes ([mesofact.md §"Publisher tag-subscription"](mesofact.md), line 834–863).

The almanac pattern formalizes the **producer side** of that subscription:

- The thing that fetches OpenRouter / quality-scores / changelog deltas / quota snapshots.
- The thing that decides "this is the schema downstream depends on."
- The thing that lives in mesofact's directory tree so its consumers are discoverable.

Without naming it, we'd end up with N one-off scrapers each inventing schema, scheduling, and failure-handling. With it, the pattern is one directory, one spec, one stable contract.

---

## 3. Anatomy

```
sites/<site>/almanacs/<almanac-name>/
  almanac.toml      # source, schedule, retry, output key — yubaba reads
  normalize.<lang>  # raw fetch → normalized artifact (the curation step)
  schema/           # the stable contract — ALL consumers depend on this
    types.rs        #   canonical Rust shape (when consumers cross language)
    types.ts        #   generated / mirrored TS bindings, if a webview consumer needs them
```

Consumers live **outside** the almanac directory and depend on `schema/`:

```
sites/<site>/routes.ts             # webview surface declares `requires: almanac:<name>`
apps/desktop/src/almanacs/<name>.rs  # typed RPC client
crates/<other>/src/...             # CLI or yubaba-internal consumer
```

The almanac directory owns *production*. Consumption is wherever the consumer lives.

---

## 4. The four seams worth being deliberate about

### 4.1 The artifact is the contract, not the route

Routes are one of N consumers. A webview HTML page, a JSON asset, a Tauri RPC call, and a future thick-client subscriber all depend on the **normalized schema**, never on each other.

This rules out designs where "the almanac is a route that happens to be cached" — the route surface is downstream of the artifact.

### 4.2 Schema language: canonical Rust when consumers cross the language boundary

For any almanac whose consumers include desktop, yubaba, CLI, or another camp, the canonical schema lives in **Rust** (a tiny crate or `schema/types.rs`). TypeScript bindings are generated or hand-mirrored with a typecheck guard.

Webview-only almanacs (a marketing-page-scoped almanac with no native consumer) may stay TS-native. The choice is per-almanac, declared in `almanac.toml`:

```toml
[schema]
canonical = "rust"   # or "ts"
```

This matches yah's broader convention: cross-process types live in Rust crates (cf. `crates/yah/kg-anno`).

### 4.3 Yubaba↔mesofact handshake is one-directional

Yubaba owns scheduling, retries, cron, secrets, observability. Mesofact owns the artifact's schema, the consumer contract, and the publisher invalidation event.

**Direction: mesofact's `almanac.toml` is the source of truth; yubaba tooling reads it and registers a workload.** Not the reverse. Bidirectional sync rots fast.

The CLI seam (sketch):

```
yah yubaba almanacs sync <site>   # scans sites/<site>/almanacs/, registers/updates workloads
yah yubaba almanacs run <name>    # one-shot, for local dev or manual refresh
```

For local dev, `bun run dev` on a mesofact site invokes the same one-shot path so a developer sees fresh data without waiting for prod cron.

### 4.4 Storage location is opaque to consumers

Today the artifact lives at an R2 key. Tomorrow it might be sqlite, or a per-camp store. Consumers ask:

```rust
let pricing = almanac::get::<OpenrouterPricing>("openrouter-pricing").await?;
```

Not:

```rust
let raw = r2::get("almanacs/openrouter-pricing/latest.json").await?;
```

The client library — almanac-client — handles transport, caching, ETag, last-good-fallback. See §6.

---

## 5. Failure semantics (non-negotiable)

Every almanac must:

- Serve **last-good-cache** when the source is down. Never serve empty.
- Apply a **deprecation grace window** (24h default) before marking entities removed. Single-fetch absence is not deletion.
- Surface **freshness metadata** in the artifact itself (`generated_at`, `source_fetched_at`, `is_stale`). Consumers decide how to display.
- Fail closed on **schema drift** — if `normalize.<lang>` can't shape the response, the old artifact stays live and the failure is alerted, not propagated.

`almanac.toml` declares the policy knobs:

```toml
[freshness]
target_age = "1h"
stale_after = "6h"
deprecation_grace = "24h"
```

---

## 6. Consumer transports

An artifact reaches consumers two ways:

### 6.1 Webview (HTTP)

Mesofact route declares dependency:

```ts
// sites/yah-dev/routes.ts
{
  pattern: "/pricing/openrouter",
  mode: "static",
  requires: [{ almanac: "openrouter-pricing" }],
  entry: "./pages/pricing/openrouter.tsx",
}
```

Publisher re-renders when the almanac's artifact version changes. Browser fetches the rendered HTML or a `pricing.json` static asset — same artifact, different surfaces.

### 6.2 RPC (thick client, including Tauri)

**Canonical client path: through the local `yah-camp` daemon, not direct to the origin.** Rationale:

- `yah-camp` already owns local cache and offline-fallback.
- One place to swap transports later (HTTP today, iroh later, etc.).
- Consumers (Tauri, CLI, other camps) all share the same cache and freshness policy.

```rust
// apps/desktop or any consumer
let pricing = camp_client.almanac::<OpenrouterPricing>("openrouter-pricing").await?;
```

A consumer *may* fetch directly in degenerate cases (test fixtures, headless CI) but the daemon path is the default.

> **Open question (§9)**: whether `almanac-client` is a `yah-camp` library that everything else calls through, or each consumer fetches directly. Pricing is small enough that either works; the answer determines whether almanac becomes a real subsystem or stays a convention.

---

## 7. Derived artifacts (opt-in)

Some almanacs naturally produce more than just "the current snapshot":

- A **diff event stream** (pricing change webhooks: `model.added`, `pricing_changed`).
- A **changes feed** (RSS/Atom of human-readable updates).
- A **historical archive** (snapshots indexed by generation).

These are **opt-in derived artifacts**, not part of the core almanac contract. They live as additional output keys declared in `almanac.toml`:

```toml
[derive.diff_events]
emit = "webhook"
schema = "schema/events.rs"

[derive.history]
retain = "90d"
```

Most almanacs ship with only the latest snapshot. Deriving diffs is a deliberate add when a consumer needs them.

---

## 8. Case study: openrouter-pricing

What the OpenRouter source teaches the pattern:

| Source quirk | Pattern lesson | Where it lives |
|---|---|---|
| Pricing as decimal strings | Schema-layer concern; never propagate as `f64` | `schema/types.rs` |
| Tiered pricing (Gemini 128K+) | Some artifacts have per-tier shapes | `normalize.ts` and `schema/types.rs`; no pattern impact |
| `internal_reasoning` billed separately | Source-specific field; not generic | `schema/types.rs` |
| Quality scores from a second source | **This is a second almanac, joined at render** | New `theozard-quality` almanac; route reads both |
| Deprecation grace window (§9 of source doc) | **Generic — promoted to pattern (§5)** | `almanac.toml` `freshness.deprecation_grace` |
| Diff events (`model.added`, etc.) | Opt-in derived artifact, not core | `derive.diff_events` in `almanac.toml` |
| Last-good cache when API is down | **Generic — promoted to pattern (§5)** | almanac-client |
| Source rate limits | Workload concern | yubaba job spec |
| ETag / `models/count` for freshness check | Workload optimization | yubaba job spec |

What the pricing case *doesn't* tell us, and we shouldn't invent on its behalf:

- Whether almanacs ever **chain** (almanac A's output is almanac B's input). Plausible (joins, derived rollups) but unresolved.
- Whether almanacs are **per-site or per-camp scoped**. Pricing is site-scoped (yah.dev). Quota snapshots are per-camp. The directory layout in §3 assumes site-scoped; per-camp may want a different home.

These wait for a second concrete case.

---

## 9. Open questions before this graduates to `architecture/`

1. **Publisher invalidation on external writes.** Does mesofact's publisher tag-subscription fire on R2 writes that originate from a yubaba workload (not the publisher itself)? If not, that gap is the actual first ticket — it gates every future yubaba→mesofact case, not just almanacs.

2. **Canonical client path.** Is `almanac-client` a `yah-camp` library (§6.2) or a freestanding crate consumers compose? Decision determines whether almanac becomes a real subsystem with cross-process state or stays a thin convention.

3. **Schema codegen.** When the canonical schema is Rust and a TS consumer needs it: hand-mirror with a typecheck guard, or generate (ts-rs, schemars→json-schema→quicktype)? Hand-mirror is fine for one almanac; the cliff is at three.

4. **Per-camp vs per-site scope.** §3 assumes `sites/<site>/almanacs/`. A per-camp quota almanac wants `.yah/almanacs/` or similar. Resolve when the second case lands.

5. **"Almanac" vocabulary stability.** Reads well; doesn't collide. Confirm by reading aloud in three contexts: route config, RPC call, error message. ("Almanac `openrouter-pricing` is stale" — passes.)

---

## 10. Validation: stereo-design against a second case

The cheapest test that the pattern isn't overfit: pick a second concrete almanac and design it against the same four-file shape. Candidates ranked by yah-roadmap proximity:

- **theozard-quality** — quality scores; joins to pricing at render time. Tests the multi-almanac-per-route case.
- **anthropic-changelog** — model availability deltas; no obvious second consumer. Tests the "webview-only" simplification.
- **camp-quota-snapshot** — per-camp quota ledger. Tests the per-camp scope question (§9.4).

If two cases want the same `almanac.toml` + `normalize.<lang>` + `schema/` shape with different content, the pattern is real and this doc graduates. If the second case wants a meaningfully different shape, this doc gets revised before graduation.
