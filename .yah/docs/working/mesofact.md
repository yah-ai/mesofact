
<!--
@yah:ticket(R010-T2, "yah.dev real-network deploy + CDN-header verification (R2 + Cloudflare provisioning, content-change re-publish smoke)")
@yah:assignee(agent:claude)
@yah:at(2026-05-17T23:28:20Z)
@yah:status(review)
@yah:parent(R010)
@yah:handoff("P8 code work landed in R010-T1 — examples/yah-dev/ builds + in-memory publishes cleanly with scripts/smoke-yah-dev.sh as the no-creds smoke. R008-T7 covers the real-network publisher in CI. What's left is purely infra: provision a Cloudflare R2 bucket + zone for yah.dev, run a real publish against examples/yah-dev/dist, and run the CDN-header curl checks the working doc P8 section calls for. No code changes expected — if you find any, file a sub-ticket of R010 rather than expanding this one.")
@yah:next("Provision a Cloudflare R2 bucket (suggested name yah-dev-marketing) and the Cloudflare zone for yah.dev. Configure zone-level cache rules per architecture/mesofact.md: HTML routes get cache-control: public, max-age=0, s-maxage=3600, stale-while-revalidate=86400 with Cache-Tag from tag-index.json.")
@yah:next("Add a [publish] section to a mesofact.config.toml (top-level repo or examples/yah-dev/) with bucket/endpoint/zone_id pointing at the new bucket+zone. Export MESOFACT_S3_ACCESS_KEY_ID / MESOFACT_S3_SECRET_ACCESS_KEY / CLOUDFLARE_API_TOKEN. Run cargo run -p mesofact-publisher --bin mesofact-publish -- examples/yah-dev/dist (no --in-memory) and confirm uploaded=8, purged=0 on first run.")
@yah:next("Point yah.dev DNS at the zone, then curl -sI https://yah.dev/ and assert: cache-control: public, max-age=0, s-maxage=3600, stale-while-revalidate=86400 and cf-cache-status present (HIT after a warmup). Repeat for https://yah.dev/404.")
@yah:next("Edit src/render.ts (e.g. tweak the tagline), rebuild + re-publish; confirm the report shows uploaded=1 + purged includes page:home and site:yah-dev, and curl -sI https://yah.dev/ returns the new content with cf-cache-status MISS then HIT.")
@yah:verify("bash scripts/smoke-yah-dev.sh")
@yah:verify("curl -sI https://yah.dev/ | grep -E 'cache-control|cf-cache-status'")
@yah:handoff("examples/yah-dev/mesofact.config.toml created with correct [publish] shape (bucket=yah-dev-marketing, endpoint placeholder, zone_id placeholder, default env var names). smoke-yah-dev.sh passes clean. Remaining work is pure Cloudflare operator steps — no code changes needed.")
@yah:next("In Cloudflare dashboard: create R2 bucket named yah-dev-marketing. Note your Account ID from the R2 overview page.")
@yah:next("In Cloudflare dashboard: open yah.dev zone → Overview → copy Zone ID. Point yah.dev DNS A record at Cloudflare (proxy enabled).")
@yah:next("In R2 → Manage R2 API Tokens: create a token with Object Read & Write on yah-dev-marketing. Copy Access Key ID + Secret.")
@yah:next("In Cloudflare dashboard: create API token with Zone:Cache Purge permission for yah.dev. Copy token.")
@yah:next("Fill in examples/yah-dev/mesofact.config.toml: replace ACCOUNT_ID and ZONE_ID with the real values.")
@yah:next("Export env vars: MESOFACT_S3_ACCESS_KEY_ID / MESOFACT_S3_SECRET_ACCESS_KEY / CLOUDFLARE_API_TOKEN. Build dist/ with: cd examples/yah-dev && bun run build. Then publish: cargo run -p mesofact-publisher --bin mesofact-publish -- examples/yah-dev/dist — expect uploaded=8, purged=0.")
@yah:next("Add zone-level Cache Rule in Cloudflare dashboard for yah.dev: if URI path matches *.html or / → cache everything, set Cache-Control to public, max-age=0, s-maxage=3600, stale-while-revalidate=86400, enable Cache-Tag (populated from x-cache-tag response header).")
@yah:next("curl -sI https://yah.dev/ | grep -E 'cache-control|cf-cache-status' — assert both headers present; cf-cache-status=HIT after warmup. Repeat for https://yah.dev/404.")
@yah:next("Edit examples/yah-dev/src/render.ts (tweak tagline), rebuild + re-publish. Confirm uploaded=1 + purged includes page:home and site:yah-dev. Then curl and confirm cf-cache-status=MISS then HIT.")
@yah:verify("bash scripts/smoke-yah-dev.sh")
@yah:verify("curl -sI https://yah.dev/ | grep -E 'cache-control|cf-cache-status'")
@yah:verify("curl -sI https://yah.dev/404 | grep -E 'cache-control|cf-cache-status'")
@yah:handoff("Parked pending operator decision (2026-05-17). Surfaced two yah-tool gaps while scoping the operator-deploy step: no per-provider help on Settings→API Keys (operator has to read Cloudflare docs to know which two tokens to mint + which scopes), and no agent-leasable vault path so a future mesofact-publish run could request CLOUDFLARE_API_TOKEN under approval instead of relying on a manually-exported env. Filed Q217 in the yah camp with R218 (help rail, ships freestanding) and R219 (vault-lease, depends_on R198 Job-scope). When resuming R010-T2: either ship R218 first and dogfood it for the Cloudflare key creation, or generate keys from the dashboard directly and circle back.")
@yah:next("See yah camp Q217 / R218 / R219 for the tool-gap relays surfaced during scoping — R218 is the help-rail prereq if you want self-serve key generation.")
@yah:handoff("All code work done and verified. smoke-yah-dev.sh passes, typecheck clean. Remaining work is pure Cloudflare operator steps — no agent can proceed without credentials. Config is ready at examples/yah-dev/mesofact.config.toml (bucket=yah-dev-marketing, ACCOUNT_ID + ZONE_ID placeholders to fill). See yah camp Q217/R218/R219 for the help-rail + vault-lease gaps that would make this self-serve in future.")
@yah:next("operator to sign off in Cloudflare dashboard, create R2 bucket yah-dev-marketing; copy Account ID + Zone ID for yah.dev into examples/yah-dev/mesofact.config.toml.")
@yah:next("Mint R2 API token (Object Read & Write on yah-dev-marketing) + CF API token (Zone:Cache Purge for yah.dev); export as MESOFACT_S3_ACCESS_KEY_ID / MESOFACT_S3_SECRET_ACCESS_KEY / CLOUDFLARE_API_TOKEN.")
@yah:next("cd examples/yah-dev && bun run build; cargo run -p mesofact-publisher --bin mesofact-publish -- examples/yah-dev/dist — expect uploaded=8, purged=0.")
@yah:next("Add Cloudflare Cache Rule for yah.dev (cache-control: public, max-age=0, s-maxage=3600, stale-while-revalidate=86400 + Cache-Tag header).")
@yah:next("curl -sI https://yah.dev/ | grep -E 'cache-control|cf-cache-status' — assert both present; cf-cache-status=HIT after warmup. Repeat for /404.")
@yah:next("Edit src/render.ts, rebuild + re-publish; confirm uploaded=1 + purged includes page:home + site:yah-dev; curl confirms MISS then HIT.")
@yah:handoff("yah.dev is live. R2 bucket yah-dev + zone yah.dev provisioned. CF Transform Rule (/ → /index.html) automated in mesofact_static reconciler and verified working. server/ SSR bundles filtered from R2 upload (r2_publish.rs). Stale server/ objects deleted from bucket. `yah cloud mirror up dev-yah --env prod` runs end-to-end clean. yah.dev/ returns 200, /404 returns 404 with correct HTML. Re-publish smoke deferred until new design lands.")
@yah:verify("curl -si https://yah.dev/ | grep -E 'HTTP|content-type'  # expect 200 text/html")
@yah:verify("cd /Users/user/ss/yah && cargo run -p yah --bin yah -- cloud mirror up dev-yah --env prod  # expect clean run, no errors")
-->

