//! `revalidate` — the ephemeral revalidate receiver: the mesofact-native
//! replacement for the standalone `almanac-serve` binary (W225 §3/§4).
//!
//! ## What it is
//!
//! §3 splits two verbs: **`build`** (source → bundle, CI-gated, carries the
//! bundler) and **`revalidate`** (data → SSG output *on the already-built
//! bundle*, no recompilation). This module is the revalidate half: on an
//! invalidation poke it re-runs the render path against fresh data and
//! republishes to the CDN. Per §4 the receiver is "a route mesofact mounts,"
//! not its own service binary — so it ships as a **mode of `mesofact serve`**
//! (`mesofact serve <workload> --revalidate`), not a separate executable.
//!
//! ## Why it is ephemeral (the memory-footprint property)
//!
//! Unlike `mesofact serve`'s SSR-serving mode — which boots a **resident** V8
//! isolate and holds it for the process lifetime — the receiver spins V8 up
//! **per poke** and drops it (`render_route_all` calls `SsgRuntime::start()`
//! then discards it). Resident cost is just axum + config; V8 memory is spent
//! only while a re-render is actively running. One receiver node can therefore
//! back many static sites without holding one isolate per site.
//!
//! ## Bundler-free (W225 §3)
//!
//! `serve` must not link the bundler. The render half comes from the
//! bundler-free `mesofact-render` crate (extracted from `mesofact-build` for
//! exactly this reason, R535-T9); the publish half from `mesofact-publisher`.
//! Neither pulls `rolldown` / `lightningcss`.
//!
//! ## Scope (single-tenant, v1)
//!
//! The receiver serves **one** workload directory, matching what
//! `runner.yah.dev` actually runs today (almanac-serve's single `ALMANAC_DIR`
//! shape). The optional `mirror_key` bearer is ported from
//! `almanac::receiver` as the cross-mirror-pollution guard. A multi-tenant
//! `tenants/<id>.toml` registry — which finally settles the long-open
//! R330-F12 config format — is a follow-up; the ephemeral-V8 property is
//! identical either way.
//!
//! ## Payload-carrying pokes (yah R330-F33)
//!
//! Getting *fresh data onto disk* (the almanac feed-fetch: a release manifest →
//! `data/*.json`) is an upstream **trigger** that plugs into the seam and then
//! pokes this receiver (§3a "domain-triggered invalidation"). Producing that
//! data is still out of scope here — but *receiving* it is not.
//!
//! A poke may carry the render inputs it wants used ([`DataInputs`]); the
//! receiver writes them into the workload before rendering. This exists because
//! the inputs used to be node-local while the output is global: with several
//! instances behind one hostname, whichever one serviced a poke published *its
//! own* copy of the data to the shared bucket, so a poke landing on an instance
//! whose feed sidecar had not yet ticked would overwrite fresher output with
//! staler — silently, since last write wins and nothing errors. A poke that
//! carries its data can be serviced by any instance with identical results, so
//! routing becomes an optimisation rather than a correctness input.
//!
//! A poke with no `data_inputs` is still valid and still means "re-render from
//! whatever is on disk" — that is what a whole-site poke, a manual curl, and a
//! genuinely poll-driven feed all send.
//!
//! @yah:relay(R446, "mesofact-serve --revalidate: multi-tenant tenants/&lt;id&gt;.toml registry (R330-F12 receiver re-home)")
//! @yah:status(review)
//! @yah:at(2026-08-13T19:10:19Z)
//! @yah:assignee(agent:bundle-anthropic-ashguard)
//! @yah:gotcha("COORDINATE revalidate.rs edits with Glimmerstone (chat, sigil g-polar-star) — they are live in the mesofact tree with in-flight fixes: per-extension Content-Type in object-store r2.rs put/publish (landed, uncommitted) + the clean-URL extensionless->.html router fix (mesofact R443-B4). Those are general infra; this relay must not duplicate or collide with them. Sync before substantive revalidate.rs edits.")
//! @yah:gotcha("/releases is a STATIC prerender (releases.html) re-rendered from releases.json on revalidate — NOT a serveInstance/pointer route (W059 §3 'materialisation = build-time static, style a'). The registry routes pokes to render+publish; it does not add per-request dynamic serving.")
//! @yah:next("DESIGN (boundary decision): keep the tenant registry MESOFACT-NATIVE. Do NOT deref yah's .yah/services/<svc>/mirrors/<env>.toml inside mesofact — that couples an independently-exportable workspace to yah's config schema, and PublishConfig (mesofact-publisher) is deliberately yah-agnostic (env-named creds, no yah types). tenants/<id>.toml entry = { id, mirror_key (or *_env name), workload (dir containing dist/), publish_config (path to that tenant's mesofact.config.toml [publish]), routes? (optional allowlist) }. Registry maps mirror_key -> tenant -> (workload, publish_config) — a clean generalization of today's single-tenant RevalidateConfig. Glimmerstone's 'thin deref / compose provider ref' goal is RIGHT but belongs on the YAH side: a yah reconciler generates each tenant's mesofact.config.toml from the mirror toml (no such generator exists yet — separate yah-side ticket under R330-F12's producer track).")
//! @yah:next("IMPL: add a TenantRegistry (tenants/<id>.toml parse/load: sorted, missing-dir=empty, id==stem invariant) + a multi-tenant router that routes {route, mirror_key} through it — bearer matches no tenant -> 403; tenant doesn't serve route (allowlist) -> 404; match -> revalidate_once(tenant.workload, tenant.publish_config, route). serve.rs bin: add --tenants <dir> mode, mutually exclusive with single-tenant --workload/--publish-config. Unit-test routing with a fake render/publish callback (mirror revalidate.rs's existing serve_receiver_on split) — no V8, no network.")
//! @yah:next("RE-HOME CONTEXT: receiver half of yah-root R330-F12. almanac-serve BINARY retired for mesofact-serve --revalidate (W225 §3/§4). The gh-releases FETCH -> releases.json is the upstream PRODUCER (yubaba almanac, landed) and is OUT of this receiver's scope — it pokes this receiver after writing fresh data. F11 runner hosts mesofact-serve --revalidate, not almanac-serve.")
//! @yah:assumes("DIVERGENCE flagged to Glimmerstone: they suggested a thinner shape (tenant = {service, env, data_inputs}, deref the yah mirror toml for bucket/prefix/zone/provider). Overriding to mesofact-native on the export-boundary rationale above. The routing CORE (mirror_key->tenant, 403/404, revalidate_once dispatch) is invariant across both shapes; only the config-source detail differs. Awaiting their ack/objection before finalizing field names, but not blocked on it — routing can land first behind the config seam.")
//! @yah:assumes("data_inputs do NOT belong in the tenant registry: route<-data bindings already live in the mesofact manifest.json (RenderRequest.data), and the data SOURCE (gh-releases fetch) is the producer's concern, out of the receiver's scope.")
//! @yah:handoff("LANDED (code-complete, mesofact-dev, feature=ssr): new crate::tenants module + --tenants CLI mode. (1) tenants.rs: TenantFile (id/workload/publish_config/mirror_key_env from tenants/<id>.toml) -> ResolvedTenant (bearer resolved) -> TenantRegistry.tenant_for(mirror_key) routing; TenantJob{tenant_id,workload,publish_config,route}; load_tenants(dir) (sorted, missing-dir=empty, id==stem fail-loud) + resolve_tenants(files, env-lookup closure) (bearer via mirror_key_env, never a literal secret in git); axum router (POST /revalidate {route,mirror_key} -> bearer selects tenant -> enqueue TenantJob -> 202; absent/empty/unknown bearer -> 403) + serve() draining TenantJob through the EXISTING crate::revalidate::revalidate_once (render+publish unchanged, only multiplied). (2) lib.rs: pub mod tenants (ssr). (3) serve.rs bin: --tenants <dir> mode; workload now optional; mutually exclusive with single-tenant --workload/--publish-config. Boundary held: a tenant references its OWN mesofact.config.toml, NOT yah's mirror toml. Tests: 11 new (registry routing incl. unroutable-without-bearer; HTTP 202/403 + whole-site None-route; load sorted/missing-dir/stem-mismatch; resolve env present/absent). mesofact-dev 76->87 green; clippy clean on tenants.rs/serve.rs.")
//! @yah:handoff("REMAINING (not code in this crate): (a) YAH-SIDE generator — a yah reconciler emits each tenant's mesofact.config.toml [publish] from .yah/services/<svc>/mirrors/<env>.toml (Glimmerstone's 'thin deref' goal, kept on the yah side to preserve the export boundary); file under R330-F12's producer track. (b) DEPLOY: F11 runner hosts `mesofact-serve --tenants <dir>` (not almanac-serve), with tenants/<id>.toml + the mirror_key_env bearers set. (c) SMOKE: POST runner /revalidate {route:'/releases', mirror_key:'<yah-marketing bearer>'} -> renders+publishes to yah-marketing's R2. (d) Glimmerstone ack on the mesofact-native shape (divergence flagged; routing core is shape-invariant either way).")
//! @yah:verify("cargo test -p mesofact --features ssr tenants::  # 22 pass (registry routing, per-tenant allowlist, validate, load/resolve). NOTE: crate is `mesofact`, not `mesofact-dev` — the serving engine moved here in W225 §2a; the old mesofact-dev verify lines were stale.")
//! @yah:verify("cargo test -p mesofact --features ssr  # 131 lib + 5 integration pass")
//! @yah:verify("cargo clippy -p mesofact --features ssr --all-targets  # clean")
//! @yah:verify("LIVE SMOKE (done, no infra needed): mesofact serve --revalidate --tenants <dir> --listen 127.0.0.1:38446 with two tenant tomls; /releases+mkt-bearer 202, /pricing+mkt-bearer 403 (outside that tenant's allowlist), /pricing+acme-bearer 202 (acme declares none), wrong/absent bearer 403, whole-site poke 202, escaping data_inputs 400. Renders then fail on the absent fixture workload — which is the proof the worker reached revalidate_once per tenant.")
//! @yah:verify("CLI guards on the real binary: `--tenants X <workload>` -> clap conflict error; empty --tenants dir -> refuses to boot; two tenants sharing a bearer -> refuses to boot naming the ids (never the bearer).")
//! @yah:handoff("R446 receiver half is COMPLETE in-crate. This session closed the last modelled gap: the per-tenant `routes` allowlist that tenants.rs carried as a `&[]` TODO at the revalidate_once call. TenantFile.routes / ResolvedTenant.routes / TenantJob.allow now thread it end-to-end, enforced in the SAME two places as the single-tenant receiver (yah R752-B7): an explicit out-of-list route is refused 403 in the handler, a whole-site poke (route:None) is NARROWED by the worker. 403 not 404 deliberately, matching revalidate.rs:474 — the route may exist, the caller lacks authority over it.")
//! @yah:verify("Hardening found while implementing (all in R446's own files, all tested): (1) TenantRegistry::validate() — two tenants resolving to the same bearer meant tenant_for() silently gave every poke to the first, i.e. rendering one tenant's workload into the other's bucket with a 202 on the wire. Now refuses at boot, naming ids and never the bearer. (2) --tenants now clap-conflicts with workload/--publish-config/--allow-route instead of silently winning — a silently-ignored --allow-route is an allowlist an operator believes is enforced. --mirror-key only warns (it carries env=MESOFACT_MIRROR_KEY, which a runner may set process-wide). (3) An empty/missing --tenants dir refuses to boot: it produced a receiver that 403s everything while passing /readyz — the yah R330-T35 silent-failure shape. load_tenants keeps missing-dir=empty (library contract, tested); the CLI is where it becomes fatal. (4) serde(deny_unknown_fields) on TenantFile: `route` for `routes` would have parsed clean and yielded an unscoped tenant. (5) Per-tenant startup log lines (workload, publish_config, routable, allowed_routes).")
//! @yah:gotcha("Verify lines that referenced `-p mesofact-dev` were STALE and are corrected: the serving engine (revalidate + tenants) moved into `crates/mesofact` in W225 §2a. Test with `-p mesofact --features ssr`.")
//! @yah:gotcha("Working tree is DIRTY and uncommitted by design — tenants.rs + cli/serve.rs. No git write was made (not requested).")
//! @yah:assumes("Glimmerstone (g-polar-star) is no longer in this camp, so the flagged mesofact-native-vs-thin-deref divergence never got an explicit ack. The mesofact-native shape is what shipped and is now tested end-to-end; the routing core is shape-invariant either way, so a later thin-deref would change only where publish_config comes from, not the registry.")

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    routing::post,
    Json, Router,
};
use mesofact_core::manifest::{Manifest, RouteMode};
use mesofact_publisher::{
    publish_dist, CloudflareCdnPurger, PublishConfig, PublishReport, S3Store,
};
use mesofact_render::render::{render_route_all_with, RenderAllOptions};
use mesofact_render::js::SsgRuntime;
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

