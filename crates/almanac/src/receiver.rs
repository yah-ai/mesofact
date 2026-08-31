//! @yah:ticket(R335-F3, "Mirror-aware /revalidate receiver: reject feeds not bound to this mirror")
//! @yah:assignee(bundle-anthropic-miravel)
//! @yah:at(2026-05-27T07:09:31Z)
//! @yah:status(review)
//! @yah:phase(P1)
//! @yah:parent(R335)
//! @yah:next("Construct the receiver with its own mirror identity (env + service id).")
//! @yah:next("On revalidate: load the feed, reject (4xx) unless on_change.service resolves to a mirror binding matching the receiver's own env.")
//! @yah:next("Lands with R330-F4 (wires receiver into the reconciler). Satisfies R335-T2's negative case WITHOUT auth.")
//!
//! @yah:ticket(R335-F5, "Per-mirror capability gate on /revalidate (yubaba/xlb-net node identity)")
//! @yah:assignee(agent:claude)
//! @yah:at(2026-05-27T02:39:12Z)
//! @yah:status(review)
//! @yah:phase(P3)
//! @yah:parent(R335)
//! @yah:next("Design captured in .yah/docs/working/W058-almanac-mirror-binding.md §12. Implementation gated on yubaba control plane (authorized-signer-set provisioning + rotation) — file a fresh impl ticket when yubaba lands.")
//! @yah:next("Near-term mechanism = mode A (signed request over the existing HTTP receiver, asymmetric, request-bound); converges to mode C (xlb-net authenticated transport, verified peer NodeId) for cross-machine cloud/ha. mode B (macaroon) reserved for delegation/attenuation only.")
//! @yah:next("When buildable: add authorized_signers (from .yah/services/<id>/mirrors/<env>.toml) to MirrorBind; replace the mirror_key body field with {signer,nonce,expiry,sig}; 401 invalid sig / 403 signer-not-authorized; nonce+expiry freshness cache.")
//! @yah:next("Bootstrap = operator key seeded into the cloud receiver's signer set + revalidate port on ExposeSpec.operator (Tailscale tag) — mirror-sage.")
//! @yah:handoff("Design-only deliverable complete. §12 of .yah/docs/working/W058-almanac-mirror-binding.md specifies the per-mirror /revalidate capability gate: (1) why mirror_key is insufficient (static symmetric bearer secret, replayable, identity-blind, no freshness); (2) identity grain = yubaba/xlb-net Ed25519 node identity (yubaba/src/identity.rs hostkey + iroh NodeId), infra not cheers; (3) mechanism options A signed-request / B macaroon / C authenticated-transport with recommendation (A near-term on the existing HTTP receiver, converging to C for cross-machine cloud/ha); (4) authorized-signer set declared in .yah/services/<id>/mirrors/<env>.toml, composing with OnChangeConfig.service (not a new AlmanacManifest field); (5) operator-bridge bootstrap riding ExposeSpec.operator Tailscale tag (mirror-sage); (6) receiver-shape sketch + status codes. receiver.rs MirrorBind.env and mirror_key doc comments now point at §12. NO mechanism built — implementation gated on yubaba's control plane.")
//! @yah:gotcha("F5 SUPERSEDES mirror_key (R335-F2) — it is a replacement, not an additional auth knob. F3's feed-binding check (is-this-feed-mine) is orthogonal to F5 (are-you-allowed-to-ask) and stays.")
//!
//!
//! @yah:ticket(R752-T10, "Two invalidation stages, two verbs: POST /freshen (almanac) and POST /dawn (mesofact) replace the shared /revalidate")
//! @yah:at(2026-08-13T01:17:35Z)
//! @yah:status(review)
//! @yah:assignee(agent:bundle-anthropic-ashguard)
//! @yah:parent(R752)
//! @yah:handoff("Two stages, two verbs, on the wire and not just in prose. FRESHEN = the input changed, go re-fetch this feed: almanac's own receiver now serves POST /freshen (oss/yubaba/crates/almanac/src/receiver.rs). DAWN = the output is stale, re-render this page from these bytes: mesofact's receiver now serves POST /dawn (oss/mesofact/crates/mesofact/src/revalidate.rs, plus the multi-tenant router in tenants.rs).")
//! @yah:handoff("Operator's naming call, verbatim intent: 'refresh' and 'revalidate' are both standard HTTP-caching words and were pulling the two stages back together. 'freshen' is the dairy term - a cow freshens when she calves and starts producing - which is exactly the input stage. 'dawn' beat epoch/era on a check rather than a preference: epoch appears 514 times across this tree (raft, containerd, backups) and era 40, while dawn appears zero times, so it is the only one of the three that cannot be misread as something already in the codebase.")
//! @yah:handoff("Sender picks the path from the poke's own shape (almanac fetch.rs::HttpPoke), so a caller cannot address the wrong stage. Constructors renamed to say the stage rather than the field they set: Poke::route -> Poke::dawn, Poke::feed -> Poke::freshen. Call sites updated in almanac, issue-tracker (crates/yah/issue-tracker/src/main.rs:86) and the CLI. Types followed: RevalidateTx -> FreshenTx, RevalidateBody -> FreshenBody.")
//! @yah:handoff("ASYMMETRIC COMPATIBILITY, deliberately. almanac's /revalidate is GONE with no alias - almanac-serve is deployed in zero service/infra configs, so there was no caller to keep working, and an alias would preserve the exact ambiguity being removed. mesofact KEEPS /revalidate as a transitional alias on the same handler, because that receiver IS live on us-east-001 and its callers are separately-rolled units. A dawn that 404s retries once on /revalidate; a freshen never falls back, since a feed body reaching a /revalidate-era mesofact receiver parses as re-render-the-whole-site (the R330-T14 failure mode).")
//! @yah:handoff("CLI: `yah almanac revalidate` is now `yah almanac dawn` with `revalidate` kept as a clap alias, so scripts/publish-desktop.sh and .github/workflows/fleet-index.yml keep working untouched. Verified both spellings resolve to the same help text.")
//! @yah:handoff("Docs swept where they now lie: the canonical two-stage table lives in Poke's doc comment (fetch.rs) and names the third stage too - the CDN purge that rides publish - since that is the one HTTP's own vocabulary owns. Also corrected receiver.rs, serve.rs, r2.rs, cli/almanac.rs, cli.rs, publish-desktop.sh's header, and workload-spec's MesofactRevalidateReceiver docs.")
//! @yah:handoff("R330-T14's structural guards (no hand-rolled poke bodies in the publish script or the fleet workflow) were checking the literal string '/revalidate' and would have gone blind the moment a script used a new path. They now check a shared RECEIVER_PATHS list covering /dawn, /freshen and the legacy alias.")
//! @yah:verify("cargo test -p yah-almanac (oss/yubaba) - 132 lib + 5 fleet_feed passed, 0 failed. Three new tests pin the fallback's exact boundaries, which is the risky part of a rename with an alias: a route poke reaches /dawn and a feed poke reaches /freshen; a route poke against a receiver serving ONLY /revalidate still lands, body and payload intact; a feed poke against that same receiver FAILS loudly instead of falling back.")
//! @yah:verify("cargo test -p mesofact --all-features (oss/mesofact) - 106 passed, 0 failed. Two new: both /dawn and /revalidate accept and enqueue, and the legacy alias enforces the same R752-B7 allowlist (same handler, so scoping and auth cannot diverge between the two paths).")
//! @yah:verify("cargo test -p kamaji-bin --all-features - 239 passed. cargo test -p issue-tracker - 16 passed. cargo test -p yah --lib almanac - 11 passed, including both hand-rolled-body guards against the real script and workflow.")
//! @yah:verify("Ran the binary, not just the tests: `yah almanac revalidate --help` and `yah almanac dawn --help` both resolve to the same subcommand, exit 0.")
//! @yah:verify("Generated artifacts regenerated after the workload-spec doc edit (export-ts + emit-schemas): packages/yah/workload-spec/index.ts and .yah/schema/workload.toml.schema.json moved, and cargo test -p xtask --test schema_drift is 3/3 green again.")
//! @yah:next("Delete LEGACY_MESOFACT_PATH and its fallback branch in oss/yubaba/crates/almanac/src/fetch.rs, plus the /revalidate alias route in mesofact's revalidate.rs and tenants.rs, once every deployed node serves /dawn. The two tests named a_route_poke_falls_back_to_revalidate_on_an_unrolled_receiver and dawn_is_the_name_and_revalidate_is_still_served go with them.")
//! @yah:next("The mesofact module is still named revalidate.rs and the CLI flag is still `mesofact serve --revalidate`. Left alone on purpose - renaming the flag is a second wire-shaped break (kamaji builds that argv) and belongs with the alias deletion above, not before it.")
//! @yah:gotcha("NOT DEPLOYED. Until us-east-001 is rolled, its mesofact receiver serves only /revalidate - which is why the dawn fallback exists and why nothing breaks in the meantime. Roll order does not matter (either side works against the other), but the fallback should be deleted once no node predates this change; that is the one piece of debt this ticket leaves behind.")

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::config::{ConfigError, FeedLoader};