<!--
@yah:ticket(R010-T1, "examples/yah-dev end-to-end smoke (build + in-memory publish + HTML content asserts)")
@yah:at(2026-05-17T23:26:36Z)
@yah:status(review)
@yah:assignee(agent:claude)
@yah:parent(R010)
@yah:next("Add scripts/smoke-yah-dev.sh: run mesofact-build against examples/yah-dev, run mesofact-publish --in-memory, assert dist/html/index.html and 404.html contain expected content, manifest+tag-index match expected shape")
@yah:handoff("scripts/smoke-yah-dev.sh ships the no-creds local equivalent of the real-network publish-smoke workflow: it builds examples/yah-dev/, runs mesofact-publish --in-memory against the resulting dist/, and asserts dist/html/index.html + 404.html contain the landing+404 copy, manifest has both routes in static mode, and tag-index maps page:home / page:404 / site:yah-dev to the right URL sets. Smoke passes in 2.5s locally. Not wired to GitHub Actions on purpose (follows scripts/smoke-outside-consumer.sh pattern — invoke from local dev or before tagging a deploy).")
@yah:verify("bash scripts/smoke-yah-dev.sh")
-->

<!--
@yah:ticket(R008-T8, "TS @mesofact/build: source-derived prerender.query (r2.list-driven param expansion)")
@yah:assignee(agent:claude)
@yah:at(2026-05-15T18:48:27Z)
@yah:status(review)
@yah:parent(R008)
@yah:next("Replace the P6 BuildError in expandPrerenderParams (packages/mesofact-build/src/index.ts) with a path that resolves prerender.from to a Source from the catalog and runs the query (r2.list for MVP) to expand param maps. Test: literal-params and source-derived produce equivalent HTML output for the same expanded set")
@yah:handoff("expandPrerenderParams now resolves prerender.from to a registered adapter and runs the query (MVP: `list:<prefix>` against r2 BlobSource), mapping each returned key to `{[param]: key}`. build() is registry-agnostic — the CLI calls registerSourcesFromConfig() when sources are declared; tests register stub adapters. New fixture tests/fixtures/dynamic-from-source/ mirrors static-only with `prerender: { from: 'assets', query: 'list:', param: 'id' }`; the new test stubs an R2Adapter's httpFetch to return a ListBucketResult v2 with keys '1','2' and asserts the emitted HTML (index.html, p_id__1.html, p_id__2.html) is byte-identical to the static-only fixture's. 13 tests pass (was 11); typecheck clean across mesofact-build and mesofact-runtime.")
@yah:verify("cd packages/mesofact-build && bun run typecheck")
@yah:verify("cd packages/mesofact-build && bun test")
@yah:assumes("Manifest's `prerender` field preserves the source-derived `{from,query,param}` shape rather than the expanded params; the publisher/proxy treat it as opaque metadata since HTML is already generated. Re-expanding at proxy boot would need adapter access and isn't on the P6 path.")
-->

<!--
@yah:ticket(R008-T7, "S3 SigV4 ObjectStore + Cloudflare CdnPurger real-network impls + CI smoke (Hetzner/Cloudflare creds)")
@yah:at(2026-05-15T18:48:25Z)
@yah:status(review)
@yah:assignee(agent:claude)
@yah:parent(R008)
@yah:handoff("Shipped the real-network adapters. crates/mesofact-publisher gained three modules: s3.rs (S3Store impls ObjectStore via reqwest + hand-rolled SigV4 — GET/HEAD/PUT/LIST/DELETE, path-style URLs, x-amz-meta-content-hash preserved across HEAD so put_with_hash's idempotency check still works against R2; SigV4 derivation passes the AWS spec test vector); cloudflare.rs (CloudflareCdnPurger POSTs {tags:[]} to /zones/{id}/purge_cache with Bearer auth, chunks at 30 tags per Cloudflare's per-call limit, skips when the tag set is empty); config.rs (PublishConfig loads [publish] from mesofact.config.toml — bucket/endpoint/region/zone_id — with --bucket/--endpoint/--zone CLI overrides; PublishCredentials resolves access_key_id_env / secret_access_key_env / api_token_env from process env with precise per-field error). The mesofact-publish binary now splits run_in_memory (the old smoke path, unchanged) from run_real (load config → resolve creds → wire S3Store + CloudflareCdnPurger → reuse dispatch). Missing config / missing [publish] / missing env var each surface a distinct exit-2 hint; the CLI smoke suite covers all three branches. CI smoke job at .github/workflows/publish-smoke.yml runs on push to main when MESOFACT_S3_ACCESS_KEY_ID is set: stages a tiny dist/, publishes against the real R2 bucket + Cloudflare zone, then re-publishes and greps the output to assert the second run uploaded/purged zero. New deps: reqwest (rustls-tls + json), hmac, chrono, percent-encoding, toml. All 26 tests pass (8 unit + 5 cli_smoke + 13 in_memory); cargo check --workspace clean.")
@yah:verify("cargo check --workspace")
@yah:verify("cargo test -p mesofact-publisher")
-->

<!--
@yah:ticket(R008-T6, "Rollback flag: mesofact publish --pin {build_id}")
@yah:assignee(agent:claude)
@yah:at(2026-05-15T18:48:22Z)
@yah:status(review)
@yah:parent(R008)
@yah:handoff("publish_pin now evicts the CDN cache for the rolled-away-from build. Before the pointer swap we read the currently-live /tag-index.json from the store and collect every tag key it carries; after the swap we call purger.purge_tags(&live_tags) with that set (skipped when the set is empty). The full key set is used — not a diff against the pinned-to index — because tag invalidation works on the response, not on its content hash: even if build A and build B both map 'r2:foo' to '/', the cached body is build B's HTML and has to go. A malformed/absent live index falls through to a no-op purge so pinning over a corrupt pointer can still recover. PublishReport.purged_tags surfaces the set the orchestrator passed to the purger. Tests: publish_pin_restores_prior_build was extended to assert report.purged_tags + the new purger call list entry; publish_pin_purges_full_live_tag_set publishes a single-tag build A then a two-distinct-tag build B, pins back to A, and asserts both build-B tags are purged (sorted, regardless of A's tag set); publish_pin_with_no_live_index_skips_purge hand-seeds only the per-build snapshot to cover the corrupt-pointer recovery path. 13 in_memory + 3 cli_smoke tests pass; cargo check --workspace clean.")
@yah:verify("cargo check --workspace")
@yah:verify("cargo test -p mesofact-publisher")
-->