/// Runtime configuration for the receiver. Built by the `mesofact serve`
/// binary from CLI flags / env.
#[derive(Debug, Clone)]
pub struct RevalidateConfig {
    /// Workload directory — the parent of `dist/` (with `dist/manifest.json`).
    pub workload: PathBuf,
    /// `mesofact.config.toml` carrying the `[publish]` block (bucket / zone /
    /// env-named credentials). Resolved lazily per poke so the process can
    /// start before creds are present.
    pub publish_config: PathBuf,
    /// Optional shared bearer secret. When `Some`, a poke must carry the same
    /// `mirror_key` or it is rejected 403 — the cross-mirror-pollution guard
    /// ported from `almanac::receiver` (R335-F2). `None` = open receiver.
    pub mirror_key: Option<String>,
    /// Route allowlist. Empty = every render-eligible route in the manifest.
    ///
    /// This is scoping, NOT authentication — the two are orthogonal and a
    /// deployment may set either, both, or neither (yah R752-B7). A receiver
    /// with an allowlist re-renders exactly these routes and nothing else, so a
    /// caller who holds the bearer still cannot reach a route the deployment
    /// never declared.
    ///
    /// Enforced in two places because a poke has two shapes: an explicit
    /// `{"route": …}` outside the list is refused, and a whole-site poke
    /// (`route` absent) is NARROWED to the list rather than refused — "empty =
    /// all routes" then reads the same way from both ends.
    pub routes: Vec<String>,
}