/// Sender half of the freshen channel — carries the feed name to whoever
/// re-runs it. Clone and pass into `router()`.
pub type FreshenTx = mpsc::Sender<String>;

/// Mirror identity for feed-binding validation.
///
/// When supplied to [`router`], every `/freshen` request is checked: the
/// named feed must have an `on_change.service` matching `service_id`. Requests
/// for feeds bound to a different service (or with no binding) are rejected
/// with 422, keeping each receiver single-tenant.
#[derive(Clone)]
pub struct MirrorBind {
    /// Service id this receiver belongs to (`.yah/services/<id>/service.toml`).
    pub service_id: String,
    /// Mirror environment, e.g. `"cloud"`, `"pond"`, `"dev"`. Stored for
    /// logging; also the key the R335-F5 capability gate scopes its
    /// authorized-signer set by (per-`(service, env)` mirror). See the design
    /// in `.yah/docs/working/W058-almanac-mirror-binding.md` §12 — it supersedes the
    /// static `mirror_key` below with a node-identity signature once yubaba's
    /// control plane can provision the signer set.
    pub env: String,
    /// Directory containing feed TOML files (`.yah/almanac/`).
    pub almanac_dir: PathBuf,
}

#[derive(Clone)]
struct ReceiverState {
    tx: FreshenTx,
    /// When `Some`, inbound requests must carry the same key or receive 403.
    /// Static shared bearer secret — the pre-capability stopgap (R335-F2) that
    /// R335-F5 supersedes with a node-identity signature (design §12 of
    /// `.yah/docs/working/W058-almanac-mirror-binding.md`).
    mirror_key: Option<String>,
    /// When `Some`, the receiver validates feed-level mirror binding before
    /// forwarding. Feed not found → 404; wrong/missing service → 422.
    bind: Option<Arc<MirrorBind>>,
}

