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

use crate::{CdnPurger, ObjectStore, PurgeError, PutOpts, StoreError};
use bytes::Bytes;
use mesofact::Manifest;
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
            cache_control_for(&rel_str),
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
        _ => "application/octet-stream",
    }
    .into()
}

/// Per-path Cache-Control. assets/* and hydrate/* are content-hashed so they
/// can be immutable; html/* gets a long-but-purgeable TTL (CDN evicts by tag);
/// server/* is short-lived (re-uploaded each build, never user-visible).
fn cache_control_for(rel: &str) -> Option<String> {
    let cc = if rel.starts_with("assets/") || rel.starts_with("hydrate/") {
        "public, max-age=31536000, immutable"
    } else if rel.starts_with("html/") {
        "public, max-age=86400"
    } else {
        "public, max-age=3600"
    };
    Some(cc.into())
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