/// Render inputs carried by a poke: the **workload-relative** path a route
/// declares in its `data_inputs` → the JSON that path should hold.
///
/// Keyed exactly as the manifest declares the input (`src/data/releases.json`),
/// because that is the string both the producer and
/// [`mesofact_render`]'s `read_data_inputs` already use — the receiver needs no
/// knowledge of the producer's workspace layout to place the bytes.
pub type DataInputs = BTreeMap<String, serde_json::Value>;

/// Check every key in a poke's payload is a path that stays inside the
/// workload. Returns the offending key on the first violation.
///
/// A poke is remote input, so an unchecked key is an arbitrary-file write:
/// `/etc/x`, `../../secrets.json` and a bare `` would all escape the workload
/// the bearer authorizes. Only plain relative components are accepted — no
/// root, no prefix, no `..`, no `.`.
pub fn check_data_input_paths(inputs: &DataInputs) -> Result<(), String> {
    for key in inputs.keys() {
        if key.is_empty() {
            return Err("empty data_inputs path".to_string());
        }
        let all_normal = Path::new(key)
            .components()
            .all(|c| matches!(c, Component::Normal(_)));
        if !all_normal {
            return Err(format!(
                "data_inputs path {key:?} must be relative to the workload with no '..' segments"
            ));
        }
    }
    Ok(())
}

/// Write a poke's carried inputs into the workload, replacing whatever this
/// node's own feed sidecar last left there.
///
/// Overwriting is the point: after this, the render reads the poke's data
/// rather than node-local state, so every instance produces the same output for
/// the same poke. It also makes the node self-healing — a node that was down
/// for three releases is correct on the first poke it receives, without waiting
/// for its sidecar to catch up.
///
/// Written with `to_string_pretty`, matching the producer's own serialization.
/// The bytes are not guaranteed identical to the producer's (a JSON object
/// round-trips with sorted keys, a struct serializes in declaration order), but
/// every consumer of these files parses them, and the sidecar's own
/// change-detection compares parsed values — so a key reorder cannot manufacture
/// a phantom change.
pub async fn apply_data_inputs(workload: &Path, inputs: &DataInputs) -> Result<()> {
    check_data_input_paths(inputs).map_err(|e| anyhow::anyhow!(e))?;
    for (rel, value) in inputs {
        let abs = workload.join(rel);
        if let Some(parent) = abs.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("revalidate: creating {} for data input", parent.display()))?;
        }
        let body = serde_json::to_string_pretty(value)
            .with_context(|| format!("revalidate: serializing data input {rel}"))?;
        tokio::fs::write(&abs, body)
            .await
            .with_context(|| format!("revalidate: writing data input {}", abs.display()))?;
        info!(path = %abs.display(), "revalidate: applied carried data input");
    }
    Ok(())
}

/// Outcome of one revalidation cycle.
#[derive(Debug)]
pub struct RevalidateReport {
    /// Route patterns that were re-rendered.
    pub rendered_routes: Vec<String>,
    /// Total instances written across those routes.
    pub instances: usize,
    /// The publish leg's report (uploaded / skipped keys, purged tags).
    pub publish: PublishReport,
}

/// One full revalidation cycle: apply the poke's carried inputs, render
/// (ephemeral V8, off the async runtime), then publish. `route`: `Some` → that
/// route only; `None` → every render-eligible route in the manifest (all
/// `static`/`spa`, non-`deferred`).
///
/// `allow` is the deployment's route allowlist ([`RevalidateConfig::routes`]);
/// empty = unrestricted. It is applied HERE, not only at the HTTP edge, so the
/// scope holds for every caller of this function — the receiver, the
/// multi-tenant router, and anything that drives a revalidate in-process. A
/// disallowed explicit route is an error rather than a silent no-op, because a
/// poke that renders nothing and reports success is the failure mode that made
/// the un-enforced allowlist invisible for as long as it was (yah R752-B7).
///
/// `data_inputs` is applied *inside* this function rather than by each caller so
/// no receiver can forget it and quietly go back to rendering node-local state.
/// An empty map is the payload-less case and touches nothing.
pub async fn revalidate_once(
    workload: &Path,
    publish_config: &Path,
    route: Option<String>,
    data_inputs: &DataInputs,
    allow: &[String],
) -> Result<RevalidateReport> {
    // Land the carried data before rendering — this is what makes the output
    // independent of which instance serviced the poke.
    apply_data_inputs(workload, data_inputs).await?;

    // Render is synchronous and V8 is `!Send`, so it runs on a blocking thread
    // — booting and dropping its own isolate (the ephemeral property).
    let workload_owned = workload.to_path_buf();
    let allow_owned = allow.to_vec();
    let (rendered_routes, instances) =
        tokio::task::spawn_blocking(move || render_routes(&workload_owned, route, &allow_owned))
            .await
            .context("revalidate: render task panicked")??;

    let publish = publish_built(workload, publish_config).await?;
    Ok(RevalidateReport { rendered_routes, instances, publish })
}

