//! Keyed pointer store (parent-camp W270 §2) — the per-instance
//! generalization of the publisher's root pointer swap.
//!
//! `publish_dist` flips ONE pointer for the whole site (the root
//! `manifest.json`). Instance-addressed routes (`prerender: { deferred:
//! true }`) need the same primitive keyed finer: one pointer per minted
//! instance (a share slug → its render root). The rules carry over from the
//! atomic-release model: bytes are append-only and content-addressed, the
//! pointer is the only mutable object, unpublish is a pointer *tombstone*
//! (never byte deletion) so serving can distinguish "was published, now
//! gone" (410) from "never existed" (404).
//!
//! The object impl rides any [`ObjectStore`]: pointers are tiny JSON
//! records under a `p/` prefix, written with `no-cache` — the edge may
//! cache every content-addressed byte forever, but must revalidate
//! pointers. There is deliberately no listing surface in the API:
//! unguessable keys stay unguessable (the capability-URL property W215's
//! unlisted sharing relies on).
//!
//! The site-level root pointer is conceptually `key = ""` of this store;
//! unifying `publish_dist`'s swap onto it is deferred until the serving
//! worker consumes both through one code path (the parent camp's worker
//! artifact ticket) — repointing the live root-pointer wire format is that
//! ticket's call, not this one's.

use crate::{ObjectStore, PutOpts, StoreError};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Storage prefix for pointer records. Content roots live beside it
/// (`/{build_id}/...`, `a/<hash>`, ...) and never collide with `p/`.
pub const POINTER_PREFIX: &str = "p/";

/// Record schema version, bumped on breaking shape changes so a serving
/// worker can reject records it doesn't understand instead of misreading.
pub const POINTER_RECORD_V: u32 = 1;

#[derive(Debug, Error)]
pub enum PointerError {
    #[error("invalid pointer key {0:?}: {1}")]
    InvalidKey(String, &'static str),
    #[error("pointer record for {0:?} is malformed: {1}")]
    Malformed(String, String),
    #[error("store: {0}")]
    Store(#[from] StoreError),
}

/// What a live pointer names. `content_root` is the serving root (for a
/// published page: the render-root prefix the edge serves bytes from);
/// `source_root` optionally records the durable source bundle it was
/// derived from (W215's two-Merkle-roots split), so a renderer upgrade can
/// re-derive `content_root` without touching the pointer's identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pointer {
    pub content_root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_root: Option<String>,
    /// RFC3339, caller-stamped (the domain owns "when", not this store).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
}

/// Resolution outcome. `Deleted` exists so serving can answer 410 for
/// unpublished content instead of a generic 404.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PointerState {
    Present(Pointer),
    Deleted,
    Absent,
}

/// On-disk record: a pointer or its tombstone.
#[derive(Debug, Serialize, Deserialize)]
struct PointerRecord {
    v: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pointer: Option<Pointer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    deleted_at: Option<String>,
}

#[async_trait::async_trait]
pub trait PointerStore: Send + Sync {
    /// Atomically repoint `key` at `pointer`. Overwrites a prior pointer or
    /// tombstone; the flip is the commit point, exactly like the root
    /// manifest swap.
    async fn flip(&self, key: &str, pointer: Pointer) -> Result<(), PointerError>;
    /// Resolve `key` → live pointer / tombstone / never-existed.
    async fn resolve(&self, key: &str) -> Result<PointerState, PointerError>;
    /// Unpublish: replace the pointer with a tombstone (`deleted_at`
    /// caller-stamped, RFC3339). The content bytes are NOT touched — prune
    /// is a separate, explicit verb per the atomic-release model.
    async fn delete(&self, key: &str, deleted_at: Option<String>) -> Result<(), PointerError>;
}

/// [`PointerStore`] over any [`ObjectStore`] (R2 in prod, the in-memory /
/// dev-S3 stores locally).
pub struct ObjectPointerStore<S> {
    store: S,
}

impl<S: AsRef<dyn ObjectStore>> ObjectPointerStore<S> {
    pub fn new(store: S) -> Self {
        Self { store }
    }

    fn object_key(key: &str) -> Result<String, PointerError> {
        validate_key(key)?;
        Ok(format!("{POINTER_PREFIX}{key}"))
    }
}

/// Keys are minted by the domain (slugs), not user-echoed paths — but the
/// store still refuses shapes that could alias other storage areas.
fn validate_key(key: &str) -> Result<(), PointerError> {
    if key.is_empty() {
        return Err(PointerError::InvalidKey(
            key.into(),
            "empty key is reserved for the site root pointer (not yet unified onto this store)",
        ));
    }
    if key.starts_with('/') || key.ends_with('/') {
        return Err(PointerError::InvalidKey(key.into(), "leading/trailing '/'"));
    }
    if key.split('/').any(|seg| seg.is_empty() || seg == "." || seg == "..") {
        return Err(PointerError::InvalidKey(key.into(), "empty or dot path segment"));
    }
    if key.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(PointerError::InvalidKey(key.into(), "control or whitespace character"));
    }
    Ok(())
}

