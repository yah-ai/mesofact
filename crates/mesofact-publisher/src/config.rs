//! `[publish]` section loader for `mesofact.config.toml`. Resolves the
//! S3-compatible bucket/endpoint/region and the Cloudflare zone the publisher
//! talks to. Credentials never live in the file — they're pulled from env
//! vars named in the config (`access_key_id_env`, `secret_access_key_env`,
//! `api_token_env`) and threaded into the adapters by the CLI.

use serde::Deserialize;
use std::path::Path;
use thiserror::Error;
use tokio::fs;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("config file not found: {0}")]
    NotFound(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse: {0}")]
    Parse(String),
    #[error("missing [publish] section in {0} — declare it to enable real-network publish, or pass --in-memory")]
    MissingPublish(String),
    #[error("missing env var {var} referenced by mesofact.config.toml {field}")]
    MissingEnv { var: String, field: String },
}

/// Top-level shape we read from `mesofact.config.toml`. We only deserialize
/// what the publisher needs; other sections (`sources`, etc.) are ignored.
#[derive(Debug, Deserialize)]
struct ConfigFile {
    publish: Option<PublishConfig>,
}

/// `[publish]` block in `mesofact.config.toml`. Field semantics:
///
/// - `bucket` / `endpoint` / `region` (default `"auto"`) — S3-compatible
///   target for [`crate::s3::S3Store`]. `endpoint` is the root *without* the
///   bucket path (e.g. `https://<account>.r2.cloudflarestorage.com`).
/// - `zone_id` — Cloudflare zone for tag purges.
/// - `*_env` — env var names holding credentials. The config file itself
///   never contains secrets.
#[derive(Debug, Clone, Deserialize)]
pub struct PublishConfig {
    pub bucket: String,
    pub endpoint: String,
    #[serde(default = "default_region")]
    pub region: String,
    pub zone_id: String,
    #[serde(default = "default_access_key_env")]
    pub access_key_id_env: String,
    #[serde(default = "default_secret_key_env")]
    pub secret_access_key_env: String,
    #[serde(default = "default_api_token_env")]
    pub api_token_env: String,
}

fn default_region() -> String {
    "auto".into()
}
fn default_access_key_env() -> String {
    "MESOFACT_S3_ACCESS_KEY_ID".into()
}
fn default_secret_key_env() -> String {
    "MESOFACT_S3_SECRET_ACCESS_KEY".into()
}
fn default_api_token_env() -> String {
    "CLOUDFLARE_API_TOKEN".into()
}

/// Resolved credentials. Kept separate from [`PublishConfig`] so the config
/// can be loaded without immediately requiring the env vars (the CLI uses
/// the split to error with a precise hint when only the creds are missing).
#[derive(Debug, Clone)]
pub struct PublishCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub cloudflare_api_token: String,
}

impl PublishConfig {
    /// Load the `[publish]` block from `mesofact.config.toml` at `path`.
    /// Returns `MissingPublish` when the file exists but the block is absent
    /// — the CLI uses that to fall back to a clear error message.
    pub async fn load(path: &Path) -> Result<Self, ConfigError> {
        if !fs::try_exists(path).await? {
            return Err(ConfigError::NotFound(path.display().to_string()));
        }
        let body = fs::read_to_string(path).await?;
        let parsed: ConfigFile = toml::from_str(&body)
            .map_err(|e| ConfigError::Parse(format!("{}: {e}", path.display())))?;
        parsed
            .publish
            .ok_or_else(|| ConfigError::MissingPublish(path.display().to_string()))
    }

    /// Apply CLI flag overrides — non-`None` values win over the file.
    pub fn with_overrides(
        mut self,
        bucket: Option<String>,
        endpoint: Option<String>,
        zone: Option<String>,
    ) -> Self {
        if let Some(b) = bucket {
            self.bucket = b;
        }
        if let Some(e) = endpoint {
            self.endpoint = e;
        }
        if let Some(z) = zone {
            self.zone_id = z;
        }
        self
    }

    /// Read the env vars named in the config; precise per-field error on miss.
    pub fn resolve_credentials(&self) -> Result<PublishCredentials, ConfigError> {
        Ok(PublishCredentials {
            access_key_id: env_required(&self.access_key_id_env, "access_key_id_env")?,
            secret_access_key: env_required(&self.secret_access_key_env, "secret_access_key_env")?,
            cloudflare_api_token: env_required(&self.api_token_env, "api_token_env")?,
        })
    }
}

fn env_required(name: &str, field: &str) -> Result<String, ConfigError> {
    std::env::var(name).map_err(|_| ConfigError::MissingEnv {
        var: name.to_string(),
        field: field.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn load_publish_block_with_defaults() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mesofact.config.toml");
        let toml = r#"
[publish]
bucket = "yah-dev-site"
endpoint = "https://acct.r2.cloudflarestorage.com"
zone_id = "deadbeef"
"#;
        tokio::fs::write(&path, toml).await.unwrap();
        let cfg = PublishConfig::load(&path).await.unwrap();
        assert_eq!(cfg.bucket, "yah-dev-site");
        assert_eq!(cfg.region, "auto");
        assert_eq!(cfg.access_key_id_env, "MESOFACT_S3_ACCESS_KEY_ID");
        assert_eq!(cfg.api_token_env, "CLOUDFLARE_API_TOKEN");
    }

    #[tokio::test]
    async fn missing_publish_block_is_typed_error() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mesofact.config.toml");
        tokio::fs::write(&path, "[sources.foo]\nkind=\"r2\"\nbucket=\"x\"\nendpoint=\"y\"\n")
            .await
            .unwrap();
        let err = PublishConfig::load(&path).await.unwrap_err();
        assert!(matches!(err, ConfigError::MissingPublish(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn overrides_replace_file_values() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mesofact.config.toml");
        let toml = r#"
[publish]
bucket = "from-file"
endpoint = "https://from-file"
zone_id = "from-file"
"#;
        tokio::fs::write(&path, toml).await.unwrap();
        let cfg = PublishConfig::load(&path).await.unwrap().with_overrides(
            Some("cli-bucket".into()),
            None,
            Some("cli-zone".into()),
        );
        assert_eq!(cfg.bucket, "cli-bucket");
        assert_eq!(cfg.endpoint, "https://from-file");
        assert_eq!(cfg.zone_id, "cli-zone");
    }
}