/// Render half — boots one `SsgRuntime`, renders every instance of each
/// target route, and writes them into `dist/`. Synchronous (V8 is `!Send`).
///
/// `allow` empty = unrestricted; otherwise an explicit route must be in it and
/// a whole-site poke is intersected with it.
fn render_routes(
    workload: &Path,
    route: Option<String>,
    allow: &[String],
) -> Result<(Vec<String>, usize)> {
    let routes = render_targets(workload, route, allow)?;

    let ssg = SsgRuntime::start().context("revalidate: booting SsgRuntime")?;
    let mut instances = 0usize;
    let mut rendered = Vec::with_capacity(routes.len());
    for r in routes {
        let outcomes = render_route_all_with(
            &ssg,
            RenderAllOptions { project_root: workload.to_path_buf(), out_dir: None, route: r.clone() },
        )
        .with_context(|| format!("revalidate: rendering route {r}"))?;
        instances += outcomes.len();
        rendered.push(r);
    }
    Ok((rendered, instances))
}

/// Which routes one poke resolves to, before any rendering happens.
///
/// Split out of [`render_routes`] so the scoping rule is testable without
/// booting V8 — the allowlist is a security-shaped control and "it compiles"
/// is not evidence it holds.
fn render_targets(workload: &Path, route: Option<String>, allow: &[String]) -> Result<Vec<String>> {
    match route {
        Some(r) => {
            if !allow.is_empty() && !allow.iter().any(|a| a == &r) {
                anyhow::bail!(
                    "revalidate: route {r} is not in this receiver's allowlist ({})",
                    allow.join(", ")
                );
            }
            Ok(vec![r])
        }
        // Intersect rather than union: the allowlist may name a route the
        // manifest doesn't render (an `ssr` or `deferred` one, or a typo), and
        // rendering by name alone would turn that into a per-poke error.
        None => {
            let eligible = eligible_routes(workload)?;
            Ok(if allow.is_empty() {
                eligible
            } else {
                eligible.into_iter().filter(|r| allow.iter().any(|a| a == r)).collect()
            })
        }
    }
}

/// The render-eligible routes for a whole-site poke: everything except `ssr`
/// (rendered per-request in the SSR host, not here) and `deferred` (instances
/// minted at publish time, not enumerable). Mirrors `render_route_all_with`'s
/// own rejections so a whole-site poke skips them instead of erroring.
fn eligible_routes(workload: &Path) -> Result<Vec<String>> {
    let manifest_path = workload.join("dist").join("manifest.json");
    let manifest: Manifest = serde_json::from_str(
        &std::fs::read_to_string(&manifest_path).with_context(|| {
            format!("reading {} — run `mesofact-build build` first", manifest_path.display())
        })?,
    )
    .with_context(|| format!("parsing {}", manifest_path.display()))?;

    Ok(manifest
        .routes
        .into_iter()
        .filter(|r| {
            r.mode != RouteMode::Ssr
                && !r.prerender.as_ref().map(|p| p.is_deferred()).unwrap_or(false)
        })
        .map(|r| r.route)
        .collect())
}

/// Publish half — reuse the exact `mesofact publish` construction path:
/// load `[publish]`, resolve env creds, build the S3 + Cloudflare adapters,
/// and run the idempotent `publish_dist` (content-hash skip + tag purge).
async fn publish_built(workload: &Path, config_path: &Path) -> Result<PublishReport> {
    let cfg = PublishConfig::load(config_path)
        .await
        .with_context(|| format!("revalidate: loading [publish] from {}", config_path.display()))?;
    let creds = cfg.resolve_credentials().context("revalidate: resolving publish credentials")?;
    let store = S3Store::new(
        &cfg.endpoint,
        &cfg.bucket,
        &cfg.region,
        &creds.access_key_id,
        &creds.secret_access_key,
    )
    .context("revalidate: S3 store init")?
    .with_base_prefix(cfg.prefix.clone().unwrap_or_default());
    let purger = CloudflareCdnPurger::new(&cfg.zone_id, &creds.cloudflare_api_token)
        .context("revalidate: Cloudflare purger init")?;
    let report = publish_dist(&workload.join("dist"), &store, &purger)
        .await
        .context("revalidate: publish_dist")?;
    Ok(report)
}

// ── HTTP receiver ────────────────────────────────────────────────────────────

/// A validated poke handed from the HTTP handler to the render/publish worker.
#[derive(Debug, Clone, Default, PartialEq)]
struct Job {
    /// The route to revalidate; `None` = every render-eligible route.
    route: Option<String>,
    /// Render inputs the poke handed over; empty = render from disk.
    data_inputs: DataInputs,
}

#[derive(Clone)]
struct ReceiverState {
    tx: mpsc::Sender<Job>,
    /// When `Some`, a poke must carry a matching `mirror_key` or gets 403.
    mirror_key: Option<String>,
    /// Route allowlist; empty = unrestricted. See [`RevalidateConfig::routes`].
    routes: Vec<String>,
}

#[derive(Deserialize)]
struct RevalidateBody {
    /// Route pattern to revalidate, e.g. `/releases`. Omit to revalidate every
    /// render-eligible route in the manifest.
    #[serde(default)]
    route: Option<String>,
    /// Caller's mirror identity token; must match the receiver's configured
    /// `mirror_key` when one is set.
    #[serde(default)]
    mirror_key: Option<String>,
    /// Render inputs the poke carries, keyed by workload-relative path. Absent
    /// → the receiver renders from what is already on disk, which is the
    /// pre-R330-F33 contract and stays supported.
    #[serde(default)]
    data_inputs: DataInputs,
}

/// Largest `POST /revalidate` body this receiver accepts, in bytes.
///
/// **Stated, not inherited.** Until yah R330-F38 this was axum's built-in
/// `DefaultBodyLimit` — 2 MiB, chosen by the framework, named nowhere and
/// pinned by no test, so nobody could tell whether it was a decision or an
/// accident. That was fine while a poke carried a single release. It stopped
/// being fine when yah's `releases` feed became an *accumulating* index whose
/// payload grows once per release forever: the limit is now something this
/// system will actually reach, so it is a number with a reason next to it.
///
/// 4 MiB is **twice** the sender's own ceiling (`MAX_POKE_PAYLOAD_BYTES` in
/// yah's `almanac::fetch`). Sender and receiver are separately deployed, so
/// sizing both at the same number means any version skew turns a payload the
/// sender thought was fine into a 413. The sender refuses first, locally, where
/// it can log a reason and degrade to a payload-less poke; this limit is the
/// backstop for anything else that POSTs here.
///
/// A body over it gets 413 from axum before the handler runs — which is the
/// right answer for an unbounded stranger, and one a legitimate almanac sender
/// should never see.
pub const MAX_REVALIDATE_BODY_BYTES: usize = 4 * 1024 * 1024;

