//! Publish orchestrator. Reads a built `dist/` tree and drives an
//! [`ObjectStore`] + [`CdnPurger`] pair through three steps:
//!
//! 1. Upload every artifact under `/{build_id}/...`.
//! 2. Write the per-build snapshot of `manifest.json` + `tag-index.json` under
//!    `/{build_id}/` (so `--pin` has something to restore from).
//! 3. Atomically swap the root `/manifest.json` and `/tag-index.json` pointers
//!    (commit point — the new build only "goes live" once this PUT lands).
//!
//! T2 layered prior-key content-hash diffing on the T1 happy path: each upload
//! [`head`](ObjectStore::head)s the destination key first and skips the `PUT`
//! when the prior object's `content_hash` matches the new body. T3 layers tag
//! diffing on top of that: the orchestrator fetches the prior root
//! `/tag-index.json` before commit, diffs added/removed/changed-URL tags
//! against the new one, and calls [`CdnPurger::purge_tags`] with the union so
//! only routes whose content actually moved are evicted from the CDN.
//!
//! R749-B4 made this a real `cache_policy` consumer. The upload walk now maps
//! each `html/<key>.html` back to the path it serves at
//! ([`page_path_for`]) and asks the manifest's
//! [`CachePolicyTable`](mesofact_core::CachePolicyTable) what that route
//! declared, instead of choosing a `Cache-Control` from the path prefix alone.
//! The prefix table survives as the default for pages nobody declared a policy
//! for. Both serving tiers already honoured the declaration; this is the third
//! and last front door.

use crate::{CdnPurger, ObjectStore, PurgeError, PutOpts, StoreError};
use bytes::Bytes;
use mesofact_core::{CachePolicyTable, Manifest};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use thiserror::Error;
use tokio::fs;

#[derive(Debug, Error)]
pub enum PublishError {
    #[error("dist dir not found: {0}")]
    DistMissing(PathBuf),
    #[error("manifest.json missing in {0}")]
    ManifestMissing(PathBuf),
    #[error("tag-index.json missing in {0}")]
    TagIndexMissing(PathBuf),
    #[error("parse: {0}")]
    Parse(String),
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("purger: {0}")]
    Purger(#[from] PurgeError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("pin: build_id={0} not retained in store")]
    PinNotFound(String),
}

/// Mirrors `packages/mesofact-build/src/tag-index.ts` — tag → resolved URLs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagIndex {
    pub build_id: String,
    pub tags: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PublishReport {
    pub build_id: String,
    pub uploaded_keys: Vec<String>,
    pub skipped_keys: Vec<String>,
    pub purged_tags: Vec<String>,
}