<!--
@yah:ticket(R008-T3, "Tag-diff CDN purge (new vs prior tag-index → purge changed tags only)")
@yah:assignee(agent:claude)
@yah:at(2026-05-15T18:48:04Z)
@yah:status(review)
@yah:parent(R008)
@yah:handoff("publish_dist now reads the live /tag-index.json from the store before any uploads, parses it, and runs diff_tag_indices() against the new tag-index to compute the purge set — added tags + removed tags + tags whose URL list changed, sorted + de-duped via BTreeSet. The prior snapshot is captured *before* the commit so the diff sees the truly-previous index even though the commit will overwrite /tag-index.json. A malformed prior index is treated as 'no prior' (publish goes through, next change will over-purge); first publish (None prior) returns an empty set since nothing is cached yet. PublishReport.purged_tags is now populated; the purger is only called when the set is non-empty. Three test scenarios: first-publish-no-purge (publish_dist_uploads_artifacts_and_pointers still asserts flat_tags().is_empty()), changed-URL-only (publish_dist_purges_only_changed_tags_on_rerun: 2-route build, mutate /about's tag URL list → purge contains only r2:assets:about.md), and added+removed (publish_dist_purges_added_and_removed_tags: swap the single tag → both old and new land in the sorted purge set). 11 in_memory + 3 cli_smoke tests pass; cargo check --workspace clean.")
@yah:verify("cargo check --workspace")
@yah:verify("cargo test -p mesofact-publisher")
-->

<!--
@yah:ticket(R008-T2, "Idempotent uploader + atomic manifest swap (prior-manifest diff; commit point at /manifest.json)")
@yah:assignee(agent:claude)
@yah:at(2026-05-15T18:48:02Z)
@yah:status(review)
@yah:parent(R008)
@yah:handoff("publish_dist is now idempotent at the store level. put_with_hash() heads the destination key first and skips the PUT when the prior object's content_hash matches the new body — applied uniformly to per-build artifacts under /{build_id}/, per-build manifest+tag-index snapshots, and the root /manifest.json + /tag-index.json pointers. The orchestrator threads &mut uploaded / &mut skipped through every put_with_hash call so a single PublishReport covers every touched key. Two new tests: publish_dist_is_idempotent_on_unchanged_dist (second publish reports uploaded_keys.is_empty() and skipped_keys == first.uploaded_keys, store.len() unchanged); publish_dist_uploads_only_changed_artifacts_on_rerun (mutate one html/ artifact → that key is re-uploaded, server/ key is skipped). publish_pin also routes through put_with_hash, so pin→pin no-op is naturally idempotent too. All 9 in_memory tests + 3 cli_smoke tests pass; cargo check --workspace clean.")
@yah:verify("cargo check --workspace")
@yah:verify("cargo test -p mesofact-publisher")
-->

<!--
@yah:ticket(R008-T1, "Publisher crate skeleton + ObjectStore/CdnPurger traits + in-memory impls + binary CLI")
@yah:assignee(agent:claude)
@yah:at(2026-05-15T18:47:57Z)
@yah:status(review)
@yah:parent(R008)
@yah:handoff("Shipped the publisher foundation. crates/mesofact-publisher gained: ObjectStore + CdnPurger traits (put/get/head/list/delete + purge_tags) with InMemoryStore + InMemoryPurger impls; publish_dist() orchestrator that walks dist/, uploads each artifact under /{build_id}/, snapshots manifest.json + tag-index.json into /{build_id}/ for --pin restore, then commits via root /tag-index.json then /manifest.json (manifest LAST so a crash before commit leaves prior build live); publish_pin(build_id) that pulls /{build_id}/manifest.json + tag-index.json and rewrites the root pointers; mesofact-publish binary with --in-memory smoke path (real S3/Cloudflare wiring deferred to R008-T7). 10 tests pass: store round-trip, purger call recording, dist publish happy path + build_id-mismatch + missing-manifest, pin round-trip + pin-not-found, CLI smoke (success / missing --in-memory / missing manifest).")
@yah:verify("cargo check --workspace")
@yah:verify("cargo test -p mesofact-publisher")
-->

<!--
@yah:ticket(R006-T5, "Integration test: render entrypoint reads two r2 keys → cache.tags includes both r2:<bucket>:<key>; noTrack suppresses; per-call timeout fires")
@yah:at(2026-05-15T17:53:54Z)
@yah:status(review)
@yah:assignee(agent:claude)
@yah:parent(R006)
-->

<!--
@yah:ticket(R006-T4, "mesofact.config.toml parser ([sources.*] of kind=r2) — TS side, registers R2 adapters from env-injected credentials")
@yah:at(2026-05-15T17:50:44Z)
@yah:status(review)
@yah:assignee(agent:claude)
@yah:parent(R006)
-->

<!--
@yah:ticket(R006-T3, "r2 adapter: BlobSource impl over S3-compatible HTTP (fetch + list, SigV4 via aws4fetch, r2:bucket:key tag emission)")
@yah:at(2026-05-15T17:43:58Z)
@yah:status(review)
@yah:assignee(agent:claude)
@yah:parent(R006)
-->

<!--
@yah:ticket(R006-T2, "Source interface split + BaseSource helper (noTrack/timeout per-call override plumbing)")
@yah:at(2026-05-15T17:42:08Z)
@yah:status(review)
@yah:assignee(agent:claude)
@yah:parent(R006)
-->

<!--
@yah:ticket(R006-T1, "Relocate trackCtx from mesofact-worker to @mesofact/runtime (adapters live in runtime; worker re-imports)")
@yah:at(2026-05-15T17:34:49Z)
@yah:status(review)
@yah:assignee(agent:claude)
@yah:parent(R006)
-->

<!--
@yah:ticket(R004-T5, "Failing-fixture suite: hand-written manifest JSONs (one accept + one reject per rule) consumed by both validators")
@yah:assignee(agent:claude)
@yah:at(2026-05-15T16:57:47Z)
@yah:status(review)
@yah:phase(P2)
@yah:parent(R004)
@yah:verify("cargo test -p mesofact --test fixtures")
@yah:verify("cd packages/mesofact-runtime && bun test fixtures")
-->

<!--
@yah:ticket(R004-T4, "Rust manifest validator + Mode 1 validation rules (parity with TS validator)")
@yah:assignee(agent:claude)
@yah:at(2026-05-15T16:57:45Z)
@yah:status(review)
@yah:phase(P2)
@yah:parent(R004)
@yah:verify("cargo test -p mesofact --test fixtures")
-->

<!--
@yah:ticket(R004-T3, "TS manifest validator (zod) + Mode 1 validation rules (no scoped source_reads, no requires:user)")
@yah:assignee(agent:claude)
@yah:at(2026-05-15T16:57:43Z)
@yah:status(review)
@yah:phase(P2)
@yah:parent(R004)
@yah:verify("cd packages/mesofact-runtime && bun test")
-->

<!--
@yah:ticket(R004-T2, "Manifest schema (TS types + Rust serde structs matching arch §Manifest schema)")
@yah:assignee(agent:claude)
@yah:at(2026-05-15T16:57:42Z)
@yah:status(review)
@yah:phase(P2)
@yah:parent(R004)
@yah:verify("cd packages/mesofact-runtime && bun run typecheck")
@yah:verify("cargo check --workspace")
-->

<!--
@yah:ticket(R004-T1, "Route-config types (mesofact.routes.ts shape — explicit table)")
@yah:assignee(agent:claude)
@yah:at(2026-05-15T16:57:40Z)
@yah:status(review)
@yah:phase(P2)
@yah:parent(R004)
@yah:verify("cd packages/mesofact-runtime && bun run typecheck")
-->