/// Build an axum `Router` that accepts `POST /freshen` with body
/// `{"feed": "<name>"}` and forwards the feed name over the provided channel.
///
/// ## Why `/freshen` and not `/revalidate` (R752-T10)
///
/// Two different invalidations used to share this path, one producer type and
/// one word, distinguishable only by which key the body carried — so a typo'd
/// key silently turned a feed refresh into a whole-site re-render:
///
/// | stage | endpoint | question |
/// |---|---|---|
/// | input changed | `POST /freshen {feed}` (here) | *go re-fetch this feed* |
/// | output stale | `POST /dawn {route, data_inputs}` (mesofact) | *re-render this page from these bytes* |
///
/// They usually chain — a feed's `on_change` dawns the route once the artifact
/// lands — but either fires alone, and only the second boots V8.
///
/// **freshen**, from dairy: a cow freshens when she calves and starts producing
/// again. That is exactly this stage — the source starts yielding new data.
/// Deliberately not "refresh": both "refresh" and "revalidate" are load-bearing
/// words in HTTP caching, which is a *third* thing (the CDN purge that rides
/// the publish leg), and reusing either keeps all three blurred.
///
/// No `/revalidate` alias on purpose: an alias preserves exactly the ambiguity
/// the rename exists to remove, and `almanac-serve` appears in zero service or
/// infra configs, so there is no deployed caller to keep working.
///
/// `mirror_key`: when `Some`, the body must carry `"mirror_key": "<same value>"`
/// or the request is rejected with 403.
///
/// `bind`: when `Some`, each request is validated against the feed's
/// `on_change.service` — feeds not bound to this mirror's service are rejected
/// with 422. Satisfies R335-F3 (scope a feed to the mirror it affects).
///
/// Typical wiring:
/// ```rust,ignore
/// let (tx, mut rx) = tokio::sync::mpsc::channel(16);
/// let app = almanac::receiver::router(tx, None, None);
/// // axum::serve(listener, app) in a task
/// // tokio::spawn(async move { while let Some(feed) = rx.recv().await { … } });
/// ```
pub fn router(tx: FreshenTx, mirror_key: Option<String>, bind: Option<MirrorBind>) -> Router {
    Router::new()
        .route("/freshen", post(freshen_handler))
        .with_state(ReceiverState { tx, mirror_key, bind: bind.map(Arc::new) })
}