/// Idempotent orchestrator. Each upload [`head`](ObjectStore::head)s the
/// destination key first and skips the `PUT` when the prior object's
/// `content_hash` matches the new body, so re-running against an unchanged
/// `dist/` is a no-op at the store level.
pub async fn publish_dist(
    dist_dir: &Path,
    store: &dyn ObjectStore,
    purger: &dyn CdnPurger,
) -> Result<PublishReport, PublishError> {
    if !fs::try_exists(dist_dir).await? {
        return Err(PublishError::DistMissing(dist_dir.to_path_buf()));
    }

    let manifest_path = dist_dir.join("manifest.json");
    let tag_index_path = dist_dir.join("tag-index.json");

    let manifest_bytes = match fs::read(&manifest_path).await {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(PublishError::ManifestMissing(manifest_path));
        }
        Err(e) => return Err(PublishError::Io(e)),
    };
    let tag_index_bytes = match fs::read(&tag_index_path).await {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(PublishError::TagIndexMissing(tag_index_path));
        }
        Err(e) => return Err(PublishError::Io(e)),
    };

    let manifest: Manifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| PublishError::Parse(format!("manifest.json: {e}")))?;
    let tag_index: TagIndex = serde_json::from_slice(&tag_index_bytes)
        .map_err(|e| PublishError::Parse(format!("tag-index.json: {e}")))?;
    if manifest.build_id != tag_index.build_id {
        return Err(PublishError::Parse(format!(
            "build_id mismatch: manifest={} tag-index={}",
            manifest.build_id, tag_index.build_id
        )));
    }
    let build_id = manifest.build_id.clone();

    // Snapshot the live tag-index *before* the commit overwrites it so we can
    // diff added/removed/changed-URL tags and purge only what actually moved.
    // A malformed prior index is treated as "no prior" — we'd rather over-purge
    // on the next change than refuse to publish over a corrupt pointer.
    let prior_tag_index: Option<TagIndex> = match store.get("tag-index.json").await? {
        Some(bytes) => serde_json::from_slice(&bytes).ok(),
        None => None,
    };

    // R749-B4: the declared `cache_policy` follows the page to the CDN. Built
    // from the manifest we already parsed, so the header on the object and the
    // header `mesofact serve` stamps on the same route come out of one table.
    let cache_policy = CachePolicyTable::from_routes(&manifest.routes);

    let mut uploaded: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();

    for entry in walk_files(dist_dir).await? {
        let rel = entry
            .strip_prefix(dist_dir)
            .expect("walker stays under dist_dir");
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        // Pointers go to root last, not under /{build_id}/.
        if rel_str == "manifest.json" || rel_str == "tag-index.json" {
            continue;
        }
        let body = Bytes::from(fs::read(&entry).await?);
        let key = format!("{build_id}/{rel_str}");
        put_with_hash(
            store,
            &key,
            body,
            content_type_for(&entry),
            cache_control_for(&rel_str, &cache_policy),
            &mut uploaded,
            &mut skipped,
        )
        .await?;
    }

    // Per-build pointer snapshots so --pin can restore from /{build_id}/.
    let manifest_body = Bytes::from(manifest_bytes);
    let tag_index_body = Bytes::from(tag_index_bytes);
    put_with_hash(
        store,
        &format!("{build_id}/manifest.json"),
        manifest_body.clone(),
        "application/json".into(),
        Some("public, max-age=31536000, immutable".into()),
        &mut uploaded,
        &mut skipped,
    )
    .await?;
    put_with_hash(
        store,
        &format!("{build_id}/tag-index.json"),
        tag_index_body.clone(),
        "application/json".into(),
        Some("public, max-age=31536000, immutable".into()),
        &mut uploaded,
        &mut skipped,
    )
    .await?;

    // Commit point: flip the root pointers. manifest.json goes LAST so a crash
    // before this line leaves the previous build live.
    put_with_hash(
        store,
        "tag-index.json",
        tag_index_body,
        "application/json".into(),
        Some("no-cache".into()),
        &mut uploaded,
        &mut skipped,
    )
    .await?;
    put_with_hash(
        store,
        "manifest.json",
        manifest_body,
        "application/json".into(),
        Some("no-cache".into()),
        &mut uploaded,
        &mut skipped,
    )
    .await?;

    let purged_tags = diff_tag_indices(prior_tag_index.as_ref(), &tag_index);
    if !purged_tags.is_empty() {
        purger.purge_tags(&purged_tags).await?;
    }

    Ok(PublishReport {
        build_id,
        uploaded_keys: uploaded,
        skipped_keys: skipped,
        purged_tags,
    })
}

/// Compute the set of CDN tags whose cached content the new publish
/// invalidates. A tag is included when:
///
/// - it appears in `next` but not `prior` (newly tracked), or
/// - it appears in `prior` but not `next` (route stopped depending on it —
///   prior HTML in the CDN is still tagged with it and needs to be evicted),
///   or
/// - it appears in both but the URL set changed (route content moved).
///
/// First publish (no prior) returns an empty set: nothing is cached yet, so
/// there's nothing to purge. Result is sorted + de-duplicated.
fn diff_tag_indices(prior: Option<&TagIndex>, next: &TagIndex) -> Vec<String> {
    let Some(prior) = prior else {
        return Vec::new();
    };
    let mut changed: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for (tag, urls) in &next.tags {
        match prior.tags.get(tag) {
            None => {
                changed.insert(tag.clone());
            }
            Some(prior_urls) if prior_urls != urls => {
                changed.insert(tag.clone());
            }
            _ => {}
        }
    }
    for tag in prior.tags.keys() {
        if !next.tags.contains_key(tag) {
            changed.insert(tag.clone());
        }
    }
    changed.into_iter().collect()
}

