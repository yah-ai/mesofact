//! Dev-tier S3 surface (R490-F7).
//!
//! An FS-backed [s3s-fs] endpoint that `mesofact-dev` hosts so a workload's
//! `@mesofact/runtime` `R2Adapter` can resolve against a *local* bucket during
//! `bun run dev` instead of real Cloudflare R2. This fills the
//! "build→PUT→read" contract that the R255-S5 spike explicitly left
//! unexercised at tier 1 — that spike only ruled on the static-asset
//! browser-GET path (still serve-off-disk); the runtime `r2` adapter is a
//! separate consumer that *does* speak the S3 API.
//!
//! Design notes:
//! - **Anonymous.** Bound to `127.0.0.1` on a dynamic port, this is a
//!   single-tenant loopback dev appliance — no SigV4 verification. The
//!   `R2Adapter` still signs with `aws4fetch` using whatever dummy creds the
//!   workload injects; s3s ignores the signature when no auth is configured.
//! - **Bucket = a pre-created dir under the state root.** s3s-fs treats
//!   top-level dirs under its root as buckets, so creating `<root>/<bucket>/`
//!   up front is enough for `PutObject` to land.
//! - **Out of scope here (handed off):** seeding the bucket from `dist/`, and
//!   injecting coords into the *in-process V8 SSR runtime* (which can't inherit
//!   `process.env` the way the build subprocess does). See R490-F7.
//!
//! [s3s-fs]: https://docs.rs/s3s-fs

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::net::TcpListener;

/// Default dev bucket name. Workloads point `[sources.r2] bucket` here in dev.
pub const DEFAULT_BUCKET: &str = "dev";

/// Dummy credentials the surface accepts. SigV4-signing clients (the publisher's
/// `S3Store`, the JS `R2Adapter`) sign with these; s3s verifies the signature
/// against them. Handed to consumers verbatim via [`DevS3::env_vars`].
const DEV_ACCESS_KEY: &str = "dev";
const DEV_SECRET_KEY: &str = "dev";

/// Coordinates of a running dev S3 surface, handed to consumers (build-child
/// env, discovery file) so they can point an S3 client at it.
#[derive(Debug, Clone)]
pub struct DevS3 {
    /// e.g. `http://127.0.0.1:54321` — no trailing slash, path-style.
    pub endpoint: String,
    /// The single dev bucket, pre-created on disk.
    pub bucket: String,
    /// On-disk root backing the surface (`<state_dir>/s3`).
    pub root: PathBuf,
}

impl DevS3 {
    /// Start the surface: create `<root>/<bucket>/`, bind `127.0.0.1:0`, and
    /// spawn the serve loop on the current tokio runtime. Returns the bound
    /// coordinates; the server runs until the process exits.
    pub async fn start(root: impl Into<PathBuf>, bucket: &str) -> Result<DevS3> {
        let root = root.into();
        let bucket_dir = root.join(bucket);
        tokio::fs::create_dir_all(&bucket_dir)
            .await
            .with_context(|| format!("creating dev S3 bucket dir {}", bucket_dir.display()))?;

        let service = build_service(&root)?;

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .context("binding dev S3 listener")?;
        let addr = listener.local_addr().context("dev S3 local_addr")?;
        let endpoint = format!("http://{addr}");

        tokio::spawn(serve_loop(listener, service));

        Ok(DevS3 {
            endpoint,
            bucket: bucket.to_string(),
            root,
        })
    }

    /// Conventional env vars mesofact-dev injects so a workload's `[sources.r2]`
    /// can resolve in dev: `R2_ENDPOINT`, `R2_BUCKET`, plus dummy credentials
    /// (the surface is anonymous, but the adapter's config still requires the
    /// key/secret env vars to be present to register the source).
    pub fn env_vars(&self) -> Vec<(String, String)> {
        vec![
            ("R2_ENDPOINT".to_string(), self.endpoint.clone()),
            ("R2_BUCKET".to_string(), self.bucket.clone()),
            ("R2_ACCESS_KEY_ID".to_string(), DEV_ACCESS_KEY.to_string()),
            ("R2_SECRET_ACCESS_KEY".to_string(), DEV_SECRET_KEY.to_string()),
        ]
    }
}

