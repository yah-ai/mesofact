//! Manifest types — the single document the build emits and the proxy boots
//! from. Mirrors `packages/mesofact-runtime/src/manifest.ts`. Schema spec lives
//! in `.yah/docs/architecture/mesofact.md` §"Manifest schema".

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Current manifest schema version. Major bumps force a proxy restart.
pub const MANIFEST_VERSION: &str = "1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RouteMode {
    Static,
    Ssr,
    Spa,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

/// SSR placement as carried in the manifest — the build resolves `"auto"`
/// to a concrete value before emission, so consumers never see `auto`.
/// Mirrors `ResolvedPlacement` in `packages/mesofact-runtime/src/manifest.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResolvedPlacement {
    Host,
    Edge,
}

/// W181 resilience axis (v1: retry + timeout). Applied by the always-up
/// edge (CF Worker in prod, the mesofact-dev proxy in dev) around the SSR
/// origin hop. `queue` is type-reserved for v2; `defineRoutes` rejects it
/// today, so a well-formed manifest never carries it — the slot exists so
/// v1 binaries keep deserializing when v2 manifests appear.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Total attempts including the first; 1 = no retry.
    pub attempts: u32,
    /// Gap before attempt i+1; `len() == attempts - 1`.
    pub backoff_ms: Vec<u64>,
    /// `"connection"` (default) | `"5xx"` | `"any"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_on: Option<String>,
    /// Total wall-clock cap across request + retries + backoffs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuePolicy {
    pub queue: String,
    pub ack: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_delay_ms: Option<u64>,
}

/// Default per-attempt timeout when `resilience.timeout_ms` is omitted.
pub const DEFAULT_RESILIENCE_TIMEOUT_MS: u64 = 30_000;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ResiliencePolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue: Option<QueuePolicy>,
    /// Per-attempt request timeout; default [`DEFAULT_RESILIENCE_TIMEOUT_MS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// Mode 1 only. Three shapes the publisher runs at build time, plus one
/// that defers past the build entirely:
///   - `Literal` — explicit list of param maps
///   - `SourceDerived` — a registered source adapter (R2 BlobSource) walked
///     via async load
///   - `FromData` — a local JSON file declared on the same route's
///     `data_inputs`, walked synchronously via `items_key` (dotted path)
///   - `Deferred` — params are minted after the build (publish time); the
///     build emits the server bundle + manifest entry and prerenders
///     nothing. Instances are produced exclusively through the render-only
///     entrypoint, and serving resolves them per instance (the route is
///     *instance-addressed* — W225 §3a publish-once / parent-camp W270 §2).
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
    Deferred {
        /// Always `true` in a well-formed config (`{ deferred: true }`);
        /// `false` is rejected at validation.
        deferred: bool,
    },
}

impl Prerender {
    /// Instance-addressed routes render after the build, one instance per
    /// minted param set; serving resolves them through a pointer store
    /// rather than the build-time HTML set.
    pub fn is_deferred(&self) -> bool {
        matches!(self, Prerender::Deferred { deferred: true })
    }
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
    /// Build-time data artifact paths (relative to project root). Present
    /// when the route declared `data_inputs`; the reconciler uses it to map
    /// file changes to route rebuilds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_inputs: Option<Vec<String>>,
    pub cache_policy: CachePolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hydration: Option<Hydration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prerender: Option<Prerender>,
    /// SSR routes only; never `auto` (resolved at build time per W173).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<ResolvedPlacement>,
    /// SSR routes only (W181); see [`ResiliencePolicy`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resilience: Option<ResiliencePolicy>,
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
    /// Derived from every `mode:"ssr"` route per W173 § "SSR_PREFIXES
    /// derivation rule". Segment-aware match at the consumer:
    /// `path == p || path.starts_with(&format!("{p}/"))`. Absent when the
    /// workload has no SSR routes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssr_prefixes: Option<Vec<String>>,
}
