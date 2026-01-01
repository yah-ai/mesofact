//! Manifest types — the single document the build emits and the proxy boots
//! from. Mirrors `packages/mesofact-runtime/src/manifest.ts`. Schema spec lives
//! in `.yah/docs/architecture/mesofact.md` §"Manifest schema".

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Current manifest schema version. Major bumps force a proxy restart.
pub const MANIFEST_VERSION: &str = "1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RouteMode {
    Static,
    Ssr,
    Spa,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Requires {
    User,
    Project,
    Region,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachePolicy {
    pub ttl: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swr: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub negative_ttl: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vary: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hydration {
    pub script: String,
    pub code_split: Vec<String>,
}

/// Mode 1 only. Three shapes the publisher runs at build time:
///   - `Literal` — explicit list of param maps
///   - `SourceDerived` — a registered source adapter (R2 BlobSource) walked
///     via async load
///   - `FromData` — a local JSON file declared on the same route's
///     `data_inputs`, walked synchronously via `items_key` (dotted path)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Prerender {
    Literal {
        params: Vec<BTreeMap<String, String>>,
    },
    SourceDerived {
        from: String,
        query: String,
        param: String,
    },
    FromData {
        from_data: String,
        items_key: String,
        param: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Route {
    pub route: String,
    pub mode: RouteMode,
    pub render_entrypoint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires: Option<Vec<Requires>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_reads: Option<Vec<String>>,
    pub cache_policy: CachePolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hydration: Option<Hydration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prerender: Option<Prerender>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaticAsset {
    pub key: String,
    pub content_hash: String,
    pub content_type: String,
    pub immutable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ErrorRoutes {
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "404")]
    pub not_found: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "5xx")]
    pub server_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: String,
    pub build_id: String,
    pub routes: Vec<Route>,
    #[serde(default)]
    pub static_assets: Vec<StaticAsset>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_routes: Option<ErrorRoutes>,
}
