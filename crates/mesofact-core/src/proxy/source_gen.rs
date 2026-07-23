//! Source generation tokens for cache-key input 6.
//!
//! The proxy folds each read source's *current generation* into the Mode 2
//! cache key so a backend bump is an automatic miss (no manual purge). Per
//! §"Cache-key composition", generations come from:
//!
//! | source | token | refresh |
//! |---|---|---|
//! | `sqlite` (global) | file mtime | cached 1s |
//! | `r2` | bucket/object `Last-Modified` | cached 5s |
//! | `pg` / `rpc` | LSN / roster token | (post-MVP) |
//!
//! MVP implements the sqlite mtime path (the P9 slice's source). Other kinds
//! return a stable placeholder until their poll lands — a stable token is safe
//! (it just means generation never advances on its own; TTL still expires).
//!
//! The proxy caches each token with a 1s TTL so a burst of misses doesn't
//! amplify into N filesystem stats / backend pings.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant, UNIX_EPOCH};

const GENERATION_TTL: Duration = Duration::from_secs(1);

/// One declared source, parsed from `[sources.<name>]` in `mesofact.config.toml`.
/// Only the fields the generation poll needs are kept.
#[derive(Debug, Clone)]
pub struct SourceDef {
    pub kind: String,
    pub path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawConfig {
    #[serde(default)]
    sources: HashMap<String, RawSource>,
}

#[derive(Debug, Deserialize)]
struct RawSource {
    kind: String,
    #[serde(default)]
    path: Option<String>,
}

/// Generation provider: maps source name → current generation token, with a
/// 1s memo so repeated cache-key composition is cheap.
pub struct Generations {
    defs: HashMap<String, SourceDef>,
    memo: Mutex<HashMap<String, (String, Instant)>>,
}

impl Generations {
    /// Empty provider — every source resolves to the placeholder token. Used
    /// when no `mesofact.config.toml` is configured.
    pub fn empty() -> Self {
        Self { defs: HashMap::new(), memo: Mutex::new(HashMap::new()) }
    }

    /// Parse `[sources.*]` from a `mesofact.config.toml` string.
    pub fn from_config_str(toml_str: &str) -> Result<Self, toml::de::Error> {
        let raw: RawConfig = toml::from_str(toml_str)?;
        let defs = raw
            .sources
            .into_iter()
            .map(|(name, s)| (name, SourceDef { kind: s.kind, path: s.path }))
            .collect();
        Ok(Self { defs, memo: Mutex::new(HashMap::new()) })
    }

    /// Load from a config file path. A missing file yields an empty provider —
    /// a project with no scoped sources legitimately ships no config.
    pub fn from_config_file(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(s) => Ok(Self::from_config_str(&s)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::empty()),
            Err(e) => Err(e.into()),
        }
    }

    /// Current generation token for `name`, memoized for 1s. Unknown sources
    /// (not in config) resolve to the stable placeholder.
    pub fn token(&self, name: &str) -> String {
        let now = Instant::now();
        {
            let memo = self.memo.lock().unwrap();
            if let Some((tok, at)) = memo.get(name) {
                if now.saturating_duration_since(*at) < GENERATION_TTL {
                    return tok.clone();
                }
            }
        }
        let fresh = self.compute(name);
        self.memo.lock().unwrap().insert(name.to_string(), (fresh.clone(), now));
        fresh
    }

    fn compute(&self, name: &str) -> String {
        let Some(def) = self.defs.get(name) else {
            return PLACEHOLDER.to_string();
        };
        match def.kind.as_str() {
            "sqlite" => def
                .path
                .as_deref()
                .map(mtime_token)
                .unwrap_or_else(|| PLACEHOLDER.to_string()),
            // r2/pg/rpc polling is post-MVP — a stable token keeps the key
            // correct (TTL still drives expiry); it just never self-advances.
            _ => PLACEHOLDER.to_string(),
        }
    }
}

const PLACEHOLDER: &str = "0";

/// File mtime as a nanosecond token, or `"missing"` when the file is absent.
/// A non-existent DB resolves stably; once the file appears its mtime token
/// changes, busting the key.
fn mtime_token(path: &str) -> String {
    match std::fs::metadata(path).and_then(|m| m.modified()) {
        Ok(t) => t
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos().to_string())
            .unwrap_or_else(|_| "0".to_string()),
        Err(_) => "missing".to_string(),
    }
}

/// Convenience: a `SystemTime` formatted the same way `mtime_token` does, for
/// tests that want to assert against a known mtime.
#[cfg(test)]
fn token_of(t: std::time::SystemTime) -> String {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_nanos().to_string()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::SystemTime;

    #[test]
    fn unknown_source_is_placeholder() {
        let g = Generations::empty();
        assert_eq!(g.token("whatever"), PLACEHOLDER);
    }

    #[test]
    fn parses_sources_and_keeps_kind_and_path() {
        let g = Generations::from_config_str(
            r#"
            [sources.project_db]
            kind = "sqlite"
            scope = "global"
            path = "/tmp/x.db"

            [sources.assets]
            kind = "r2"
            scope = "global"
            bucket = "b"
            endpoint_env = "R2_ENDPOINT"
            "#,
        )
        .unwrap();
        assert_eq!(g.defs.get("project_db").unwrap().kind, "sqlite");
        assert_eq!(g.defs.get("project_db").unwrap().path.as_deref(), Some("/tmp/x.db"));
        // r2 resolves to placeholder for MVP.
        assert_eq!(g.token("assets"), PLACEHOLDER);
    }

    #[test]
    fn sqlite_token_tracks_file_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("project.db");
        let mut f = std::fs::File::create(&db).unwrap();
        f.write_all(b"v1").unwrap();
        f.sync_all().unwrap();

        let cfg = format!(
            "[sources.project_db]\nkind = \"sqlite\"\nscope = \"global\"\npath = \"{}\"\n",
            db.display()
        );
        let g = Generations::from_config_str(&cfg).unwrap();

        let t1 = g.token("project_db");
        assert_ne!(t1, "missing");
        // Same instant within the 1s memo window → identical token.
        assert_eq!(g.token("project_db"), t1);

        // Bump the mtime past the memo window and confirm the token advances.
        let later = SystemTime::now() + Duration::from_secs(5);
        let f2 = std::fs::OpenOptions::new().write(true).open(&db).unwrap();
        f2.set_modified(later).unwrap();
        std::thread::sleep(Duration::from_millis(1100));
        let t2 = g.token("project_db");
        assert_ne!(t1, t2, "token should follow the new mtime");
        assert_eq!(t2, token_of(later));
    }

    #[test]
    fn missing_sqlite_file_is_stable_missing_token() {
        let g = Generations::from_config_str(
            "[sources.project_db]\nkind = \"sqlite\"\npath = \"/no/such/file.db\"\n",
        )
        .unwrap();
        assert_eq!(g.token("project_db"), "missing");
    }
}