/// Build the receiver router: `POST /dawn` (enqueue) + the [`crate::health`]
/// probes. Decoupled from the render/publish worker via `tx` so it is
/// unit-testable without V8 or a network publish — the same split
/// `almanac::serve::serve_receiver_on` uses.
///
/// Readiness is unconditional here: this receiver holds no isolate and serves
/// no tree, so once the router is mounted it can accept every request it
/// claims to. The one state that must fail `/readyz` — draining — is owned by
/// [`serve`], which is why it builds its own handle and calls
/// [`router_with_health`] directly. That leaves this a test-only convenience.
#[cfg(test)]
fn router(tx: mpsc::Sender<Job>, mirror_key: Option<String>, routes: Vec<String>) -> Router {
    router_with_health(tx, mirror_key, routes, crate::Health::ready())
}

fn router_with_health(
    tx: mpsc::Sender<Job>,
    mirror_key: Option<String>,
    routes: Vec<String>,
    health: Arc<crate::Health>,
) -> Router {
    Router::new()
        // `POST /dawn` — "this page is stale, re-render it from these bytes"
        // (yah R752-T10). The sibling stage, "the input changed, go re-fetch
        // this feed", is `POST /freshen` on almanac's receiver. Both used to be
        // `/revalidate`, told apart only by which key the body carried, and
        // because this receiver's `RevalidateBody` is `#[serde(default)]` with
        // no `deny_unknown_fields`, a feed-shaped body sent here parsed fine and
        // silently meant *re-render the whole site*.
        .route("/dawn", post(revalidate_handler))
        // Transitional alias, unlike almanac's side which took a clean break:
        // this path IS deployed (kamaji forks this receiver on the tenant's
        // node) and its callers are separately-rolled units, so removing it in
        // the same change would break every poke until both sides rolled.
        // Retire it once no node predates R752-T10.
        .route("/revalidate", post(revalidate_handler))
        // Explicit rather than axum's 2 MiB default — see
        // [`MAX_REVALIDATE_BODY_BYTES`] for why this stopped being a framework
        // detail once pokes started carrying an accumulating history.
        .layer(DefaultBodyLimit::max(MAX_REVALIDATE_BODY_BYTES))
        .with_state(ReceiverState { tx, mirror_key, routes })
        // After `with_state`, because the probes carry their own state — see
        // the same note in `Server::router`. Also leaves them outside the body
        // limit above, which is moot for a bodyless GET.
        .merge(crate::health::probe_routes(health))
}

async fn revalidate_handler(
    State(state): State<ReceiverState>,
    Json(body): Json<RevalidateBody>,
) -> StatusCode {
    if let Some(ref expected) = state.mirror_key {
        match &body.mirror_key {
            Some(provided) if provided == expected => {}
            _ => {
                warn!("revalidate rejected — mirror_key mismatch (cross-mirror pollution blocked)");
                return StatusCode::FORBIDDEN;
            }
        }
    }

    // Scope check, same reasoning as the payload check below: an out-of-scope
    // poke gets a synchronous refusal, not a 202 that renders nothing. 403
    // rather than 404 — the route may well exist on this site; what the caller
    // lacks is authority over it (yah R752-B7).
    if let Some(ref route) = body.route {
        if !state.routes.is_empty() && !state.routes.iter().any(|a| a == route) {
            warn!(
                route = %route,
                allowed = %state.routes.join(", "),
                "revalidate rejected — route outside this receiver's allowlist",
            );
            return StatusCode::FORBIDDEN;
        }
    }

    // Validate the payload's paths here, not in the worker: a malformed poke
    // deserves a synchronous 400 rather than a 202 followed by a log line no
    // caller ever sees.
    if let Err(e) = check_data_input_paths(&body.data_inputs) {
        warn!(err = %e, "revalidate rejected — bad data_inputs path");
        return StatusCode::BAD_REQUEST;
    }

    let job = Job { route: body.route, data_inputs: body.data_inputs };
    match state.tx.try_send(job) {
        Ok(()) => StatusCode::ACCEPTED,
        Err(mpsc::error::TrySendError::Full(_)) => {
            warn!("revalidate channel full — dropping poke");
            StatusCode::SERVICE_UNAVAILABLE
        }
        Err(mpsc::error::TrySendError::Closed(_)) => StatusCode::SERVICE_UNAVAILABLE,
    }
}

