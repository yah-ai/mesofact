//! Object-store abstraction. Models the read/write surface the publisher needs
//! against R2 (S3-compatible) without leaking S3 specifics into the orchestrator.
//!
//! The S3-backed impl lands in `R008-T7`; tests use [`InMemoryStore`].

use async_trait::async_trait;
use bytes::Bytes;
use std::collections::HashMap;
use std::sync::Mutex;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("transport error: {0}")]
    Transport(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Metadata returned by [`ObjectStore::head`]. The `content_hash` is the SHA-256
/// hex of the body, persisted as `x-amz-meta-content-hash` (or equivalent) on the
/// S3 side so idempotency checks don't have to re-download the object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectMeta {
    pub content_hash: String,
    pub content_type: String,
    pub size: u64,
}

#[derive(Debug, Clone, Default)]
pub struct PutOpts {
    pub content_type: String,
    /// SHA-256 hex of the body. The orchestrator computes this once and threads
    /// it into both the upload and (later) the idempotency-check head.
    pub content_hash: String,
    pub cache_control: Option<String>,
}

#[async_trait]
pub trait ObjectStore: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<Bytes>, StoreError>;
    async fn head(&self, key: &str) -> Result<Option<ObjectMeta>, StoreError>;
    async fn put(&self, key: &str, body: Bytes, opts: PutOpts) -> Result<(), StoreError>;
    async fn list(&self, prefix: &str) -> Result<Vec<String>, StoreError>;
    async fn delete(&self, key: &str) -> Result<(), StoreError>;
}

/// In-memory store for tests and the `--in-memory` CLI smoke. Drops everything
/// on process exit; production wiring goes through the S3 impl in `R008-T7`.
#[derive(Debug, Default)]
pub struct InMemoryStore {
    inner: Mutex<HashMap<String, (Bytes, ObjectMeta)>>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> Vec<String> {
        let mut keys: Vec<_> = self.inner.lock().unwrap().keys().cloned().collect();
        keys.sort();
        keys
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.lock().unwrap().is_empty()
    }
}

#[async_trait]
impl ObjectStore for InMemoryStore {
    async fn get(&self, key: &str) -> Result<Option<Bytes>, StoreError> {
        Ok(self.inner.lock().unwrap().get(key).map(|(b, _)| b.clone()))
    }

    async fn head(&self, key: &str) -> Result<Option<ObjectMeta>, StoreError> {
        Ok(self.inner.lock().unwrap().get(key).map(|(_, m)| m.clone()))
    }

    async fn put(&self, key: &str, body: Bytes, opts: PutOpts) -> Result<(), StoreError> {
        let meta = ObjectMeta {
            content_hash: opts.content_hash,
            content_type: opts.content_type,
            size: body.len() as u64,
        };
        self.inner
            .lock()
            .unwrap()
            .insert(key.to_string(), (body, meta));
        Ok(())
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>, StoreError> {
        let inner = self.inner.lock().unwrap();
        let mut keys: Vec<_> = inner
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect();
        keys.sort();
        Ok(keys)
    }

    async fn delete(&self, key: &str) -> Result<(), StoreError> {
        self.inner.lock().unwrap().remove(key);
        Ok(())
    }
}