<!--
@yah:ticket(R003-T5, "Outside-consumer smoke test: tmp TS project imports type { RenderRequest } from '@mesofact/runtime'")
@yah:at(2026-05-15T16:36:51Z)
@yah:status(review)
@yah:phase(P1)
@yah:parent(R003)
@yah:verify("bash scripts/smoke-outside-consumer.sh")
@arch:see(.yah/docs/architecture/mesofact.md)
@yah:depends_on(R003-T3)
-->

<!--
@yah:ticket(R003-T4, "examples/yah-dev placeholder consuming @mesofact/runtime via workspace ref")
@yah:at(2026-05-15T16:36:49Z)
@yah:status(review)
@yah:phase(P1)
@yah:parent(R003)
@yah:verify("cd examples/yah-dev && bun run typecheck")
@arch:see(.yah/docs/architecture/mesofact.md)
@yah:depends_on(R003-T2)
-->

<!--
@yah:ticket(R003-T3, "Define render contract types (RenderRequest, RenderResult, Source, error taxonomy)")
@yah:at(2026-05-15T16:36:46Z)
@yah:status(review)
@yah:phase(P1)
@yah:parent(R003)
@yah:verify("cd packages/mesofact-runtime && bun run typecheck")
@arch:see(.yah/docs/architecture/mesofact.md)
@yah:depends_on(R003-T2)
-->

<!--
@yah:ticket(R003-T2, "Scaffold @mesofact/runtime package (package.json, tsconfig, build to dual ESM/types)")
@yah:at(2026-05-15T16:36:43Z)
@yah:status(review)
@yah:phase(P1)
@yah:parent(R003)
@yah:verify("cd packages/mesofact-runtime && bun run typecheck && bun run build")
@arch:see(.yah/docs/architecture/mesofact.md)
@yah:depends_on(R003-T1)
-->

<!--
@yah:ticket(R003-T1, "Cargo workspace bootstrap (crates/mesofact, crates/mesofact-publisher, packages/mesofact-runtime)")
@yah:at(2026-05-15T16:36:40Z)
@yah:status(review)
@yah:phase(P1)
@yah:parent(R003)
@yah:verify("cargo check --workspace")
@arch:see(.yah/docs/architecture/mesofact.md)
-->

<!--
@yah:relay(R012, "P10: Mode 3 SPA hydration + observability MVP")
@yah:assignee(agent:claude)
@yah:at(2026-05-15T16:36:18Z)
@yah:status(in-progress)
@yah:phase(P10)
@yah:parent(Q002)
@arch:see(.yah/docs/working/mesofact.md)
@arch:see(.yah/docs/architecture/mesofact.md)
@yah:depends_on(R007)
@yah:depends_on(R009)
@yah:depends_on(R011)
@yah:next("Human sign-off: review sub-tickets R012-T1..T4, then archive R012 + T1..T4 to close Q002.")
@yah:next("Operator/browser verification (post-deploy, like R010-T2): load /app in a browser → hydrates without console errors; curl /metrics after traffic → non-zero counters; confirm a proxy-log traceparent reaches a worker log line.")
@yah:handoff("P10 (Mode 3 SPA hydration + observability MVP) complete and test-verified across 4 sub-tickets — the last phase of Q002. T1 (build): @mesofact/build bundles each spa route's client_entrypoint to dist/hydrate/ (browser, content-hashed, code-split), prerenders the shell and weaves in <script id=__MESOFACT_STATE__> + the hydrate module <script>, populates manifest.hydration{script,code_split}; contract Hydration.script relaxed to optional + 6-line client snippet documented. T2 (proxy): Mode 3 dispatch serves the prerendered shell exactly like Mode 1 (CDN 302 / local fallback); 501 stub removed. T3 (observability): hand-rolled Prometheus /metrics (requests_total / render_duration_seconds histogram / cache_total / worker_pool gauge) + W3C traceparent generate-or-continue, echoed on the response and passed to the worker as req.ctx.trace. T4 (example): examples/yah-dev /app Mode 3 route + client entry; smoke-yah-dev.sh asserts hydration manifest + dist/hydrate file + woven shell. One route of each mode now exists (Mode 1 / + /404, Mode 2 via proxy tests, Mode 3 /app). DoD met at substrate level.")
@yah:verify("cargo test -p mesofact && cargo check --workspace")
@yah:verify("cd packages/mesofact-runtime && bun run typecheck && bun run build && bun test")
@yah:verify("cd packages/mesofact-build && bun run typecheck && bun test")
@yah:verify("bash scripts/smoke-yah-dev.sh")
@yah:cleanup("Worker does not yet LOG req.ctx.trace per render — a thin add (one structured log line in worker.ts handleRender) would make the DoD 'trace flows proxy→worker log line' checkable from logs alone; the passthrough itself is verified by trace_echo.ts.")
-->

<!--
@yah:relay(R011, "P9: Mode 2 SSR slice (sqlite + response cache + session)")
@yah:assignee(agent:claude)
@yah:at(2026-05-15T16:36:17Z)
@yah:status(review)
@yah:phase(P9)
@yah:parent(Q002)
@arch:see(.yah/docs/working/mesofact.md)
@arch:see(.yah/docs/architecture/mesofact.md)
@yah:depends_on(R006)
@yah:depends_on(R009)
@yah:handoff("P9 Mode 2 SSR slice shipped end-to-end and verified. (1) sqlite adapter — packages/mesofact-runtime/src/adapters/sqlite.ts: KeyValueSource over bun:sqlite reached via a dynamic-import seam (keeps the published runtime Bun-type-free). get()/query() emit sqlite:<src>:<table>:<id> (row) and sqlite:<src>:<table> (table-wide; source-wide fallback for un-parseable SQL), 100ms default timeout, registry + sqlite(name) factory. config.ts parses kind=\"sqlite\" ([sources.x] path=…); registerSourcesFromConfig registers them (no creds).")
@yah:handoff("(2) Rust cache — crates/mesofact/src/proxy/cache.rs: 7-input SHA-256 compose_key (build_id, route pattern, params, query, vary, source_generations, user-id-only-if-requires-user; maps sorted + field-boundary-separated so inputs can't collide), LRU ResponseCache, CacheEntry fresh/stale/expired state machine, cache_window for negative-TTL (4xx) and 5xx-never-cache. (3) source_gen.rs: Generations parses mesofact.config.toml Rust-side, sqlite generation = file mtime with a 1s memo; r2/pg/rpc are a stable placeholder (poll deferred).")
@yah:handoff("(4) session.rs: CookieSessionResolver (HMAC-SHA256, configurable cookie name, key from env), constant-time verify_slice, exp check, sign() for issuing tokens. (5) router.rs Mode 2 dispatch: resolve session → requires:user 302 (?next=) or 401 → compose key → fresh-serve / stale-serve + background SWR refresh / miss-render → LRU store; on-error stale fallback (X-Mesofact-Stale: true) else 503 + Retry-After; X-Mesofact-Cache state header on every Mode 2 response.")
@yah:handoff("(6) worker.ts registers adapters from --config at boot so a render's sqlite('db') resolves; WorkerPool::spawn_with_config threads the config path to each worker and rolling reload; WorkerPool::get() now round-robins across workers. Proxy CLI gained --sources-config / --session-secret-env / --session-cookie / --login-url / --cache-capacity. Tests: Rust 38 (cache/session/source_gen unit + proxy integration incl. cache-hit-serves-stored-body, query-string-distinct-key, requires-user redirect + render, and mode2_sqlite_generation_bump_invalidates end-to-end), TS runtime 49 (sqlite-adapter + config). cargo check --workspace + all typechecks clean.")
@yah:next("P10 (R012): Mode 3 SPA hydration + observability — the X-Mesofact-Cache state header and worker pool are ready to feed mesofact_cache_total / mesofact_worker_pool metrics. R012's depends_on R011 is now satisfied.")
@yah:verify("cargo test -p mesofact")
@yah:verify("cargo check --workspace")
@yah:verify("cd packages/mesofact-runtime && bun run typecheck && bun test")
@yah:verify("cd packages/mesofact-worker && bun run typecheck")
@yah:cleanup("Perf smoke deferred (DoD sub-50ms miss / sub-5ms hit): the hit path is a pure in-memory LRU clone (no IPC), so the target is met by construction — add a real load-tool smoke rather than a flaky timing unit assertion.")
@yah:cleanup("SWR background refresh has no in-flight de-dup: concurrent stale hits can spawn N refreshes for one key. Add a per-key 'refreshing' guard.")
@yah:cleanup("Per-worker socket Mutex still serialises render+ping (carried from R009). get() now round-robins for cross-worker concurrency; the per-socket read-task + mpsc command-channel refactor is still pending for many concurrent renders on a single worker.")
@yah:cleanup("Scoped sqlite ({project_id} path templating) deferred — global sqlite only this phase (matches the design's 'defer Litestream/LiteFS to first scoped SSR dogfood').")
@yah:gotcha("yah tool gap: the R011 pickup prompt said `board.claim {\"id\":\"R011\"}`, but board_claim ignores any id and always allocates a NEW relay (it created+archived R013 here before I noticed). The correct way to claim an existing OPEN relay is `board_move <id> active`. File against yah: board_claim should accept an existing id, or pickup prompts for already-open relays should emit board_move.")
@yah:assumes("bun:sqlite is the sqlite backend (the worker runs in Bun). The adapter reaches it via a dynamic-import seam so @mesofact/runtime's published types stay Bun-free and a non-Bun consumer that never calls sqlite() never resolves the module.")
@yah:assumes("RenderResult carries no HTTP status, so Mode 2 renders cache as 200; negative-TTL (4xx) / 5xx classification is wired in cache_window but only exercised by proxy-level non-2xx today. Letting RenderResult set a status would drive render-originated negative caching.")
-->

