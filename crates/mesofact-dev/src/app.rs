//! The **library-tier dev entry point** — [`serve_app`], the counterpart to
//! [`mesofact::serve_app`] for a consumer whose routes are Rust handlers.
//!
//! This is the `mesofact_dev::serve(app::router())` that W225 §2's Consumer DX
//! sketch promised and that nothing implemented (R832-T1). The two thin bin
//! targets a library-tier project carries differ by one identifier:
//!
//! ```ignore
//! // src/bin/yah-dashboard.rs      — the binary CI ships
//! mesofact::serve_app(yah_dashboard::router(), addr).await
//!
//! // src/bin/yah-dashboard-dev.rs  — the binary you run locally
//! mesofact_dev::serve_app(yah_dashboard::router(), addr).await
//! ```
//!
//! # What the dev half of a library-tier consumer actually is
//!
//! R832-T1 existed because this was an open design question, and the answer is
//! narrower than the standalone tier's dev binary. Recorded here rather than in
//! a doc, because the next person to add a dev affordance needs it:
//!
//! **The watcher and live-reload do not carry over, and cannot.** Both are
//! functions of a *built `dist/` tree*: [`crate::Watcher`] re-runs the bundler
//! and rotates `dist` into `.mesofact-dev/gen-N`, and the SSR pool is
//! re-spawned against the new generation. A library-tier consumer has no such
//! tree — its routes are Rust functions, and the only edit that changes one is
//! a `.rs` edit, which requires re-linking the very process that would have to
//! perform the reload. No in-process affordance can close that loop. The loop
//! is `cargo watch -x 'run --bin <name>-dev'`, and it lives outside the binary
//! by necessity, not by omission.
//!
//! **The dev object store does carry over, and is the whole of the
//! difference.** A handler that reads or writes R2 builds its store from
//! environment coordinates. In prod those point at Cloudflare R2; in dev
//! nothing points anywhere, so without this the consumer either stubs the store
//! or runs a MinIO container — precisely the "local pond emulation" W225 §2
//! puts in this crate to avoid. [`DevServer::start`] brings up the FS-backed
//! [`DevS3`] surface, [`DevServer::export_env`] publishes its coordinates into
//! this process's environment, and `.mesofact-dev/s3.json` carries them for
//! out-of-process tooling (`aws s3 --endpoint-url …`, a test harness).
//!
//! So `mesofact_dev::serve_app` is `mesofact::serve_app` plus a local R2 and a
//! banner. **The smallness is the finding, not a shortfall.** What the two-bin
//! pattern buys is the link-graph boundary (W225 §2, "always-release,
//! two-binary pattern") — prod clean by construction because its dependency
//! closure cannot reach this crate — and that boundary is worth having on day
//! one, when the dev side adds a single service. Every dev affordance added
//! here later reaches consumers without any of them changing a line.
//!
//! Not implemented, and deliberately not stubbed: permissive CORS and verbose
//! error overlays. W225 §2 lists both among the dev affordances, but neither
//! exists anywhere in mesofact today (no `tower_http::cors` use in either
//! crate), so there is nothing to lift — writing them here would be new
//! product, not an entry point over existing behaviour.
//!
//! @arch:see(.yah/docs/working/W225-mesofact-consumer-deployment-model.md)

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use axum::Router;
use tracing::{info, warn};

use crate::{DevS3, DEV_S3_BUCKET};

/// Directory, relative to a project root, that holds dev-only scratch state —
/// the S3 surface's backing store and its discovery file. The standalone tier's
/// `mes dev` uses the same name, so a project that starts standalone and grows
/// a Rust half keeps one `.gitignore` line.
pub const DEV_STATE_DIR: &str = ".mesofact-dev";

/// The dev-tier ambient services, started and ready to serve a caller's
/// [`Router`].
///
/// Prefer the one-call [`serve_app`] unless your `router()` reads the store
/// coordinates *at construction time* — see [`serve_app`]'s ordering note.
pub struct DevServer {
    s3: DevS3,
    state_dir: PathBuf,
}

impl DevServer {
    /// Bring up the dev-tier services for a project rooted at `root`.
    ///
    /// Starts the local object store under `<root>/.mesofact-dev/s3` and writes
    /// its coordinates to `<root>/.mesofact-dev/s3.json`. Does **not** touch
    /// the process environment — call [`export_env`](Self::export_env) for
    /// that, before building any router that reads it.
    pub async fn start(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        // Canonicalize so the state dir does not move under a handler that
        // changed the cwd, and so the logged path is the one on disk. A root
        // that does not exist yet is left as given — `create_dir_all` below
        // reports it far more clearly than `canonicalize` would.
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let state_dir = root.join(DEV_STATE_DIR);
        tokio::fs::create_dir_all(&state_dir)
            .await
            .with_context(|| format!("creating dev state dir {}", state_dir.display()))?;

        let s3 = DevS3::start(state_dir.join("s3"), DEV_S3_BUCKET).await?;
        info!(
            endpoint = %s3.endpoint,
            bucket = %s3.bucket,
            "mesofact-dev: local object store ready (stands in for R2)",
        );

        // Discovery file for out-of-process tooling. Best-effort on purpose:
        // a read-only project dir is a reason to log, not to refuse to serve.
        let discovery = state_dir.join("s3.json");
        if let Err(e) = std::fs::write(
            &discovery,
            serde_json::json!({ "endpoint": s3.endpoint, "bucket": s3.bucket }).to_string(),
        ) {
            warn!(error = %e, path = %discovery.display(), "dev S3: could not write discovery file");
        }

        Ok(Self { s3, state_dir })
    }

    /// Coordinates of the running local object store.
    pub fn s3(&self) -> &DevS3 {
        &self.s3
    }