#[async_trait::async_trait]
impl<S: AsRef<dyn ObjectStore> + Send + Sync> PointerStore for ObjectPointerStore<S> {
    async fn flip(&self, key: &str, pointer: Pointer) -> Result<(), PointerError> {
        let object_key = Self::object_key(key)?;
        let record = PointerRecord { v: POINTER_RECORD_V, pointer: Some(pointer), deleted_at: None };
        put_record(self.store.as_ref(), &object_key, &record).await
    }

    async fn resolve(&self, key: &str) -> Result<PointerState, PointerError> {
        let object_key = Self::object_key(key)?;
        let Some(bytes) = self.store.as_ref().get(&object_key).await? else {
            return Ok(PointerState::Absent);
        };
        let record: PointerRecord = serde_json::from_slice(&bytes)
            .map_err(|e| PointerError::Malformed(key.into(), e.to_string()))?;
        if record.v != POINTER_RECORD_V {
            return Err(PointerError::Malformed(
                key.into(),
                format!("record version {} (this reader speaks {POINTER_RECORD_V})", record.v),
            ));
        }
        match record.pointer {
            Some(p) => Ok(PointerState::Present(p)),
            None => Ok(PointerState::Deleted),
        }
    }

    async fn delete(&self, key: &str, deleted_at: Option<String>) -> Result<(), PointerError> {
        let object_key = Self::object_key(key)?;
        let record = PointerRecord { v: POINTER_RECORD_V, pointer: None, deleted_at };
        put_record(self.store.as_ref(), &object_key, &record).await
    }
}

async fn put_record(
    store: &dyn ObjectStore,
    object_key: &str,
    record: &PointerRecord,
) -> Result<(), PointerError> {
    let body = serde_json::to_vec(record)
        .map_err(|e| PointerError::Malformed(object_key.into(), e.to_string()))?;
    let content_hash = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&body);
        hex::encode(hasher.finalize())
    };
    store
        .put(
            object_key,
            Bytes::from(body),
            PutOpts {
                content_type: "application/json".into(),
                content_hash,
                // The pointer is the ONLY mutable object — the edge must
                // revalidate it on every read.
                cache_control: Some("no-cache".into()),
            },
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InMemoryStore;
    use std::sync::Arc;

    fn store() -> ObjectPointerStore<Arc<dyn ObjectStore>> {
        ObjectPointerStore::new(Arc::new(InMemoryStore::new()) as Arc<dyn ObjectStore>)
    }

    fn ptr(root: &str) -> Pointer {
        Pointer {
            content_root: root.into(),
            source_root: Some(format!("src-of-{root}")),
            published_at: Some("2026-07-05T00:00:00Z".into()),
        }
    }

    #[tokio::test]
    async fn flip_then_resolve_round_trips() {
        let s = store();
        s.flip("abc123", ptr("r1")).await.unwrap();
        assert_eq!(s.resolve("abc123").await.unwrap(), PointerState::Present(ptr("r1")));
    }

    #[tokio::test]
    async fn reflip_repoints_atomically() {
        let s = store();
        s.flip("abc123", ptr("r1")).await.unwrap();
        s.flip("abc123", ptr("r2")).await.unwrap();
        assert_eq!(s.resolve("abc123").await.unwrap(), PointerState::Present(ptr("r2")));
    }

    #[tokio::test]
    async fn absent_vs_deleted_are_distinguishable() {
        let s = store();
        assert_eq!(s.resolve("never").await.unwrap(), PointerState::Absent);
        s.flip("gone", ptr("r1")).await.unwrap();
        s.delete("gone", Some("2026-07-05T01:00:00Z".into())).await.unwrap();
        assert_eq!(s.resolve("gone").await.unwrap(), PointerState::Deleted);
        // Re-publish over a tombstone is allowed (a new pointer resurrects it).
        s.flip("gone", ptr("r3")).await.unwrap();
        assert_eq!(s.resolve("gone").await.unwrap(), PointerState::Present(ptr("r3")));
    }

    #[tokio::test]
    async fn rejects_aliasing_keys() {
        let s = store();
        for bad in ["", "/abs", "trail/", "a//b", "a/../b", "a b", "a\nb", "."] {
            assert!(
                matches!(s.resolve(bad).await, Err(PointerError::InvalidKey(..))),
                "expected InvalidKey for {bad:?}"
            );
        }
        // Nested namespacing is fine (e.g. "chat/<slug>" if a consumer wants it).
        s.flip("chat/abc", ptr("r1")).await.unwrap();
        assert_eq!(s.resolve("chat/abc").await.unwrap(), PointerState::Present(ptr("r1")));
    }

    #[tokio::test]
    async fn unknown_record_version_is_loud() {
        let s = store();
        s.store
            .as_ref()
            .put(
                "p/vnext",
                Bytes::from(r#"{"v":99,"pointer":{"content_root":"x"}}"#),
                PutOpts {
                    content_type: "application/json".into(),
                    content_hash: "h".into(),
                    cache_control: None,
                },
            )
            .await
            .unwrap();
        assert!(matches!(s.resolve("vnext").await, Err(PointerError::Malformed(..))));
    }
}