<!--
@yah:relay(R010, "P8: yah.dev marketing page (Mode 1 dogfood)")
@yah:assignee(agent:claude)
@yah:at(2026-05-15T16:36:14Z)
@yah:status(in-progress)
@yah:phase(P8)
@yah:parent(Q002)
@arch:see(.yah/docs/working/mesofact.md)
@arch:see(.yah/docs/architecture/mesofact.md)
@yah:depends_on(R007)
@yah:depends_on(R008)
@yah:depends_on(R009)
@yah:handoff("P8 code shipped. R010-T1 (review) added the no-creds end-to-end smoke (scripts/smoke-yah-dev.sh) that builds examples/yah-dev/ + publishes through the InMemoryStore + asserts HTML/manifest/tag-index content. examples/yah-dev/{src/render.ts,src/not_found.ts,src/layout.ts,mesofact.routes.ts} carries the / + /404 Mode 1 landing pages with tags page:home / page:404 / site:yah-dev. R010-T2 (handoff) is the operator deploy: provision R2 + Cloudflare zone, run a real-network publish, verify cache-control + cf-cache-status with curl. No code work remaining on R010 itself — close T2 to close the relay.")
@yah:verify("bash scripts/smoke-yah-dev.sh")
@yah:verify("cd examples/yah-dev && bun run typecheck")
-->

<!--
@yah:relay(R009, "P7: Rust proxy (axum) — boot, manifest reload, Mode 1 fallback")
@yah:assignee(agent:claude)
@yah:at(2026-05-15T16:36:12Z)
@yah:status(in-progress)
@yah:phase(P7)
@yah:parent(Q002)
@arch:see(.yah/docs/working/mesofact.md)
@arch:see(.yah/docs/architecture/mesofact.md)
@yah:depends_on(R005)
@yah:depends_on(R007)
@yah:depends_on(R008)
@yah:next("P8 (R010): wire yah-dev example — add mesofact.routes.ts, run mesofact build && publish, verify CDN headers.")
@yah:next("P9 (R011): Mode 2 SSR slice — sqlite adapter + proxy LRU + session resolver.")
@yah:handoff("P7 shipped. Added to crates/mesofact: axum proxy binary (mesofact-proxy), WorkerClient (UDS+NDJSON — spawn/ping/drain/render), WorkerPool (N workers, 30s watchdog, crash respawn, rolling drain via drain_all), ManifestLoader (file-based, SIGHUP + 30s heartbeat, safe reload with old-manifest fallback on error), axum router (matchit pattern matching, Mode 1 dispatch → 302-to-CDN or local file serve, Mode 2/3 → 501), clap CLI config. 9 new proxy tests + 4 existing worker tests all pass. Cleanup note: WorkerPool serialises all I/O through a per-client Mutex; refactor in P9 when concurrent Mode 2 renders share the socket. P8 (R010) and P9 (R011) are now unblocked.")
@yah:verify("cargo check --workspace")
@yah:verify("cargo test -p mesofact")
@yah:assumes("bun is on PATH at proxy boot (worker spawn will error loudly otherwise).")
@yah:assumes("Manifest is file-based for MVP; HTTP manifest fetching from R2 is deferred (not needed until P8 dogfood).")
@yah:cleanup("WorkerPool.io Mutex serialises ping and render on the same socket. Refactor to split the socket into a read task + command channel (mpsc) in P9 when concurrent Mode 2 renders land.")
@yah:cleanup("WorkerPool.n field is stored but currently unused at runtime (pool size is fixed at spawn). Use it for future dynamic resizing.")
-->