async fn freshen_handler(
    State(state): State<ReceiverState>,
    Json(body): Json<FreshenBody>,
) -> StatusCode {
    // Mirror-key auth: reject cross-mirror freshens.
    if let Some(ref expected) = state.mirror_key {
        match &body.mirror_key {
            Some(provided) if provided == expected => {}
            _ => {
                tracing::warn!(
                    feed = %body.feed,
                    "freshen rejected — mirror_key mismatch (cross-mirror pollution blocked)"
                );
                return StatusCode::FORBIDDEN;
            }
        }
    }

    // Feed-level binding check: reject feeds not bound to this mirror's service.
    if let Some(ref bind) = state.bind {
        let loader = FeedLoader::new(&bind.almanac_dir);
        match loader.load(&body.feed) {
            Err(ConfigError::NotFound(_)) => {
                tracing::warn!(
                    feed = %body.feed,
                    service = %bind.service_id,
                    env = %bind.env,
                    "freshen rejected — feed not found in almanac"
                );
                return StatusCode::NOT_FOUND;
            }
            Err(e) => {
                tracing::error!(
                    feed = %body.feed,
                    err = %e,
                    "freshen — failed to load feed config"
                );
                return StatusCode::INTERNAL_SERVER_ERROR;
            }
            Ok(cfg) => {
                // Every `on_change` variant names a service, including the
                // render-free `reload` (R707-F4): this gate reads `on_change`
                // for the feed's *identity*, not for what it will render, so a
                // feed with nothing to rebuild still has to declare who owns it.
                let bound_service = match &cfg.feed.emit.on_change {
                    Some(on_change) => on_change.service(),
                    None => {
                        tracing::warn!(
                            feed = %body.feed,
                            service = %bind.service_id,
                            env = %bind.env,
                            "freshen rejected — feed has no mirror binding (no on_change)"
                        );
                        return StatusCode::UNPROCESSABLE_ENTITY;
                    }
                };
                if bound_service != bind.service_id.as_str() {
                    tracing::warn!(
                        feed = %body.feed,
                        feed_bound_to = bound_service,
                        receiver_service = %bind.service_id,
                        receiver_env = %bind.env,
                        "freshen rejected — feed is bound to a different mirror"
                    );
                    return StatusCode::UNPROCESSABLE_ENTITY;
                }
            }
        }
    }

    tracing::info!(feed = %body.feed, "freshen request received");
    match state.tx.try_send(body.feed) {
        Ok(_) => StatusCode::OK,
        Err(mpsc::error::TrySendError::Full(_)) => {
            tracing::warn!("freshen channel full — dropping request");
            StatusCode::SERVICE_UNAVAILABLE
        }
        Err(mpsc::error::TrySendError::Closed(_)) => StatusCode::SERVICE_UNAVAILABLE,
    }
}

