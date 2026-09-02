//! `mesofact-dev` — the dev-tier affordances, and nothing else.
//!
//! **This crate is deliberately small.** The serving engine (`Server`, SSR
//! dispatch, the revalidate receiver, tenants, the same-origin proxy) used to
//! live here, which meant the *prod* `mesofact-serve` binary — which shipped
//! from this crate — linked the file watcher and the dev S3 surface. That broke
//! the dev/prod crate boundary W225 §2 relies on for its security claim
//! ("prod is clean by construction … the crate boundary already keeps it out of
//! prod"). It wasn't: cleanliness rested on linker dead-stripping.
//!
//! The engine now lives in the `mesofact` facade and this crate *depends on*
//! it, holding only the pieces that must never reach a prod binary:
//!
//! - [`watcher`] — the rebuild-on-change file watcher.
//! - [`s3`] — the local S3 surface that stands in for R2 during `dev`
//!   (W225 §2 "local pond emulation").
//! - [`app`] — the **library-tier** dev entry point, [`serve_app`]: the dev
//!   counterpart of [`mesofact::serve_app`] for a consumer whose routes are
//!   Rust handlers rather than a built `dist/` tree. Read its module doc for
//!   what the dev half of that tier is and, just as load-bearing, what it
//!   deliberately is not.
//! - [`cli`] — the `mes` toolchain CLI, and the two bin targets over it.
//!
//! [`cli`] carries the prod verbs (`serve`, `publish`, `new`) as well as the
//! dev ones, so consumers learn one CLI — but it gets them by *calling into*
//! [`mesofact::cli`], which is the direction that costs the prod binary
//! nothing. The boundary above is about what links into a binary, not about
//! which verbs a binary spells.
//!
//! Engine types are re-exported below so existing `mesofact_dev::Server`-style
//! callsites keep working; new code should prefer `mesofact::…` directly.
//!
//! @arch:see(.yah/docs/working/W225-mesofact-consumer-deployment-model.md)

pub mod app;
pub mod cli;
pub mod s3;
pub mod watcher;

pub use app::{serve_app, DevServer, DEV_STATE_DIR};
pub use s3::{DevS3, DEFAULT_BUCKET as DEV_S3_BUCKET};
pub use watcher::{BuildDriver, WatchOptions, Watcher};

// Engine re-exports — the serving path now lives in the `mesofact` facade.
pub use mesofact::proxy;
pub use mesofact::server;
pub use mesofact::{DistPointer, Identity, ProxyMap, ProxyState, Server, DEFAULT_PORT};
#[cfg(feature = "ssr")]
pub use mesofact::{
    revalidate, ssr, tenants, ResiliencePolicy, RetryPolicy, SsrChild, SsrSlot, SsrSpawnOptions,
    DEFAULT_RESILIENCE_TIMEOUT_MS,
};

#[cfg(test)]
mod tests {
    //! Cross-boundary smoke: the dev S3 surface driven through the facade's
    //! `Server`. This test is the reason it lives here rather than in the
    //! facade — it needs BOTH the engine (facade) and `DevS3` (this crate), and
    //! the dependency only points one way.

    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use mesofact_publisher::ObjectStore;
    use std::sync::Arc;
    use tempfile::tempdir;
    use tower::ServiceExt;

    async fn body_string(response: axum::response::Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn deferred_workload() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        let dist = dir.path().join("dist");
        std::fs::create_dir_all(dist.join("html")).unwrap();
        std::fs::write(
            dist.join("manifest.json"),
            r#"{"version":"1","build_id":"b","routes":[{"route":"/c/:slug","mode":"static","render_entrypoint":"dist/server/c_slug.js","cache_policy":{"ttl":0},"prerender":{"deferred":true}}]}"#,
        )
        .unwrap();
        dir
    }

    async fn flip_instance(store: &Arc<dyn ObjectStore>, key: &str, content_root: &str) {
        use mesofact_publisher::{ObjectPointerStore, Pointer, PointerStore};
        ObjectPointerStore::new(store.clone())
            .flip(
                key,
                Pointer { content_root: content_root.into(), source_root: None, published_at: None },
            )
            .await
            .unwrap();
    }

    async fn put_bytes(store: &Arc<dyn ObjectStore>, key: &str, body: &'static [u8]) {
        use mesofact_publisher::PutOpts;
        store
            .put(
                key,
                axum::body::Bytes::from_static(body),
                PutOpts { content_type: "text/html".into(), content_hash: "h".into(), cache_control: None },
            )
            .await
            .unwrap();
    }

