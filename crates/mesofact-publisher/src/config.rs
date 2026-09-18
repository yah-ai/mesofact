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
    #[error("missing env var {var} (or {var}_FILE) referenced by mesofact.config.toml {field}")]
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
    /// Optional base key prefix — every published object lands under this
    /// prefix within `bucket`, so one bucket can host several surfaces (e.g.
    /// `yah-marketing/cloud`). Absent → publish at bucket root. Threaded into
    /// [`crate::s3::S3Store::with_base_prefix`] by the publish/revalidate paths.
    #[serde(default)]
    pub prefix: Option<String>,
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
        let publish = parsed
            .publish
            .ok_or_else(|| ConfigError::MissingPublish(path.display().to_string()))?;
        publish.reject_placeholders(path)?;
        Ok(publish)
    }

    /// Refuse a config still carrying the template's literal placeholders.
    ///
    /// This has now been a live landmine three times — `app/yah/web/marketing`
    /// (R330-T35), `app/yah/web/analytics` (R556-F6) and `app/yah/web/dashboard`
    /// (R891-B6) — and each one survived for months because the failure is
    /// *deferred*: `publish` only purges when the cache-tag diff is non-empty,
    /// so every content-identical publish skips the purge and reports success.
    /// The first publish that actually changes a file POSTs to
    /// `zones/ZONE_ID/purge_cache`, takes a 400, and fails the run — after the
    /// objects have already been uploaded, which is the worst moment to learn.
    ///
    /// Failing at load turns a deferred, half-applied publish into an immediate
    /// refusal that names the field. Placeholders stay correct in a *template*;
    /// they are only wrong in a config something tries to publish with, and
    /// this is the exact seam between the two.
    fn reject_placeholders(&self, path: &Path) -> Result<(), ConfigError> {
        const PLACEHOLDERS: &[&str] = &["ACCOUNT_ID", "ZONE_ID", "BUCKET_NAME"];
        for (field, value) in [
            ("endpoint", &self.endpoint),
            ("zone_id", &self.zone_id),
            ("bucket", &self.bucket),
        ] {
            if let Some(found) = PLACEHOLDERS.iter().find(|p| value.contains(**p)) {
                return Err(ConfigError::Parse(format!(
                    "{}: [publish].{field} still holds the template placeholder \
                     `{found}` — substitute the real value before publishing. \
                     Neither the Cloudflare account id nor the zone id is a \
                     secret; both are readable from the Cloudflare dashboard.",
                    path.display()
                )));
            }
        }
        Ok(())
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

/// Resolves a credential by NAME, value-first. If `<name>` is unset but
/// `<name>_FILE` is, reads and trims the file it points to — the mount-path
/// delivery form used by the SecretMount + SecretTarget::File path (R876-B16),
/// so a receiver that can read either form is safe regardless of which one
/// the sender emits.
fn env_required(name: &str, field: &str) -> Result<String, ConfigError> {
    if let Ok(v) = std::env::var(name) {
        return Ok(v);
    }
    let file_var = format!("{name}_FILE");
    if let Ok(path) = std::env::var(&file_var) {
        let contents = std::fs::read_to_string(&path).map_err(|_| ConfigError::MissingEnv {
            var: name.to_string(),
            field: field.to_string(),
        })?;
        return Ok(contents.trim().to_string());
    }
    Err(ConfigError::MissingEnv {
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

    /// R891-B6: the template placeholders must fail at load, not at the first
    /// content-changing publish. Three separate configs shipped with these
    /// literals and each was found by hand months later.
    #[tokio::test]
    async fn load_rejects_unsubstituted_placeholders() {
        for (field, toml) in [
            (
                "endpoint",
                "[publish]\nbucket=\"b\"\nendpoint=\"https://ACCOUNT_ID.r2.cloudflarestorage.com\"\nzone_id=\"deadbeef\"\n",
            ),
            (
                "zone_id",
                "[publish]\nbucket=\"b\"\nendpoint=\"https://acct.r2.cloudflarestorage.com\"\nzone_id=\"ZONE_ID\"\n",
            ),
        ] {
            let dir = tempdir().unwrap();
            let path = dir.path().join("mesofact.config.toml");
            tokio::fs::write(&path, toml).await.unwrap();
            let err = PublishConfig::load(&path)
                .await
                .expect_err("placeholder must be refused");
            let msg = err.to_string();
            assert!(msg.contains(field), "error should name the field: {msg}");
        }
    }

    #[tokio::test]
    async fn prefix_is_optional_and_parses() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mesofact.config.toml");
        // Absent → None (publish at bucket root; back-compat).
        tokio::fs::write(&path, "[publish]\nbucket=\"b\"\nendpoint=\"e\"\nzone_id=\"z\"\n")
            .await
            .unwrap();
        assert!(PublishConfig::load(&path).await.unwrap().prefix.is_none());
        // Present → Some.
        tokio::fs::write(
            &path,
            "[publish]\nbucket=\"b\"\nendpoint=\"e\"\nzone_id=\"z\"\nprefix=\"yah-marketing/cloud\"\n",
        )
        .await
        .unwrap();
        assert_eq!(
            PublishConfig::load(&path).await.unwrap().prefix.as_deref(),
            Some("yah-marketing/cloud")
        );
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

    #[test]
    fn env_required_falls_back_to_file_when_value_var_unset() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("secret.txt");
        std::fs::write(&path, "sentinel-value\n").unwrap();
        let name = "MESOFACT_TEST_CRED_ENV_REQUIRED_FALLBACK";
        let file_var = format!("{name}_FILE");
        // SAFETY: test-local env vars, no other test reads this name.
        unsafe {
            std::env::remove_var(name);
            std::env::set_var(&file_var, &path);
        }
        let result = env_required(name, "test_field");
        unsafe {
            std::env::remove_var(&file_var);
        }
        assert_eq!(result.unwrap(), "sentinel-value");
    }

    #[test]
    fn env_required_prefers_value_var_over_file() {
        let name = "MESOFACT_TEST_CRED_ENV_REQUIRED_PREFERENCE";
        let file_var = format!("{name}_FILE");
        // SAFETY: test-local env vars, no other test reads this name.
        unsafe {
            std::env::set_var(name, "direct-value");
            std::env::set_var(&file_var, "/nonexistent/path/should/not/be/read");
        }
        let result = env_required(name, "test_field");
        unsafe {
            std::env::remove_var(name);
            std::env::remove_var(&file_var);
        }
        assert_eq!(result.unwrap(), "direct-value");
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