/// Repoint the root `/manifest.json` at a previously-retained build. Used for
/// rollback (`mesofact publish --pin <BUILD_ID>`). The currently-live
/// `/tag-index.json` is read *before* the swap; every tag it carries is then
/// passed to [`CdnPurger::purge_tags`] so the CDN evicts the about-to-be-stale
/// HTML keyed under those tags. (Tag invalidation works on the response, not
/// on its content hash — even if the pinned build maps the same tag to the
/// same URL, the cached body is the rolled-away-from build's and has to go.)
pub async fn publish_pin(
    build_id: &str,
    store: &dyn ObjectStore,
    purger: &dyn CdnPurger,
) -> Result<PublishReport, PublishError> {
    let manifest_key = format!("{build_id}/manifest.json");
    let tag_index_key = format!("{build_id}/tag-index.json");
    let manifest_body = store
        .get(&manifest_key)
        .await?
        .ok_or_else(|| PublishError::PinNotFound(build_id.to_string()))?;
    let tag_index_body = store
        .get(&tag_index_key)
        .await?
        .ok_or_else(|| PublishError::PinNotFound(build_id.to_string()))?;

    // Snapshot the currently-live tag-index *before* the swap so we know what
    // HTML the CDN may have cached under which tags. A malformed/absent live
    // index falls through to "nothing to purge" — pinning over a corrupted
    // pointer shouldn't refuse to recover.
    let live_tags: Vec<String> = match store.get("tag-index.json").await? {
        Some(bytes) => match serde_json::from_slice::<TagIndex>(&bytes) {
            Ok(idx) => idx.tags.into_keys().collect(),
            Err(_) => Vec::new(),
        },
        None => Vec::new(),
    };

    let mut uploaded: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    put_with_hash(
        store,
        "tag-index.json",
        tag_index_body,
        "application/json".into(),
        Some("no-cache".into()),
        &mut uploaded,
        &mut skipped,
    )
    .await?;
    put_with_hash(
        store,
        "manifest.json",
        manifest_body,
        "application/json".into(),
        Some("no-cache".into()),
        &mut uploaded,
        &mut skipped,
    )
    .await?;

    if !live_tags.is_empty() {
        purger.purge_tags(&live_tags).await?;
    }

    Ok(PublishReport {
        build_id: build_id.to_string(),
        uploaded_keys: uploaded,
        skipped_keys: skipped,
        purged_tags: live_tags,
    })
}

/// Hash-keyed idempotent PUT: skip the upload when the store already holds an
/// object at `key` whose `content_hash` matches the new body. `uploaded` and
/// `skipped` are appended in-place so the orchestrator can return a single
/// `PublishReport` covering every touched key.
async fn put_with_hash(
    store: &dyn ObjectStore,
    key: &str,
    body: Bytes,
    content_type: String,
    cache_control: Option<String>,
    uploaded: &mut Vec<String>,
    skipped: &mut Vec<String>,
) -> Result<(), PublishError> {
    let content_hash = sha256_hex(&body);
    if let Some(prior) = store.head(key).await? {
        if prior.content_hash == content_hash {
            skipped.push(key.to_string());
            return Ok(());
        }
    }
    store
        .put(
            key,
            body,
            PutOpts {
                content_type,
                content_hash,
                cache_control,
            },
        )
        .await?;
    uploaded.push(key.to_string());
    Ok(())
}

fn sha256_hex(body: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(body);
    hex::encode(hasher.finalize())
}

fn content_type_for(path: &Path) -> String {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        // The published Content-Type is what the CDN stores and replays, and
        // instantiateStreaming rejects anything but exactly application/wasm.
        // This walk hashes dist/ directly and never consults the manifest's
        // `static_assets[].content_type`, so the build-side arm alone would
        // still ship octet-stream to R2 (R821-B1).
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
    .into()
}

