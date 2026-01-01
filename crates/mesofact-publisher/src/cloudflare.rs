//! Cloudflare-backed [`CdnPurger`]. POSTs `{"tags": [...]}` against
//! `https://api.cloudflare.com/client/v4/zones/{zone_id}/purge_cache` with a
//! Bearer API token. Cloudflare caps per-call tag count at 30, so the tag set
//! is chunked.
//!
//! Real-network exercise is gated on the CI smoke job; local `cargo test`
//! uses [`InMemoryPurger`](crate::cdn::InMemoryPurger).

use async_trait::async_trait;
use reqwest::{header::HeaderValue, Client};
use serde::Serialize;

use crate::cdn::{CdnPurger, PurgeError};

const PURGE_TAG_CHUNK: usize = 30;
const API_BASE: &str = "https://api.cloudflare.com/client/v4";

pub struct CloudflareCdnPurger {
    client: Client,
    zone_id: String,
    api_token: String,
    api_base: String,
}

impl std::fmt::Debug for CloudflareCdnPurger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudflareCdnPurger")
            .field("zone_id", &self.zone_id)
            .field("api_token", &"<redacted>")
            .field("api_base", &self.api_base)
            .finish()
    }
}

impl CloudflareCdnPurger {
    pub fn new(zone_id: impl Into<String>, api_token: impl Into<String>) -> Result<Self, PurgeError> {
        let client = Client::builder()
            .build()
            .map_err(|e| PurgeError::Transport(format!("reqwest build: {e}")))?;
        Ok(Self {
            client,
            zone_id: zone_id.into(),
            api_token: api_token.into(),
            api_base: API_BASE.to_string(),
        })
    }

    /// Test seam — point at a stub URL instead of the real Cloudflare API.
    #[cfg(test)]
    pub fn with_api_base(mut self, base: impl Into<String>) -> Self {
        self.api_base = base.into();
        self
    }
}

#[derive(Serialize)]
struct PurgeBody<'a> {
    tags: &'a [String],
}

#[async_trait]
impl CdnPurger for CloudflareCdnPurger {
    async fn purge_tags(&self, tags: &[String]) -> Result<(), PurgeError> {
        if tags.is_empty() {
            return Ok(());
        }
        let auth = HeaderValue::from_str(&format!("Bearer {}", self.api_token))
            .map_err(|e| PurgeError::Transport(format!("invalid token: {e}")))?;
        let url = format!("{}/zones/{}/purge_cache", self.api_base, self.zone_id);
        for chunk in tags.chunks(PURGE_TAG_CHUNK) {
            let body = PurgeBody { tags: chunk };
            let res = self
                .client
                .post(&url)
                .header(reqwest::header::AUTHORIZATION, auth.clone())
                .json(&body)
                .send()
                .await
                .map_err(|e| PurgeError::Transport(format!("POST {url}: {e}")))?;
            if !res.status().is_success() {
                let status = res.status();
                let body = res.text().await.unwrap_or_default();
                return Err(PurgeError::Transport(format!(
                    "purge_cache → HTTP {status}: {body}"
                )));
            }
        }
        Ok(())
    }
}