    /// The `.mesofact-dev` directory backing this server's scratch state.
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// Publish the store coordinates into this process's environment
    /// (`R2_ENDPOINT`, `R2_BUCKET`, `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY`
    /// — see [`DevS3::env_vars`]), so a handler that builds its store from env
    /// resolves against the local bucket with no dev-specific code.
    ///
    /// Existing values are **overwritten**: a dev binary that left a real
    /// `R2_ENDPOINT` in place would write to production from a laptop, which is
    /// the one outcome this surface exists to make impossible. Anything already
    /// set is logged so the override is never silent.
    pub fn export_env(&self) {
        for (key, value) in self.s3.env_vars() {
            if let Ok(prior) = std::env::var(&key) {
                if prior != value {
                    warn!(%key, %prior, "mesofact-dev: overriding inherited value with the local store's");
                }
            }
            std::env::set_var(&key, &value);
        }
    }

    /// Serve `app` with the standard mesofact stack — the `/livez` + `/readyz`
    /// probes, the trace layer, and graceful shutdown — until Ctrl+C or
    /// SIGTERM, exactly as [`mesofact::serve_app`] does in prod.
    pub async fn serve(self, app: Router, addr: SocketAddr) -> Result<()> {
        warn!(
            addr = %addr,
            s3_endpoint = %self.s3.endpoint,
            "mesofact-dev: DEV BINARY — dev affordances are linked in; do not ship this target",
        );
        mesofact::serve_app(app, addr).await
    }
}

/// Start the dev-tier services, publish their coordinates into the environment,
/// and serve `app` until Ctrl+C or SIGTERM. The dev counterpart of
/// [`mesofact::serve_app`], and the whole body of a library-tier project's
/// `src/bin/<name>-dev.rs`.
///
/// State lands under `./.mesofact-dev`, relative to the process's current
/// directory — the project root, when run via `cargo run`.
///
/// **Ordering note.** `app` is already built by the time this is called, so a
/// `router()` that constructs its object store *at construction time* has
/// already read an unset `R2_ENDPOINT`. Handlers that build the store per
/// request (or lazily) are unaffected. If yours reads env eagerly, drive the
/// two halves in order instead:
///
/// ```ignore
/// let dev = mesofact_dev::DevServer::start(".").await?;
/// dev.export_env();
/// dev.serve(yah_dashboard::router(), addr).await
/// ```
pub async fn serve_app(app: Router, addr: SocketAddr) -> Result<()> {
    let dev = DevServer::start(std::env::current_dir().context("reading current directory")?)
        .await?;
    dev.export_env();
    dev.serve(app, addr).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{Request, StatusCode};
    use tempfile::tempdir;

    #[tokio::test]
    async fn start_creates_state_dir_and_discovery_file() {
        let root = tempdir().unwrap();
        let dev = DevServer::start(root.path()).await.unwrap();

        assert_eq!(dev.state_dir(), dev.state_dir().canonicalize().unwrap());
        assert!(dev.state_dir().ends_with(DEV_STATE_DIR));
        assert!(dev.state_dir().join("s3").join(DEV_S3_BUCKET).is_dir());

        let discovery: serde_json::Value = serde_json::from_slice(
            &std::fs::read(dev.state_dir().join("s3.json")).expect("discovery file written"),
        )
        .unwrap();
        assert_eq!(discovery["endpoint"], dev.s3().endpoint);
        assert_eq!(discovery["bucket"], DEV_S3_BUCKET);
    }

    /// The point of the entry point: a plain Rust handler that builds its
    /// object store the way a prod handler does — from `R2_*` env — resolves
    /// against the local surface, with nothing dev-specific in the handler.
    ///
    /// Drives the router through the same [`mesofact::wrap`] stack
    /// [`DevServer::serve`] serves it under, so the probe routes and the
    /// caller's routes are proven to coexist rather than assumed to.
    #[cfg(feature = "ssr")]
    #[tokio::test]
    async fn handler_reads_r2_from_env_against_the_dev_surface() {
        use axum::body::Body;
        use axum::routing::get;
        use mesofact_publisher::{ObjectStore, S3Store};
        use tower::ServiceExt;

        let root = tempdir().unwrap();
        let dev = DevServer::start(root.path()).await.unwrap();
        dev.export_env();

        // Seed the bucket over plain HTTP, as any out-of-process tool would.
        let put = reqwest::Client::new()
            .put(format!("{}/{}/greeting.txt", dev.s3().endpoint, dev.s3().bucket))
            .body("hello from the dev store")
            .send()
            .await
            .unwrap();
        assert!(put.status().is_success(), "seed PUT status: {}", put.status());

        // A handler with no knowledge of dev: env in, bytes out.
        async fn greeting() -> String {
            let store = S3Store::new(
                std::env::var("R2_ENDPOINT").unwrap(),
                std::env::var("R2_BUCKET").unwrap(),
                "auto",
                std::env::var("R2_ACCESS_KEY_ID").unwrap(),
                std::env::var("R2_SECRET_ACCESS_KEY").unwrap(),
            )
            .unwrap();
            let bytes = store.get("greeting.txt").await.unwrap().unwrap();
            String::from_utf8(bytes.to_vec()).unwrap()
        }

        let app = mesofact::wrap(Router::new().route("/greeting", get(greeting)));

        let response = app
            .clone()
            .oneshot(Request::builder().uri("/greeting").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            String::from_utf8(body.to_vec()).unwrap(),
            "hello from the dev store"
        );

        // …and the standard stack is still there around it.
        let probe = app
            .oneshot(
                Request::builder()
                    .uri(mesofact::LIVE_PATH)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(probe.status(), StatusCode::OK);
    }
}