/// Per-path Cache-Control. assets/* and hydrate/* are content-hashed so they
/// can be immutable; html/* carries the route's **declared** `cache_policy`
/// when it has one and a long-but-purgeable TTL otherwise (CDN evicts by tag);
/// server/* is short-lived (re-uploaded each build, never user-visible).
///
/// R749-B4: the declared branch is the fix. Before it this function knew only
/// path prefixes, so a route declaring `{ ttl: 3600 }` was published at 86400 —
/// the serve tier honoured the number, the proxy tier honoured the number, and
/// the CDN copy of the same page silently did not. The prefix values stay as
/// the *default* for a page no route declared a policy for, which is every page
/// on a site that never writes `cache_policy`; nothing changes for them.
///
/// The content-hashed prefixes deliberately outrank the declaration: an
/// `assets/x.<hash>.js` URL cannot serve different bytes, so a route TTL on it
/// would be strictly worse than `immutable` and is not what the author meant by
/// putting a TTL on the *page*.
fn cache_control_for(rel: &str, policy: &CachePolicyTable) -> Option<String> {
    if rel.starts_with("assets/") || rel.starts_with("hydrate/") {
        return Some("public, max-age=31536000, immutable".into());
    }
    if let Some(path) = page_path_for(rel) {
        if let Some(declared) = policy.cache_control_for(&path) {
            return Some(declared.to_string());
        }
    }
    let cc = if rel.starts_with("html/") {
        "public, max-age=86400"
    } else {
        "public, max-age=3600"
    };
    Some(cc.into())
}

/// The public request path a `dist/` entry serves at, for the emissions that
/// have one — the inverse of `prerenderKey`
/// (`packages/mesofact-build/src/route-key.ts`, ported in
/// `crates/mesofact-render/src/route_key.rs`), which names each page by exactly
/// that path so the edge can find it from `url.pathname`:
///
/// | emitted | serves at |
/// |---|---|
/// | `html/index.html` | `/` |
/// | `html/releases.html` | `/releases` |
/// | `html/docs/index.html` | `/docs/` |
/// | `html/issues/abc.html` | `/issues/abc` |
///
/// `None` for everything else, which is the honest answer for the two shapes
/// `prerenderKey` deliberately does *not* name by path — an unexpanded SPA
/// shell and a wildcard route both keep the flattened `routeKey` name
/// (`item_id`), because neither serves at one path. Inverting `item_id` would
/// produce the path `/item_id`, which matches no route and would quietly select
/// the wrong policy if it ever did; a page whose name is not a path keeps the
/// prefix default. Non-page assets (`server/`, `hydrate/`) return `None` too.
fn page_path_for(rel: &str) -> Option<String> {
    let key = rel.strip_prefix("html/")?.strip_suffix(".html")?;
    if key == "index" {
        return Some("/".into());
    }
    match key.strip_suffix("/index") {
        Some(dir) => Some(format!("/{dir}/")),
        None => Some(format!("/{key}")),
    }
}