    /// Real-path smoke (W270 §9): resolve a deferred route through an
    /// `mesofact_publisher::S3Store` pointed at the live dev-S3 surface — the
    /// exact wiring `main.rs` uses. Proves the SigV4-signed requests are
    /// accepted by the anonymous `s3s-fs` surface, so the local
    /// `publish → view` loop resolves over real HTTP, not just the
    /// InMemoryStore the facade's own tests use.
    #[cfg(feature = "ssr")]
    #[tokio::test]
    async fn deferred_route_resolves_through_dev_s3_store() {
        use mesofact_publisher::S3Store;

        let dir = deferred_workload();
        let dev = DevS3::start(dir.path().join("s3-surface"), DEV_S3_BUCKET)
            .await
            .unwrap();
        let store: Arc<dyn ObjectStore> = Arc::new(
            S3Store::new(dev.endpoint.clone(), dev.bucket.clone(), "auto", "dev", "dev").unwrap(),
        );

        // Publisher-side: flip the pointer + write the render-root bytes, both
        // through the S3Store (the same store the server resolves against).
        flip_instance(&store, "c/xyz", "content/xyz.html").await;
        put_bytes(&store, "content/xyz.html", b"<h1>via dev s3</h1>").await;

        let app = Server::from_workload(dir.path())
            .unwrap()
            .with_instance_store(store)
            .router();
        let response = app
            .oneshot(Request::builder().uri("/c/xyz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("cache-control").unwrap(),
            "public, max-age=31536000, immutable"
        );
        assert!(body_string(response).await.contains("via dev s3"));
    }

    /// R444 end-to-end: a `mode:"ssr"` render handler that imports `r2` from
    /// `@mesofact/runtime` and calls `.fetch(key)` resolves against the dev S3
    /// surface from *inside the in-process V8 isolate* — the exact wiring
    /// `main.rs` does (`DevS3::start` → `SsrSpawnOptions::with_env` →
    /// `ssr::spawn`), proving the env + `[sources.r2]` plumbing reaches a real
    /// request, not just a unit-level `resolve_r2_sources` call.
    #[cfg(feature = "ssr")]
    #[tokio::test]
    async fn ssr_route_resolves_r2_source_against_dev_s3() {
        let dir = tempdir().unwrap();
        let dev = DevS3::start(dir.path().join("s3-surface"), DEV_S3_BUCKET)
            .await
            .unwrap();

        // Seed the bucket directly over HTTP — the same anonymous PUT path
        // s3.rs's own round-trip test exercises.
        let put_url = format!("{}/{}/greeting.txt", dev.endpoint, dev.bucket);
        let put = reqwest::Client::new()
            .put(&put_url)
            .body("hello from dev r2")
            .send()
            .await
            .unwrap();
        assert!(put.status().is_success(), "seed PUT status: {}", put.status());

        std::fs::write(
            dir.path().join("mesofact.config.toml"),
            "[sources.assets]\nkind = \"r2\"\nbucket = \"dev\"\nendpoint_env = \"R2_ENDPOINT\"\naccess_key_id_env = \"R2_ACCESS_KEY_ID\"\nsecret_access_key_env = \"R2_SECRET_ACCESS_KEY\"\n",
        )
        .unwrap();

        let server_dir = dir.path().join("dist/server");
        std::fs::create_dir_all(&server_dir).unwrap();
        std::fs::write(
            server_dir.join("greet.js"),
            "import { r2 } from \"@mesofact/runtime\";\n\
             export default async function () {\n\
               const bytes = await r2('assets').fetch('greeting.txt');\n\
               const text = bytes ? new TextDecoder().decode(bytes) : null;\n\
               return new Response(text ?? 'MISSING', { status: 200 });\n\
             }\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("dist/manifest.json"),
            r#"{"routes": [{"route": "/greet", "mode": "ssr", "render_entrypoint": "dist/server/greet.js"}]}"#,
        )
        .unwrap();

        let opts = ssr::SpawnOptions::new(
            dir.path().to_path_buf(),
            dir.path().join("dist"),
            dir.path().join(".mesofact-dev"),
        )
        .with_env(dev.env_vars());
        let child = ssr::spawn(opts).await.unwrap().expect("ssr present");

        let resp = child
            .dispatch(
                "/greet",
                mesofact::ssr_runtime::DispatchRequest {
                    method: "GET".into(),
                    url: "http://dev/greet".into(),
                    headers: vec![],
                    body: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(String::from_utf8(resp.body).unwrap(), "hello from dev r2");
    }
}
