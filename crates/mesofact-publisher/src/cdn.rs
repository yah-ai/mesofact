//! CDN purge abstraction. The publisher emits a set of cache tags that were
//! invalidated by this publish (Mode 1 routes whose content changed); a
//! [`CdnPurger`] turns those into provider-specific purge calls.
//!
//! The Cloudflare-backed impl lands in `R008-T7`; tests use [`InMemoryPurger`].

use async_trait::async_trait;
use std::sync::Mutex;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PurgeError {
    #[error("transport error: {0}")]
    Transport(String),
}

#[async_trait]
pub trait CdnPurger: Send + Sync {
    async fn purge_tags(&self, tags: &[String]) -> Result<(), PurgeError>;
}

/// Records every `purge_tags` call so tests can assert exactly which tags the
/// orchestrator surfaced.
#[derive(Debug, Default)]
pub struct InMemoryPurger {
    calls: Mutex<Vec<Vec<String>>>,
}

impl InMemoryPurger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every call's tag list, in order.
    pub fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().unwrap().clone()
    }

    /// Union of every tag passed to `purge_tags`, sorted + de-duplicated.
    pub fn flat_tags(&self) -> Vec<String> {
        let mut tags: Vec<_> = self
            .calls
            .lock()
            .unwrap()
            .iter()
            .flatten()
            .cloned()
            .collect();
        tags.sort();
        tags.dedup();
        tags
    }
}

#[async_trait]
impl CdnPurger for InMemoryPurger {
    async fn purge_tags(&self, tags: &[String]) -> Result<(), PurgeError> {
        if !tags.is_empty() {
            self.calls.lock().unwrap().push(tags.to_vec());
        }
        Ok(())
    }
}