#[derive(Deserialize)]
struct FreshenBody {
    feed: String,
    /// Caller's mirror identity token. Must match the receiver's configured
    /// `mirror_key` when one is set; absent or mismatched → 403.
    #[serde(default)]
    mirror_key: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Method, Request},
    };
    use std::fs;
    use tempfile::TempDir;
    use tower::util::ServiceExt;

    async fn post_json(app: Router, body: &'static str) -> axum::response::Response {
        let req = Request::builder()
            .method(Method::POST)
            .uri("/freshen")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        app.oneshot(req).await.unwrap()
    }

    fn write_feed(dir: &std::path::Path, name: &str, service: Option<&str>) {
        let on_change = match service {
            Some(svc) => format!(
                "\n[feed.emit.on_change]\nkind = \"mesofact-rebuild\"\nservice = \"{svc}\"\nroute = \"/releases\""
            ),
            None => String::new(),
        };
        let toml = format!(
            "[feed]\nname = \"{name}\"\n\n[feed.source]\nkind = \"gh-releases\"\nrepo = \"o/r\"\n\n[feed.trigger]\nkind = \"webhook\"\n\n[feed.emit]\nartifact = \"out.json\"{on_change}"
        );
        fs::write(dir.join(format!("{name}.toml")), toml).unwrap();
    }

    // ── mirror_key auth (R335-F2) ────────────────────────────────────────────

    #[tokio::test]
    async fn freshen_sends_feed_name() {
        let (tx, mut rx) = mpsc::channel(4);
        let app = router(tx, None, None);
        let resp = post_json(app,r#"{"feed":"releases"}"#).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(rx.try_recv().unwrap(), "releases");
    }

    #[tokio::test]
    async fn full_channel_returns_503() {
        let (tx, _rx) = mpsc::channel(1);
        tx.try_send("already-full".to_string()).unwrap();
        let app = router(tx, None, None);
        let resp = post_json(app,r#"{"feed":"releases"}"#).await;
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn correct_mirror_key_passes() {
        let (tx, mut rx) = mpsc::channel(4);
        let app = router(tx, Some("secret-abc".to_string()), None);
        let resp = post_json(app,r#"{"feed":"releases","mirror_key":"secret-abc"}"#).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(rx.try_recv().unwrap(), "releases");
    }

    #[tokio::test]
    async fn wrong_mirror_key_returns_403() {
        let (tx, _rx) = mpsc::channel(4);
        let app = router(tx, Some("secret-abc".to_string()), None);
        let resp = post_json(app,r#"{"feed":"releases","mirror_key":"wrong-key"}"#).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn absent_mirror_key_returns_403_when_configured() {
        let (tx, _rx) = mpsc::channel(4);
        let app = router(tx, Some("secret-abc".to_string()), None);
        let resp = post_json(app,r#"{"feed":"releases"}"#).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn no_configured_key_accepts_any_request() {
        let (tx, mut rx) = mpsc::channel(4);
        let app = router(tx, None, None);
        let resp = post_json(app,r#"{"feed":"releases","mirror_key":"anything"}"#).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(rx.try_recv().unwrap(), "releases");
    }

    // ── mirror binding (R335-F3) ─────────────────────────────────────────────

    fn bind(dir: &std::path::Path, service_id: &str) -> MirrorBind {
        MirrorBind {
            service_id: service_id.to_string(),
            env: "cloud".to_string(),
            almanac_dir: dir.to_path_buf(),
        }
    }

    #[tokio::test]
    async fn feed_matching_service_passes() {
        let tmp = TempDir::new().unwrap();
        write_feed(tmp.path(), "releases", Some("dev-yah"));
        let (tx, mut rx) = mpsc::channel(4);
        let app = router(tx, None, Some(bind(tmp.path(), "dev-yah")));
        let resp = post_json(app,r#"{"feed":"releases"}"#).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(rx.try_recv().unwrap(), "releases");
    }

    #[tokio::test]
    async fn feed_wrong_service_returns_422() {
        let tmp = TempDir::new().unwrap();
        write_feed(tmp.path(), "releases", Some("other-service"));
        let (tx, _rx) = mpsc::channel(4);
        let app = router(tx, None, Some(bind(tmp.path(), "dev-yah")));
        let resp = post_json(app,r#"{"feed":"releases"}"#).await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn feed_no_on_change_returns_422() {
        let tmp = TempDir::new().unwrap();
        write_feed(tmp.path(), "releases", None); // no on_change
        let (tx, _rx) = mpsc::channel(4);
        let app = router(tx, None, Some(bind(tmp.path(), "dev-yah")));
        let resp = post_json(app,r#"{"feed":"releases"}"#).await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    /// R707-F4: a `reload` feed has no route to render, and the gate must still
    /// admit it. `on_change` is the mirror binding first and the action second —
    /// a feed that rebuilds nothing is not a feed that belongs to nobody.
    #[tokio::test]
    async fn a_render_free_reload_feed_still_binds_and_passes() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("fleet.toml"),
            "[feed]\nname = \"fleet\"\n\n\
             [feed.source]\nkind = \"gh-releases\"\nrepo = \"o/r\"\n\n\
             [feed.trigger]\nkind = \"webhook\"\n\n\
             [feed.emit]\nartifact = \"out.json\"\n\n\
             [feed.emit.on_change]\nkind = \"reload\"\nservice = \"yah-cloud-admin\"\n",
        )
        .unwrap();
        let (tx, mut rx) = mpsc::channel(4);
        let app = router(tx, None, Some(bind(tmp.path(), "yah-cloud-admin")));
        let resp = post_json(app, r#"{"feed":"fleet"}"#).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(rx.try_recv().unwrap(), "fleet");
    }

    /// ...and the gate is still a gate for it: a `reload` bound elsewhere is
    /// rejected exactly like a `mesofact-rebuild` bound elsewhere. Dropping
    /// `on_change` to "simplify" a render-free feed is what would have deleted
    /// this.
    #[tokio::test]
    async fn a_reload_feed_bound_to_another_service_returns_422() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("fleet.toml"),
            "[feed]\nname = \"fleet\"\n\n\
             [feed.source]\nkind = \"gh-releases\"\nrepo = \"o/r\"\n\n\
             [feed.trigger]\nkind = \"webhook\"\n\n\
             [feed.emit]\nartifact = \"out.json\"\n\n\
             [feed.emit.on_change]\nkind = \"reload\"\nservice = \"someone-else\"\n",
        )
        .unwrap();
        let (tx, _rx) = mpsc::channel(4);
        let app = router(tx, None, Some(bind(tmp.path(), "yah-cloud-admin")));
        let resp = post_json(app, r#"{"feed":"fleet"}"#).await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn feed_not_in_almanac_returns_404() {
        let tmp = TempDir::new().unwrap(); // empty almanac dir
        let (tx, _rx) = mpsc::channel(4);
        let app = router(tx, None, Some(bind(tmp.path(), "dev-yah")));
        let resp = post_json(app,r#"{"feed":"nonexistent"}"#).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn no_bind_skips_feed_check() {
        // Without a bind, any feed name is forwarded regardless of almanac state.
        let (tx, mut rx) = mpsc::channel(4);
        let app = router(tx, None, None);
        let resp = post_json(app,r#"{"feed":"unknown-feed"}"#).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(rx.try_recv().unwrap(), "unknown-feed");
    }
}