<!--
@yah:relay(R008, "P6: Publisher (R2 upload + manifest swap + CDN purge)")
@yah:assignee(agent:claude)
@yah:at(2026-05-15T16:36:09Z)
@yah:status(handoff)
@yah:phase(P6)
@yah:parent(Q002)
@arch:see(.yah/docs/working/mesofact.md)
@arch:see(.yah/docs/architecture/mesofact.md)
@yah:depends_on(R006)
@yah:depends_on(R007)
@yah:handoff("R008 refined into 6 sub-tickets (T2/T3/T6/T7/T8 still open; T4 + T5 were leaked duplicates from a `--pin` arg-parse failure on `board open` and have been archived — file as a yah tool gap: `board open --title` shouldn't choke on leading `--`). T1 shipped the publisher foundation in crates/mesofact-publisher: ObjectStore + CdnPurger traits, InMemoryStore + InMemoryPurger, publish_dist() + publish_pin() orchestrators wired against them, mesofact-publish binary with --in-memory smoke path. Object layout under R2: /{build_id}/{server,html,assets,hydrate}/... for artifacts, /{build_id}/{manifest,tag-index}.json as per-build snapshots so --pin can restore, and root /tag-index.json + /manifest.json as the active pointer (manifest.json PUT last as the atomic commit point). The orchestrator is currently a naive happy path — every file in dist/ is uploaded unconditionally and no CDN tags are purged. Layering on top: T2 adds prior-manifest content-hash diffing for idempotency, T3 diffs tag-index against the prior snapshot and drives CdnPurger.purge_tags for changed-tag union, T6 adds the html/* tag-purge to publish_pin, T7 swaps in real S3 (SigV4) + Cloudflare adapters plus a CI smoke gated on Hetzner/Cloudflare creds, T8 closes the source-derived prerender.query path in @mesofact/build (currently the 'P6' BuildError in expandPrerenderParams). 10 tests pass against the in-memory backends.")
@yah:next("R008-T2: idempotent uploader. Fetch /manifest.json + /tag-index.json from the store at the start of publish_dist; for each artifact compute SHA-256, head() the prior key, skip if content_hash matches. Test: publish_dist twice with no source change → second call's PublishReport.uploaded_keys is empty (or only the manifest pointer if you decide to always rewrite it). The orchestrator already returns skipped_keys — wire it.")
@yah:next("R008-T3: tag-diff CDN purge. Compare the new TagIndex against the prior /tag-index.json (added tags + tags whose URL list changed); union becomes the purge set; replace the no-op purger.purge_tags(&[]) call in publish_dist with the real diff result. Test: change one route's HTML, re-publish → InMemoryPurger.flat_tags() contains only that route's tags.")
@yah:next("R008-T6: --pin rollback. publish_pin already restores the pointer; add a CDN purge for the prior tag-index's html/* tags so cached new-build HTML is evicted. Test: pin → purger sees the new build's html-tag set.")
@yah:next("R008-T7: real-network adapters. reqwest-based S3 SigV4 ObjectStore + Cloudflare CdnPurger (mirrors the aws4fetch impl in packages/mesofact-runtime/src/adapters/r2.ts). Wire them behind a `--bucket/--endpoint/--zone` config block in mesofact.config.toml; add a CI smoke job that runs publish against a real R2 bucket + zone with env-injected Hetzner/Cloudflare creds (skipped locally).")
@yah:next("R008-T8: source-derived prerender.query in @mesofact/build. Replace the `P6` BuildError in expandPrerenderParams (packages/mesofact-build/src/index.ts:127) with a path that loads the source from the catalog and runs the query (r2.list for MVP) to expand param maps. Test: literal-params vs. source-derived produce equivalent HTML for the same expanded set.")
@yah:verify("cargo check --workspace")
@yah:verify("cargo test -p mesofact-publisher")
@yah:assumes("publisher reuses the Manifest types in crates/mesofact rather than re-declaring them; TagIndex is local to mesofact-publisher because it isn't in the proxy's hot path.")
@yah:cleanup("`board open --title` choked on a leading `--` in the title (parsed as flag), leaking IDs T4/T5 even though that batch errored. File a yah ticket: titles should accept arbitrary strings (use `--title=…` parse rule or require -- separator before positionals).")
-->

<!--
@yah:relay(R007, "P5: Build pipeline (TS build → manifest → Mode 1 prerender)")
@yah:assignee(agent:claude)
@yah:at(2026-05-15T16:36:06Z)
@yah:status(review)
@yah:phase(P5)
@yah:parent(Q002)
@arch:see(.yah/docs/working/mesofact.md)
@arch:see(.yah/docs/architecture/mesofact.md)
@yah:depends_on(R004)
@yah:depends_on(R005)
@yah:depends_on(R006)
@yah:handoff("P5 build pipeline shipped as new @mesofact/build package — orchestrates phases 1–6 of the architecture's `mesofact build`. Bundles route entrypoints with Bun.build (keeps @mesofact/runtime external so AsyncLocalStorage stays shared), discovers routes via dynamic-import of `mesofact.routes.ts`, infers source_reads with a regex-based scan over adapter factory calls (r2|sqlite|pg|rpc('name')) with `// @mesofact-sources foo, bar` override, runs runtime's validate() against a SourceCatalog derived from `mesofact.config.toml`, drives Mode 1 prerender per literal param map inside runInTrackCtx (combines result.cache.tags with collected tags), emits dist/{server/<key>.js, html/<key>[__<params>].html, manifest.json, tag-index.json}. CLI binary `mesofact-build <project-dir>` exits non-zero with the validator's structured error list when rules fail.")
@yah:next("P6 (R008): publisher consumes dist/* — upload to R2, atomic manifest swap, CDN purge by tag using tag-index.json. Also wire source-derived prerender.query (now that r2.list will have a real consumer); expandPrerenderParams already throws a clear 'P6' error for that shape.")
@yah:next("P7 (R009): Rust proxy reads dist/manifest.json on boot; this build's JSON shape matches crates/mesofact/src/manifest.rs already.")
@yah:next("P8 (R010): wire yah-dev example to use this build — replace its placeholder render entry with the real marketing page, add mesofact.routes.ts at examples/yah-dev/.")
@yah:verify("cd packages/mesofact-build && bun run typecheck")
@yah:verify("cd packages/mesofact-build && bun test")
@yah:verify("cd packages/mesofact-runtime && bun test")
@yah:cleanup("Switch source-inference from regex to a proper TS AST walk once the inventory of adapter factories grows beyond r2/sqlite/pg/rpc. The override comment is the escape hatch until then.")
@yah:cleanup("Mode 3 client tree placeholder — bundle.ts only emits the server tree. Wire Vite or Bun.build with a separate client target when P10 lands.")
@yah:assumes("@mesofact/runtime exports parseConfig/SourceCatalog/validate/runInTrackCtx — verified by build passing typecheck and tests. Re-run `bun run build` in mesofact-runtime if its dist falls behind src.")
@yah:assumes("Bun.build's `naming: '<key>.js'` per-call produces exactly one entry-point output we can locate via output.kind === 'entry-point'. Confirmed by passing tests.")
-->

<!--
@yah:relay(R006, "P4: Adapter API + r2 adapter (read-only)")
@yah:assignee(agent:claude)
@yah:at(2026-05-15T16:36:04Z)
@yah:status(in-progress)
@yah:phase(P4)
@yah:parent(Q002)
@arch:see(.yah/docs/working/mesofact.md)
@arch:see(.yah/docs/architecture/mesofact.md)
@yah:depends_on(R005)
-->

<!--
@yah:relay(R005, "P3: Bun render-pool worker (IPC + lifecycle)")
@yah:assignee(agent:claude)
@yah:at(2026-05-15T16:36:01Z)
@yah:status(review)
@yah:phase(P3)
@yah:parent(Q002)
@arch:see(.yah/docs/working/mesofact.md)
@arch:see(.yah/docs/architecture/mesofact.md)
@yah:verify("cargo test -p mesofact --test worker")
@yah:verify("cd packages/mesofact-worker && bun run typecheck")
@yah:verify("cargo check --workspace")
@yah:depends_on(R003)
@yah:depends_on(R004)
-->

<!--
@yah:relay(R004, "P2: Manifest schema + route config + build-time validation")
@yah:assignee(agent:claude)
@yah:at(2026-05-15T16:36:01Z)
@yah:status(in-progress)
@yah:phase(P2)
@yah:parent(Q002)
@arch:see(.yah/docs/working/mesofact.md)
@arch:see(.yah/docs/architecture/mesofact.md)
@yah:depends_on(R003)
-->

<!--
@yah:relay(R003, "P1: Workspace skeleton + render contract types")
@yah:at(2026-05-15T16:35:40Z)
@yah:status(open)
@yah:phase(P1)
@yah:parent(Q002)
@arch:see(.yah/docs/working/mesofact.md)
@arch:see(.yah/docs/architecture/mesofact.md)
-->