async fn walk_files(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut rd = fs::read_dir(&dir).await?;
        while let Some(entry) = rd.next_entry().await? {
            let ft = entry.file_type().await?;
            let p = entry.path();
            if ft.is_dir() {
                stack.push(p);
            } else if ft.is_file() {
                out.push(p);
            }
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{cache_control_for, content_type_for, page_path_for};
    use mesofact_core::CachePolicyTable;
    use std::path::Path;

    fn table(routes_json: &str) -> CachePolicyTable {
        CachePolicyTable::from_manifest_json(format!(r#"{{"routes":{routes_json}}}"#).as_bytes())
            .expect("fixture manifest parses")
    }

    /// The defect R749-B4 names: a declared TTL reached both servers and was
    /// dropped on the way to the CDN, so the page an author said to hold for an
    /// hour was published with a day on it.
    #[test]
    fn a_declared_ttl_reaches_the_published_object() {
        let t = table(r#"[{"route":"/issues","cache_policy":{"ttl":3600,"swr":86400}}]"#);
        assert_eq!(
            cache_control_for("html/issues.html", &t).as_deref(),
            Some("public, max-age=3600, stale-while-revalidate=86400"),
        );
    }

    /// A gated route's page must not be published shareable — a CDN holding it
    /// `public` serves one user's render to the next.
    #[test]
    fn a_gated_route_publishes_private() {
        let t = table(r#"[{"route":"/app","requires":["user"],"cache_policy":{"ttl":60}}]"#);
        assert_eq!(
            cache_control_for("html/app.html", &t).as_deref(),
            Some("private, max-age=60"),
        );
    }

    /// Every site that never writes `cache_policy` publishes exactly what it
    /// published before — the declaration is an override, not a new default.
    #[test]
    fn an_undeclared_page_keeps_the_prefix_default() {
        let t = table(r#"[{"route":"/plain","cache_policy":{"ttl":0}}]"#);
        assert_eq!(
            cache_control_for("html/plain.html", &t).as_deref(),
            Some("public, max-age=86400"),
        );
        assert_eq!(
            cache_control_for("server/plain.js", &t).as_deref(),
            Some("public, max-age=3600"),
        );
    }

    /// Content-hashed URLs stay immutable even when their route declares a TTL:
    /// the bytes at `assets/x.<hash>.js` can never change, so the page's TTL is
    /// not a statement about them.
    #[test]
    fn hashed_assets_outrank_a_declared_policy() {
        let t = table(r#"[{"route":"/","cache_policy":{"ttl":60}}]"#);
        for rel in ["assets/app.abc123.js", "hydrate/index.abc123.js"] {
            assert_eq!(
                cache_control_for(rel, &t).as_deref(),
                Some("public, max-age=31536000, immutable"),
                "{rel}",
            );
        }
    }

    /// Param instances inherit their pattern's policy — the whole point of
    /// inverting the emitted key back to a path rather than matching key text.
    #[test]
    fn a_param_instance_inherits_its_patterns_policy() {
        let t = table(r#"[{"route":"/issues/:id","cache_policy":{"ttl":120}}]"#);
        assert_eq!(
            cache_control_for("html/issues/abc.html", &t).as_deref(),
            Some("public, max-age=120"),
        );
    }

    #[test]
    fn page_keys_invert_to_the_path_they_serve_at() {
        assert_eq!(page_path_for("html/index.html").as_deref(), Some("/"));
        assert_eq!(
            page_path_for("html/releases.html").as_deref(),
            Some("/releases")
        );
        assert_eq!(
            page_path_for("html/docs/index.html").as_deref(),
            Some("/docs/")
        );
        assert_eq!(
            page_path_for("html/issues/abc.html").as_deref(),
            Some("/issues/abc")
        );
        // Not pages: no path to invert to.
        assert_eq!(page_path_for("assets/app.abc.js"), None);
        assert_eq!(page_path_for("html/wasm/demo_bg.wasm"), None);
        assert_eq!(page_path_for("server/index.js"), None);
    }

    /// A trailing-slash route (`/docs/` → `html/docs/index.html`) matches the
    /// manifest pattern `/docs`, because both sides drop empty segments.
    #[test]
    fn a_directory_index_matches_its_extensionless_pattern() {
        let t = table(r#"[{"route":"/docs","cache_policy":{"ttl":300}}]"#);
        assert_eq!(
            cache_control_for("html/docs/index.html", &t).as_deref(),
            Some("public, max-age=300"),
        );
    }

    /// The Content-Type we PUT is the one the CDN stores and replays forever,
    /// and `WebAssembly.instantiateStreaming` accepts exactly `application/wasm`
    /// — on anything else wasm-bindgen's loader warns and falls back to
    /// `arrayBuffer()` + `instantiate()`, downloading the whole module before
    /// compilation starts. This walk hashes `dist/` directly and never consults
    /// the manifest's `static_assets[].content_type`, so this table is the only
    /// thing standing between a `.wasm` and `application/octet-stream` in
    /// production (R821-B1).
    #[test]
    fn wasm_publishes_as_application_wasm() {
        assert_eq!(
            content_type_for(Path::new("dist/html/wasm/demo_bg.wasm")),
            "application/wasm"
        );
        assert_eq!(
            content_type_for(Path::new("dist/html/blob.bin")),
            "application/octet-stream"
        );
    }
}