/// Run the receiver: bind `port`, serve the router, and drain pokes through
/// [`revalidate_once`] one at a time (renders are serialized — one V8 boot at
/// a time keeps the footprint bounded). Runs until a hard I/O error.
pub async fn serve(cfg: RevalidateConfig, host: std::net::IpAddr, port: u16) -> Result<()> {
    info!(
        workload = %cfg.workload.display(),
        publish_config = %cfg.publish_config.display(),
        mirror_key = cfg.mirror_key.is_some(),
        // Logged as the resolved list, not a bool: "which routes may this
        // process touch" is the first question when a poke did nothing, and the
        // startup line is where an operator looks for it.
        allowed_routes = %if cfg.routes.is_empty() { "<all>".to_string() } else { cfg.routes.join(", ") },
        "mesofact serve revalidate receiver starting (ephemeral render → publish)",
    );

    let (tx, mut rx) = mpsc::channel::<Job>(16);
    let health = crate::Health::ready();
    let app = router_with_health(tx, cfg.mirror_key.clone(), cfg.routes.clone(), health.clone());

    let workload = cfg.workload.clone();
    let publish_config = cfg.publish_config.clone();
    let allow = cfg.routes.clone();
    tokio::spawn(async move {
        while let Some(Job { route, data_inputs }) = rx.recv().await {
            info!(
                route = ?route,
                carried_inputs = data_inputs.len(),
                "revalidate poke accepted"
            );
            match revalidate_once(&workload, &publish_config, route.clone(), &data_inputs, &allow)
                .await
            {
                Ok(report) => info!(
                    route = ?route,
                    rendered = ?report.rendered_routes,
                    instances = report.instances,
                    uploaded = report.publish.uploaded_keys.len(),
                    skipped = report.publish.skipped_keys.len(),
                    purged = report.publish.purged_tags.len(),
                    "revalidate complete",
                ),
                Err(e) => error!(route = ?route, err = ?e, "revalidate failed"),
            }
        }
    });

    let addr = SocketAddr::new(host, port);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("revalidate receiver: binding to {addr}"))?;
    info!(%addr, "revalidate receiver listening");
    // Graceful shutdown, where there previously was none: a SIGTERM used to
    // sever in-flight POSTs at the socket, so a sender could not tell a
    // rejected poke from a lost one. This drains the HTTP side only — the
    // render/publish worker below reads from an `mpsc` on its own task and is
    // still cut mid-`revalidate_once`. Draining that too means holding
    // shutdown until `rx` empties, which is a separate change with its own
    // ceiling (a poke queue can be long, and kubelet's grace period is not).
    axum::serve(listener, app)
        .with_graceful_shutdown(crate::shutdown_signal_for(health))
        .await
        .context("revalidate receiver: server error")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request};
    use tower::util::ServiceExt;

    async fn post_json(app: Router, body: &'static str) -> axum::response::Response {
        let req = Request::builder()
            .method(Method::POST)
            .uri("/revalidate")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        app.oneshot(req).await.unwrap()
    }

    #[tokio::test]
    async fn poke_enqueues_route_and_returns_202() {
        let (tx, mut rx) = mpsc::channel::<Job>(4);
        let app = router(tx, None, vec![]);
        let resp = post_json(app, r#"{"route":"/releases"}"#).await;
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        assert_eq!(rx.try_recv().unwrap().route, Some("/releases".to_string()));
    }

    #[tokio::test]
    async fn poke_without_route_enqueues_none_whole_site() {
        let (tx, mut rx) = mpsc::channel::<Job>(4);
        let app = router(tx, None, vec![]);
        let resp = post_json(app, r#"{}"#).await;
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        assert_eq!(rx.try_recv().unwrap().route, None);
    }

    #[tokio::test]
    async fn full_channel_returns_503() {
        let (tx, _rx) = mpsc::channel::<Job>(1);
        tx.try_send(Job { route: Some("already-full".into()), ..Job::default() }).unwrap();
        let app = router(tx, None, vec![]);
        let resp = post_json(app, r#"{"route":"/x"}"#).await;
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn correct_mirror_key_passes() {
        let (tx, mut rx) = mpsc::channel::<Job>(4);
        let app = router(tx, Some("secret-abc".into()), vec![]);
        let resp = post_json(app, r#"{"route":"/r","mirror_key":"secret-abc"}"#).await;
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        assert_eq!(rx.try_recv().unwrap().route, Some("/r".to_string()));
    }

    #[tokio::test]
    async fn wrong_mirror_key_returns_403_and_does_not_enqueue() {
        let (tx, mut rx) = mpsc::channel::<Job>(4);
        let app = router(tx, Some("secret-abc".into()), vec![]);
        let resp = post_json(app, r#"{"route":"/r","mirror_key":"nope"}"#).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert!(rx.try_recv().is_err(), "rejected poke must not enqueue");
    }

    #[tokio::test]
    async fn absent_mirror_key_returns_403_when_configured() {
        let (tx, _rx) = mpsc::channel::<Job>(4);
        let app = router(tx, Some("secret-abc".into()), vec![]);
        let resp = post_json(app, r#"{"route":"/r"}"#).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let (tx, _rx) = mpsc::channel::<Job>(4);
        let app = router(tx, None, vec![]);
        let req = Request::builder()
            .method(Method::GET)
            .uri("/__mesofact/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// Whole-site enumeration filters out `ssr` + `deferred` routes.
    #[tokio::test]
    async fn eligible_routes_filters_ssr_and_deferred() {
        let tmp = tempfile::tempdir().unwrap();
        let dist = tmp.path().join("dist");
        std::fs::create_dir_all(&dist).unwrap();
        let r = |route: &str, mode: &str, extra: &str| {
            format!(
                r#"{{"route":"{route}","mode":"{mode}","render_entrypoint":"e.js","cache_policy":{{"ttl":0}}{extra}}}"#
            )
        };
        let manifest = format!(
            r#"{{"version":"1","build_id":"b1","routes":[{},{},{},{},{}]}}"#,
            r("/", "static", ""),
            r("/releases", "static", ""),
            r("/app", "spa", ""),
            r("/api/x", "ssr", ""),
            r("/c/:id", "static", r#","prerender":{"deferred":true}"#),
        );
        std::fs::write(dist.join("manifest.json"), manifest).unwrap();

        let mut got = eligible_routes(tmp.path()).unwrap();
        got.sort();
        assert_eq!(got, vec!["/", "/app", "/releases"]);
    }

    // ── /dawn, and the alias it replaces (yah R752-T10) ──────────────────────

    async fn post_to(app: Router, path: &str, body: &'static str) -> StatusCode {
        let req = Request::builder()
            .method(Method::POST)
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        app.oneshot(req).await.unwrap().status()
    }

    #[tokio::test]
    async fn dawn_is_the_name_and_revalidate_is_still_served() {
        for path in ["/dawn", "/revalidate"] {
            let (tx, mut rx) = mpsc::channel::<Job>(4);
            let status = post_to(router(tx, None, vec![]), path, r#"{"route":"/r"}"#).await;
            assert_eq!(status, StatusCode::ACCEPTED, "{path} must accept a poke");
            assert_eq!(rx.try_recv().unwrap().route, Some("/r".to_string()));
        }
    }

    /// The alias is a deployment concession, not a second contract: both paths
    /// hit the same handler, so scoping and auth cannot diverge between them.
    #[tokio::test]
    async fn the_legacy_alias_enforces_the_same_allowlist() {
        let (tx, _rx) = mpsc::channel::<Job>(4);
        let status = post_to(
            router(tx, None, vec!["/releases".to_string()]),
            "/revalidate",
            r#"{"route":"/issues"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    // ── Route allowlist (yah R752-B7) ────────────────────────────────────────
    //
    // The allowlist was config-only for its whole life: parsed from the mirror
    // toml, shipped over the wire to the node, and read by nobody. Measured on
    // us-east-001 with `routes = ["/releases"]` declared, a poke for /issues was
    // accepted (202) and republished. These pin both halves of the fix — the
    // synchronous refusal, and the whole-site narrowing that a refusal alone
    // would have missed, since that poke names no route at all.

    fn allow(routes: &[&str]) -> Vec<String> {
        routes.iter().map(|s| (*s).to_string()).collect()
    }

    #[tokio::test]
    async fn route_outside_the_allowlist_is_403_and_does_not_enqueue() {
        let (tx, mut rx) = mpsc::channel::<Job>(4);
        let app = router(tx, None, allow(&["/releases"]));
        let resp = post_json(app, r#"{"route":"/issues"}"#).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert!(rx.try_recv().is_err(), "a refused poke must not reach the renderer");
    }

    #[tokio::test]
    async fn route_inside_the_allowlist_is_accepted() {
        let (tx, mut rx) = mpsc::channel::<Job>(4);
        let app = router(tx, None, allow(&["/releases", "/issues"]));
        let resp = post_json(app, r#"{"route":"/issues"}"#).await;
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        assert_eq!(rx.try_recv().unwrap().route, Some("/issues".to_string()));
    }

    /// The allowlist is scoping, not auth: holding the bearer buys nothing
    /// outside the declared routes.
    #[tokio::test]
    async fn a_correct_mirror_key_does_not_widen_the_allowlist() {
        let (tx, _rx) = mpsc::channel::<Job>(4);
        let app = router(tx, Some("secret-abc".into()), allow(&["/releases"]));
        let resp = post_json(app, r#"{"route":"/issues","mirror_key":"secret-abc"}"#).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    /// An empty allowlist is the documented "all routes" case, unchanged.
    #[tokio::test]
    async fn an_empty_allowlist_accepts_any_route() {
        let (tx, mut rx) = mpsc::channel::<Job>(4);
        let app = router(tx, None, vec![]);
        let resp = post_json(app, r#"{"route":"/anything"}"#).await;
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        assert_eq!(rx.try_recv().unwrap().route, Some("/anything".to_string()));
    }

    /// A whole-site poke names no route, so the handler has nothing to refuse —
    /// this is the shape that was actually measured escaping the allowlist.
    /// It is narrowed at render time instead.
    #[tokio::test]
    async fn a_whole_site_poke_is_accepted_and_narrowed_at_render_time() {
        let (tx, mut rx) = mpsc::channel::<Job>(4);
        let app = router(tx, None, allow(&["/releases"]));
        let resp = post_json(app, r#"{}"#).await;
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        assert_eq!(rx.try_recv().unwrap().route, None, "whole-site pokes stay whole-site here");
    }

    /// Manifest fixture with two static routes; `render_routes` is only asked to
    /// enumerate, never to render (no V8 in unit tests).
    fn workload_with_two_routes() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let dist = tmp.path().join("dist");
        std::fs::create_dir_all(&dist).unwrap();
        let r = |route: &str| {
            format!(
                r#"{{"route":"{route}","mode":"static","render_entrypoint":"e.js","cache_policy":{{"ttl":0}}}}"#
            )
        };
        std::fs::write(
            dist.join("manifest.json"),
            format!(
                r#"{{"version":"1","build_id":"b1","routes":[{},{}]}}"#,
                r("/releases"),
                r("/issues")
            ),
        )
        .unwrap();
        tmp
    }

    #[test]
    fn whole_site_render_targets_are_the_manifest_intersected_with_the_allowlist() {
        let tmp = workload_with_two_routes();
        assert_eq!(
            render_targets(tmp.path(), None, &allow(&["/releases"])).unwrap(),
            vec!["/releases".to_string()],
        );
        let mut all = render_targets(tmp.path(), None, &[]).unwrap();
        all.sort();
        assert_eq!(all, vec!["/issues".to_string(), "/releases".to_string()]);
    }

    /// A route in the allowlist that the manifest cannot render (`ssr`,
    /// `deferred`, or a typo) must not be conjured into the target set.
    #[test]
    fn an_allowlisted_route_absent_from_the_manifest_is_not_rendered() {
        let tmp = workload_with_two_routes();
        assert_eq!(
            render_targets(tmp.path(), None, &allow(&["/releases", "/nope"])).unwrap(),
            vec!["/releases".to_string()],
        );
    }

    /// Defence in depth: the handler refuses first, but a caller reaching
    /// `revalidate_once` in-process gets an error rather than a silent no-op.
    #[test]
    fn an_explicit_disallowed_route_errors_at_render_time_too() {
        let tmp = workload_with_two_routes();
        let err = render_targets(tmp.path(), Some("/issues".into()), &allow(&["/releases"]))
            .expect_err("a disallowed explicit route must not resolve to a target set");
        assert!(err.to_string().contains("allowlist"), "error names the cause: {err}");
    }

    // ── Payload-carrying pokes (yah R330-F33) ────────────────────────────────

    const INPUT: &str = "src/data/releases.json";

    /// Drive one poke through the real HTTP handler, then apply the resulting
    /// job to `workload` exactly as [`serve`]'s worker does immediately before
    /// it renders. Everything downstream of this (render → publish) is a pure
    /// function of the workload's contents, so this is the seam where "which
    /// instance serviced the poke" either does or does not matter.
    async fn receive_and_apply(workload: &Path, body: &'static str) -> StatusCode {
        let (tx, mut rx) = mpsc::channel::<Job>(4);
        let status = post_json(router(tx, None, vec![]), body).await.status();
        if status == StatusCode::ACCEPTED {
            let job = rx.try_recv().expect("an accepted poke is enqueued");
            apply_data_inputs(workload, &job.data_inputs).await.unwrap();
        }
        status
    }

    /// Stand up one instance's workload with its sidecar holding `version`.
    fn instance_with_local_data(version: &str) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(INPUT);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&serde_json::json!({
                "fetched_at": "2026-01-01T00:00:00Z",
                "releases": [{"version": version}],
            }))
            .unwrap(),
        )
        .unwrap();
        tmp
    }

    fn local_data(workload: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(workload.join(INPUT)).unwrap()).unwrap()
    }

    /// **The point of R330-F33.** Two instances whose node-local feed sidecars
    /// have diverged — one never ticked past 0.8.19, one is at 0.8.21 — receive
    /// the same payload-carrying poke. Both end up rendering from identical
    /// inputs, so whichever one the router happened to pick publishes the same
    /// bytes to the shared bucket.
    ///
    /// Before the payload existed, the poke was `{route, mirror_key}` and each
    /// instance rendered from its own copy: a poke landing on the stale
    /// instance published 0.8.19 over 0.8.21, silently.
    #[tokio::test]
    async fn a_poke_to_either_of_two_divergent_instances_renders_identical_input() {
        let stale = instance_with_local_data("0.8.19");
        let fresh = instance_with_local_data("0.8.21");
        assert_ne!(
            local_data(stale.path()),
            local_data(fresh.path()),
            "precondition: the two instances really do disagree",
        );

        // One release lands; the producer pokes whichever instance it reaches.
        const POKE: &str = r#"{"route":"/releases","data_inputs":{"src/data/releases.json":
            {"fetched_at":"2026-07-30T00:00:00Z","releases":[{"version":"0.8.22"}]}}}"#;

        assert_eq!(receive_and_apply(stale.path(), POKE).await, StatusCode::ACCEPTED);
        assert_eq!(receive_and_apply(fresh.path(), POKE).await, StatusCode::ACCEPTED);

        assert_eq!(
            local_data(stale.path()),
            local_data(fresh.path()),
            "the render input must not depend on which instance serviced the poke",
        );
        assert_eq!(
            local_data(stale.path())["releases"][0]["version"],
            "0.8.22",
            "and it must be the data the poke carried, not either node's own",
        );
    }

    /// Back-compat, and the reason the sidecar is not deleted: a payload-less
    /// poke is still accepted, and leaves node-local data exactly as it found
    /// it. That is the whole-site branch, a manual curl, and any genuinely
    /// poll-driven feed.
    #[tokio::test]
    async fn a_payload_less_poke_is_accepted_and_touches_no_data() {
        let node = instance_with_local_data("0.8.19");
        let before = local_data(node.path());

        for body in [r#"{}"#, r#"{"route":"/releases"}"#] {
            let (tx, mut rx) = mpsc::channel::<Job>(4);
            let status = post_json(router(tx, None, vec![]), body).await.status();
            assert_eq!(status, StatusCode::ACCEPTED, "empty body must not be an error");
            let job = rx.try_recv().unwrap();
            assert!(job.data_inputs.is_empty());
            apply_data_inputs(node.path(), &job.data_inputs).await.unwrap();
        }

        assert_eq!(local_data(node.path()), before, "no payload, no write");
    }

    /// A carried input for a file this node has never seen is created, parents
    /// and all — a freshly-provisioned instance is correct on its first poke
    /// without waiting for its own sidecar to tick.
    #[tokio::test]
    async fn a_carried_input_creates_the_file_on_a_cold_instance() {
        let tmp = tempfile::tempdir().unwrap();
        let status = receive_and_apply(
            tmp.path(),
            r#"{"route":"/releases","data_inputs":{"src/data/releases.json":{"releases":[]}}}"#,
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(local_data(tmp.path()), serde_json::json!({"releases": []}));
    }

    /// A poke is remote input, so its paths are checked before anything is
    /// written: an escaping key is a 400 and never reaches the worker.
    #[tokio::test]
    async fn an_escaping_data_input_path_returns_400_and_enqueues_nothing() {
        for body in [
            r#"{"data_inputs":{"../../etc/passwd":{}}}"#,
            r#"{"data_inputs":{"/etc/passwd":{}}}"#,
            r#"{"data_inputs":{"":{}}}"#,
        ] {
            let (tx, mut rx) = mpsc::channel::<Job>(4);
            let resp = post_json(router(tx, None, vec![]), body).await;
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "body: {body}");
            assert!(rx.try_recv().is_err(), "a rejected poke must not enqueue");
        }
    }

    // ── The body limit is stated and pinned (yah R330-F38) ───────────────────

    async fn post_owned(app: Router, body: String) -> axum::response::Response {
        let req = Request::builder()
            .method(Method::POST)
            .uri("/revalidate")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        app.oneshot(req).await.unwrap()
    }

    /// A poke whose `data_inputs` serialize to `payload` bytes of filler.
    fn poke_of_size(payload: usize) -> String {
        format!(
            r#"{{"route":"/releases","data_inputs":{{"src/data/releases.json":{{"filler":"{}"}}}}}}"#,
            "x".repeat(payload)
        )
    }

    /// The limit is a decision now, so it is asserted. Before this it was
    /// axum's built-in `DefaultBodyLimit` — a framework default nothing in
    /// either tree named or tested, which is exactly how a ceiling goes
    /// unnoticed until the thing that reaches it ships.
    ///
    /// Both directions matter. Under the limit must be ACCEPTED, or an
    /// accumulating release history silently stops rendering the day it crosses
    /// whatever the framework happened to pick; over it must be 413, or the
    /// receiver has no backstop at all.
    #[tokio::test]
    async fn the_revalidate_body_limit_is_explicit_and_enforced() {
        // Comfortably inside: bigger than axum's 2 MiB default, so this cell
        // FAILS if the explicit layer is ever removed and the default returns.
        let (tx, mut rx) = mpsc::channel::<Job>(4);
        let big = poke_of_size(3 * 1024 * 1024);
        assert!(big.len() < MAX_REVALIDATE_BODY_BYTES);
        assert!(big.len() > 2 * 1024 * 1024, "must exceed the inherited default it replaced");
        let resp = post_owned(router(tx, None, vec![]), big).await;
        assert_eq!(
            resp.status(),
            StatusCode::ACCEPTED,
            "a body under the stated limit must be accepted — if this is 413, the \
             explicit DefaultBodyLimit layer was dropped and axum's 2 MiB default is back"
        );
        assert!(rx.try_recv().is_ok());

        // Over: rejected by the layer before the handler runs.
        let (tx, mut rx) = mpsc::channel::<Job>(4);
        let resp = post_owned(router(tx, None, vec![]), poke_of_size(MAX_REVALIDATE_BODY_BYTES)).await;
        assert_eq!(
            resp.status(),
            StatusCode::PAYLOAD_TOO_LARGE,
            "an oversize body is refused, not truncated"
        );
        assert!(rx.try_recv().is_err(), "a rejected poke must not enqueue");
    }

    /// The margin against the sender is deliberate: yah's
    /// `almanac::fetch::MAX_POKE_PAYLOAD_BYTES` is 2 MiB and degrades to a
    /// payload-less poke above that, so a legitimate sender always refuses
    /// before this receiver would. Sizing them equally would turn any version
    /// skew between separately-deployed binaries into a 413.
    #[test]
    fn the_receiver_limit_leaves_headroom_over_the_sender_ceiling() {
        const ALMANAC_SENDER_CEILING: usize = 2 * 1024 * 1024;
        assert!(
            MAX_REVALIDATE_BODY_BYTES >= 2 * ALMANAC_SENDER_CEILING,
            "the receiver must stay well above the sender's ceiling"
        );
    }

    #[test]
    fn path_check_accepts_plain_relative_paths_only() {
        let ok = DataInputs::from([("src/data/releases.json".into(), serde_json::json!({}))]);
        assert!(check_data_input_paths(&ok).is_ok());

        for bad in ["../x.json", "a/../../x.json", "/abs.json", "./x.json", ""] {
            let inputs = DataInputs::from([(bad.to_string(), serde_json::json!({}))]);
            assert!(
                check_data_input_paths(&inputs).is_err(),
                "{bad:?} must be rejected"
            );
        }
    }

    /// A bearer-protected receiver checks the bearer *before* the payload — a
    /// rejected caller must not get to write files.
    #[tokio::test]
    async fn a_wrong_bearer_is_rejected_even_with_a_valid_payload() {
        let node = instance_with_local_data("0.8.19");
        let before = local_data(node.path());
        let (tx, mut rx) = mpsc::channel::<Job>(4);
        let resp = post_json(
            router(tx, Some("secret-abc".into()), vec![]),
            r#"{"route":"/releases","mirror_key":"nope","data_inputs":{"src/data/releases.json":
                {"releases":[{"version":"9.9.9"}]}}}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert!(rx.try_recv().is_err());
        assert_eq!(local_data(node.path()), before, "403 must write nothing");
    }
}