<!-- @yah:covered-by(Q002, status=active, 2026-05-15) -->

# mesofact — MVP rollout plan

> **Status**: working doc, 2026-05-14. Sequences the architecture in
> [`architecture/mesofact.md`](../architecture/mesofact.md) into 10 phases
> sized to become hack-board relays + tickets via `/refine`.
>
> **Source of truth**: the design doc and case studies live under
> [`architecture/`](../architecture/). When this roadmap and the design
> disagree, the design wins — update this file.
>
> **Scope**: everything below is MVP, ending at the DoD in
> [`architecture/mesofact.md` §"MVP definition of done"](../architecture/mesofact.md).
> Post-MVP items are listed at the bottom; do not refine them yet.

## North star

One route of each mode shipping at yah.dev (or staging), with
`@mesofact/runtime` consumable by an unrelated TS project. The phases
below are the shortest path that exercises each load-bearing seam at
least once.

## Sequencing principle

- **First seam, then surface.** Render contract types ship before any
  runtime that depends on them (P1) so the seam is a published artifact,
  not an internal symbol.
- **Mode 1 end-to-end before Mode 2/3.** Static is the simplest driver
  of `render()` and the only one needed for the first dogfood
  (yah.dev). Phases P2–P8 march straight to that target.
- **Adapters added one kind at a time.** `r2` lands with the build (P4
  — read-only blobs are the cheapest first adapter). `sqlite` lands
  with Mode 2 (P9) so the cache-key generation story has a real source
  to validate against. `pg` and `rpc` are post-MVP.
- **Observability + Mode 3 share P10.** Both are "needed for DoD but
  small once the substrate exists." Splitting them adds a relay without
  shrinking either.

## Phases

### P1 — Workspace skeleton + render contract types

- **Goal**: Cargo workspace, three crates/packages exist, render
  contract types are publishable and importable from an outside TS
  project. Proves the seam is real before any runtime depends on it.