/// Permissive access layer: allow every request, authenticated or not. s3s only
/// consults `S3Access` when an auth provider is configured, so pairing this with
/// [`SimpleAuth`](s3s::auth::SimpleAuth) means signed `dev/dev` requests verify
/// AND unsigned loopback requests still pass — preserving the anonymous
/// dev-appliance contract while unblocking SigV4 clients.
struct AllowAllAccess;

#[async_trait::async_trait]
impl s3s::access::S3Access for AllowAllAccess {
    async fn check(&self, _cx: &mut s3s::access::S3AccessContext<'_>) -> s3s::S3Result<()> {
        Ok(())
    }
}

fn build_service(root: &Path) -> Result<s3s::service::S3Service> {
    use s3s::auth::SimpleAuth;
    use s3s::service::S3ServiceBuilder;
    // s3s_fs::Error doesn't impl std::error::Error, so map it by Display.
    let fs = s3s_fs::FileSystem::new(root)
        .map_err(|e| anyhow::anyhow!("opening s3s-fs at {}: {e:?}", root.display()))?;
    let mut builder = S3ServiceBuilder::new(fs);
    // Accept SigV4-signed requests. Without ANY auth provider s3s answers 501
    // ("no authentication provider") to every *signed* request — which breaks
    // the `S3Store`-based pointer/content reads the local publish→view loop
    // needs (W270 §9), and the R2Adapter signs too. SimpleAuth verifies the
    // dev/dev signature; AllowAllAccess keeps anonymous loopback access working,
    // so the surface stays the single-tenant dev appliance it was.
    builder.set_auth(SimpleAuth::from_single(DEV_ACCESS_KEY, DEV_SECRET_KEY));
    builder.set_access(AllowAllAccess);
    Ok(builder.build())
}

async fn serve_loop(listener: TcpListener, service: s3s::service::S3Service) {
    use hyper_util::rt::{TokioExecutor, TokioIo};
    use hyper_util::server::conn::auto::Builder as ConnBuilder;

    let http = ConnBuilder::new(TokioExecutor::new());
    loop {
        let socket = match listener.accept().await {
            Ok((socket, _)) => socket,
            Err(e) => {
                tracing::warn!(error = %e, "dev S3: accept failed");
                continue;
            }
        };
        // `.into_owned()` detaches the connection future from the borrowed
        // builder so it can be spawned with a `'static` lifetime.
        let conn = http
            .serve_connection(TokioIo::new(socket), service.clone())
            .into_owned();
        tokio::spawn(async move {
            if let Err(e) = conn.await {
                tracing::debug!(error = %e, "dev S3: connection ended");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn start_creates_bucket_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let s3 = DevS3::start(tmp.path().join("s3"), DEFAULT_BUCKET)
            .await
            .unwrap();
        assert!(s3.root.join(DEFAULT_BUCKET).is_dir());
        assert!(s3.endpoint.starts_with("http://127.0.0.1:"));
        assert_eq!(s3.bucket, DEFAULT_BUCKET);
    }

    #[tokio::test]
    async fn put_then_get_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let s3 = DevS3::start(tmp.path().join("s3"), DEFAULT_BUCKET)
            .await
            .unwrap();

        let url = format!("{}/{}/hello.txt", s3.endpoint, s3.bucket);
        let client = reqwest::Client::new();

        // Anonymous PUT (s3s skips signature checks with no auth configured).
        let put = client
            .put(&url)
            .body("WORLD")
            .send()
            .await
            .expect("PUT request");
        assert!(
            put.status().is_success(),
            "PUT status: {} body: {:?}",
            put.status(),
            put.text().await
        );

        let get = client.get(&url).send().await.expect("GET request");
        assert!(get.status().is_success(), "GET status: {}", get.status());
        assert_eq!(get.text().await.unwrap(), "WORLD");

        // And it actually landed on disk under the bucket dir.
        assert!(s3.root.join(DEFAULT_BUCKET).join("hello.txt").is_file());
    }
}
