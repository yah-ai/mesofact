//! Minimal `mesofact.config.toml` reader for the build pipeline: the
//! `[sources]` catalog (validator input) and `[build] public_dir`
//! (R490-F4). Adapter wiring stays in the TS runtime / proxy — the
//! Rust-native pipeline only needs names + scopes.

use anyhow::{bail, Context, Result};
use mesofact::validate::{SourceCatalog, SourceScope};
use std::path::Path;

pub struct BuildConfig {
    pub catalog: SourceCatalog,
    pub public_dir: String,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self { catalog: SourceCatalog::new(), public_dir: crate::assets::DEFAULT_PUBLIC_DIR.into() }
    }
}

pub fn load_config(project_root: &Path) -> Result<BuildConfig> {
    let path = project_root.join("mesofact.config.toml");
    if !path.exists() {
        return Ok(BuildConfig::default());
    }
    let raw = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let parsed: toml::Value = raw.parse().with_context(|| format!("parsing {}", path.display()))?;

    let mut out = BuildConfig::default();
    if let Some(build) = parsed.get("build") {
        if let Some(pd) = build.get("public_dir") {
            let Some(s) = pd.as_str().filter(|s| !s.is_empty()) else {
                bail!("[build] public_dir must be a non-empty string");
            };
            out.public_dir = s.to_string();
        }
    }
    if let Some(sources) = parsed.get("sources").and_then(|s| s.as_table()) {
        for (name, body) in sources {
            let scope = match body.get("scope").and_then(|s| s.as_str()).unwrap_or("global") {
                "global" => SourceScope::Global,
                "project" => SourceScope::Project,
                "user" => SourceScope::User,
                other => bail!("[sources.{name}] unknown scope {other:?}"),
            };
            out.catalog.insert(name.clone(), scope);
        }
    }
    Ok(out)
}