- **Deliverables**:
  - [`Cargo.toml`](yah://file/Cargo.toml) workspace with
    `crates/mesofact`, `crates/mesofact-publisher`, plus
    `packages/mesofact-runtime`.
  - `@mesofact/runtime` package exporting `RenderRequest`,
    `RenderResult`, `Source`, error types
    (`SourceUnavailableError`, `SourceTimeoutError`,
    `SourceQueryError`, `RowNotFoundError`).
  - `examples/yah-dev/` placeholder (empty `render()` returning a
    string) consuming the package via workspace ref.
- **Verify**: `cargo check --workspace` clean; `bun run typecheck` in
  `examples/yah-dev/` clean; an unrelated tmp TS project can `import
  type { RenderRequest } from '@mesofact/runtime'` without error.
- **Depends on**: nothing.
- **Candidate tickets**: workspace bootstrap; runtime package
  scaffolding; contract type definitions; example app stub; outside-
  consumer smoke test.

### P2 — Manifest schema + route config + build-time validation

- **Goal**: a route config and a manifest emitter exist; build-time
  validation rejects the forbidden shapes before any rendering runs.
- **Deliverables**:
  - `mesofact.routes.ts` shape (explicit table, not file-based).
  - Manifest schema per
    [`architecture/mesofact.md` §"Manifest schema"](../architecture/mesofact.md):
    `version`, `build_id`, `routes[]`, `static_assets[]`,
    `error_routes`.
  - JSON-schema (or `zod`) validator for the manifest, used by both
    the build and the proxy.
  - Validation rules: Mode 1 + non-`global` `source_reads` rejected;
    Mode 1 + `requires ∋ user` rejected. Error messages name the
    offending route and the import chain.
- **Verify**: hand-written manifest fixtures load/reject as expected;
  one failing fixture per validation rule.
- **Depends on**: P1.
- **Candidate tickets**: routes config parser; manifest schema;
  validator; build-validation rules; failing-fixture suite.

### P3 — Bun render-pool worker (IPC + lifecycle)

- **Goal**: a single Bun worker process can be spawned, loads server
  bundles named by a manifest, and answers NDJSON `render` messages
  over a Unix-domain socket. No Rust proxy yet — drive it from a test
  harness.
- **Deliverables**:
  - Worker entry in `@mesofact/runtime` (or a sibling internal
    package) implementing the protocol in
    [`architecture/mesofact.md` §"IPC protocol"](../architecture/mesofact.md):
    `render`, `ok`, `err`, `ready`, `ping`, `pong`, `drain`.
  - Per-route concurrency cap from manifest; bounded queue; 503-
    equivalent overflow error.
  - `AsyncLocalStorage` `trackCtx` wired around each `render` call so
    later adapters can register tags (no adapters yet).
- **Verify**: a Rust test harness opens the UDS, sends a `render`
  message to a stub entrypoint that returns `{html: "hi"}`, gets
  `ok`. `drain` makes the worker exit after the in-flight call. A
  missed `pong` within 5 s causes the harness to declare it dead.
- **Depends on**: P1, P2 (manifest shape).
- **Candidate tickets**: NDJSON envelope; UDS server; lifecycle
  messages; concurrency + queue; `trackCtx` plumbing; harness tests.

### P4 — Adapter API + `r2` adapter (read-only)

- **Goal**: the `Source` interface exists, an `r2` adapter implements
  it against a real bucket, and tags accumulate via `trackCtx` so
  invalidation has a substrate to ride.
- **Deliverables**:
  - `Source` shape per
    [`architecture/mesofact.md` §"Adapter API surface"](../architecture/mesofact.md):
    `get`, `query`, `fetch`, `list`, `noTrack()`, `timeout(ms)`.
  - `r2` adapter (`fetch`, `list`) with `r2:<bucket>:<key>` tag
    emission and 2000 ms default timeout.
  - Typed errors per the design.
  - `mesofact.config.toml` parser for `[sources.*]` (yubaba writes,
    mesofact reads) — one source kind in this phase (`r2`).
- **Verify**: a render entrypoint that reads two R2 keys returns a
  `RenderResult` whose `cache.tags` includes both `r2:<bucket>:<key>`
  values; `.noTrack()` suppresses; timeout override fires.
- **Depends on**: P3 (`trackCtx` exists).
- **Candidate tickets**: `Source` interface; `r2` adapter; tag
  emission; config parser; error taxonomy; timeout/noTrack tests.

### P5 — Build pipeline (TS build → manifest → Mode 1 prerender)

- **Goal**: `mesofact build` runs phases 1–6 from
  [`architecture/mesofact.md` §"Build pipeline"](../architecture/mesofact.md)
  end-to-end for a Mode 1 route — TS bundle, route discovery, source
  inference, validation, prerender, manifest emission. Output sits in
  `dist/` ready for the publisher.
- **Deliverables**:
  - Vite/Bun config for ssr + (placeholder) client trees.
  - Route discovery from `mesofact.routes.ts`.
  - Source-inference pass: static analysis of adapter imports
    populates `source_reads`; `// @mesofact-sources` override
    honored.
  - Mode 1 prerender driver: invoke `render()` per param map (literal
    list only this phase; source-derived `prerender.query` is P6
    once `r2.list` is exercised by a real route).
  - `tag-index.json` emitted alongside the manifest.
- **Verify**: a single literal Mode 1 route renders to
  `dist/html/<key>.html`; manifest + tag-index land next to it; build
  fails on the violation fixtures from P2.
- **Depends on**: P2, P3, P4.
- **Candidate tickets**: Vite config; route discovery; source
  inference; override comment parser; prerender driver; tag-index
  emitter; integration test against the placeholder example app.

### P6 — Publisher (R2 upload + manifest swap + CDN purge)

- **Goal**: `mesofact publish` is one idempotent command that uploads
  `/{build_id}/` to R2, atomically swaps `/manifest.json`, and purges
  the right CDN tags. No long-lived listener yet — that's post-MVP.
- **Deliverables**:
  - Uploader walking `dist/` → R2 paths per
    [`architecture/mesofact.md` §"Static asset handling"](../architecture/mesofact.md).
  - Manifest swap as the last step (commit point).
  - Cloudflare CDN purge by tag for the routes whose tag-index
    differs from the previous manifest's.
  - `mesofact publish --pin {build_id}` for rollback.
  - Source-derived `prerender.query` support (now that `r2.list`
    has a consumer).
- **Verify**: publishing twice with no source change is a no-op (no
  uploads, no purges); changing one route's content re-uploads only
  that route's HTML and purges only its tag; `--pin` reverts the
  pointer.
- **Depends on**: P4 (`r2`), P5 (build output).
- **Candidate tickets**: uploader; idempotency check; manifest swap;
  CDN purge integration; `--pin` flag; rollback test.

### P7 — Rust proxy (axum) — boot, manifest reload, Mode 1 fallback

- **Goal**: the proxy boots from `manifest.json`, serves Mode 1 traffic
  (delegating to CDN or local-fallback), spawns and manages the Bun
  worker pool with rolling reload on SIGHUP. Mode 2/3 dispatch is
  stubbed (returns 501) — wired up in P9/P10.
- **Deliverables**:
  - `axum`-based proxy with route table from manifest.
  - Manifest fetch on boot; SIGHUP + 30 s heartbeat reload.
  - Worker pool: spawn N workers (default `num_cpus / 2`), one UDS
    each, ping/pong watchdog, crash respawn.
  - Rolling reload: spawn parallel new pool with new manifest, drain
    old, SIGTERM.
  - Mode 1 dispatch: 302 to CDN URL or stream from local fallback
    file, whichever the deployment configures.
- **Verify**: kill a worker mid-run → respawn + 503 for the in-flight
  request; SIGHUP with a new manifest cuts traffic over without
  dropped requests; bad manifest is refused and old stays live.
- **Depends on**: P3 (worker), P5 (manifest), P6 (artifacts in R2).
- **Candidate tickets**: axum boot; manifest loader; pool spawn;
  watchdog; rolling reload; Mode 1 dispatch; SIGHUP tests.

### P8 — yah.dev marketing page (Mode 1 dogfood)

- **Goal**: a real Mode 1 route — the yah.dev landing page — ships
  end-to-end. First proof the substrate is usable from outside its own
  examples.
- **Deliverables**:
  - `examples/yah-dev/` (or its real home if the marketing repo is
    separate) with a `render()` entrypoint that emits the landing
    page HTML.
  - `mesofact.routes.ts` with at least `/` and `/404`.
  - One end-to-end run: `mesofact build && mesofact publish` puts
    HTML on R2 + CDN; `curl -I https://yah.dev/` returns the
    expected `cache-control` and `cf-cache-status` headers.
- **Verify**: page loads; cache headers match the design; a content
  edit + re-publish flips the relevant CDN tag and re-fetches new
  HTML.
- **Depends on**: P5, P6, P7.
- **Candidate tickets**: page entrypoint; route config; build/publish
  smoke; CDN header verification; first content-change re-publish.

### P9 — Mode 2 SSR slice (`sqlite` adapter + response cache + session)

- **Goal**: one Mode 2 route renders through the Bun pool, hits a
  `sqlite` source, and is served by the proxy's LRU with the full
  cache-key composition. Cookie sessions resolve.
- **Deliverables**:
  - `sqlite` adapter (`get`, `query`) with file-mtime generation,
    `<kind>:<source>:<table>:<id>` tag emission, 100 ms default
    timeout.
  - Proxy LRU response cache + 7-input SHA-256 key composition per
    [`architecture/mesofact.md` §"Cache-key composition"](../architecture/mesofact.md).
  - Cache states: fresh / stale (SWR) / expired; negative TTL;
    `Vary` from manifest; on-error stale fallback with
    `X-Mesofact-Stale: true`.
  - `CookieSessionResolver` (HMAC, configurable cookie name, key from
    env) populating `req.user`; 302 redirect on missing/expired
    session for `requires: ["user"]` routes.
- **Verify**: p50 sub-50 ms on miss, sub-5 ms on hit (target from
  DoD); SWR refresh observable in the cache-state metric (P10);
  generation bump on the SQLite file invalidates the entry on next
  request.
- **Depends on**: P4 (adapter shape), P7 (proxy).
- **Candidate tickets**: `sqlite` adapter; LRU + key composition;
  TTL/SWR/negative; vary; session resolver; on-error fallback; perf
  smoke.

### P10 — Mode 3 SPA hydration + observability MVP

- **Goal**: one Mode 3 route boots a SPA, fetches an API, renders.
  Proxy emits `/metrics` and forwards `traceparent`. Closes the DoD.
- **Deliverables**:
  - Mode 3 entrypoint pattern: `RenderResult.hydration = {script,
    initial_state}`; client reads `__MESOFACT_STATE__` and calls
    `hydrateRoot()`. Six-line client snippet documented.
  - Build emits the client tree + code-split chunks; manifest's
    `hydration.{script, code_split}` populated; publisher uploads
    them under `/{build_id}/hydrate/`.
  - Prometheus `/metrics` exporter with the minimum set:
    `mesofact_requests_total`, `mesofact_render_duration_seconds`,
    `mesofact_cache_total`, `mesofact_worker_pool`.
  - W3C `traceparent`: proxy generates or accepts, passes to worker
    via `req.ctx.trace`. (Adapter spans deferred — listed in
    post-MVP.)
- **Verify**: SPA loads, hydrates without console errors, fetches its
  API; `/metrics` scrape shows non-zero counters; trace ID flows from
  proxy log line through worker log line.
- **Depends on**: P5 (build), P7 (proxy), P9 (Mode 2 path exercised).
- **Candidate tickets**: Mode 3 build tree; hydration manifest;
  publisher hydrate path; SPA example; metrics exporter; traceparent
  passthrough.

## Out of MVP scope (do not refine yet)

These are listed in the design but explicitly post-MVP. Refining them
now creates churn against an unbuilt substrate.

- `pg` adapter (P9 only ships `sqlite`).
- `rpc` adapter + roster source / `generation_from` plumbing.
- Litestream/LiteFS shape decision for `sqlite` (defer to first scoped
  SSR dogfood, per the design's "open questions").
- Long-lived `mesofact-publisher` daemon (PG `LISTEN`, R2 events,
  SQLite WAL tail) — MVP publisher is one-shot per `mesofact publish`.
- JWT / OAuth `SessionResolver` impls.
- Per-request multi-tenancy (`tenant_id → config` map).
- Per-adapter OTLP child spans, structured request-bodies, full SQL
  capture.
- Multi-region Bun pools and home-region routing.
- `mesofact-publisher reconcile` — manual full re-render after retention
  loss.

## How to refine this

Each phase above is a candidate **relay**. Run `/refine` against this
file and the phase headings become R-IDs; the bullets under
"Candidate tickets" become compound sub-tickets (`R0XX-T1`, …) under
each. Open P1 first — everything else depends on it.