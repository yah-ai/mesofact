//! Authored route-table types (`mesofact.routes.ts` after evaluation) +
//! validation. Mirrors `packages/mesofact-runtime/src/routes.ts`: the shim's
//! `defineRoutes` is an identity inside the SSG runtime, so the authoring
//! rules are enforced here on the extracted JSON instead — same rules, same
//! build-failure semantics, different (earlier-vs-later) error site.

use anyhow::{bail, Result};
use mesofact::manifest::{
    CachePolicy, Prerender, Requires, ResiliencePolicy, RouteMode,
    DEFAULT_RESILIENCE_TIMEOUT_MS,
};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Placement {
    Host,
    Edge,
    Auto,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RouteEntry {
    pub route: String,
    pub mode: RouteMode,
    pub entrypoint: String,
    #[serde(default)]
    pub client_entrypoint: Option<String>,
    #[serde(default)]
    pub requires: Option<Vec<Requires>>,
    #[serde(default)]
    pub source_reads: Option<Vec<String>>,
    #[serde(default)]
    pub data_inputs: Option<Vec<String>>,
    pub cache_policy: CachePolicy,
    #[serde(default)]
    pub concurrency: Option<u32>,
    #[serde(default)]
    pub prerender: Option<Prerender>,
    #[serde(default)]
    pub placement: Option<Placement>,
    #[serde(default)]
    pub resilience: Option<ResiliencePolicy>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ErrorRoutes {
    #[serde(default, rename = "404")]
    pub not_found: Option<String>,
    #[serde(default, rename = "5xx")]
    pub server_error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RoutesConfig {
    pub routes: Vec<RouteEntry>,
    #[serde(default)]
    pub error_routes: Option<ErrorRoutes>,
    /// Origin for the manifest-derived sitemap (e.g. "https://yah.dev"). When
    /// present the build emits `dist/sitemap.xml`; absent skips it (W270 §4).
    #[serde(default)]
    pub site_url: Option<String>,
}

/// `defineRoutes` parity — every rule the TS runtime enforces at config
/// import time (placement on ssr only, from_data ⊆ data_inputs, W181
/// resilience shape).
pub fn validate_routes_config(config: &RoutesConfig) -> Result<()> {
    for r in &config.routes {
        if r.placement.is_some() && r.mode != RouteMode::Ssr {
            bail!(
                "route {} has placement but mode={:?}; placement is only valid on mode:\"ssr\"",
                r.route,
                r.mode
            );
        }
        if let Some(Prerender::FromData { from_data, .. }) = &r.prerender {
            let declared = r.data_inputs.clone().unwrap_or_default();
            if !declared.contains(from_data) {
                bail!(
                    "route {} has prerender.from_data={from_data:?} but that path is not in data_inputs ({declared:?}); declare the file in data_inputs first so the build reads it once",
                    r.route
                );
            }
        }
        if let Some(Prerender::Deferred { deferred }) = &r.prerender {
            if !deferred {
                bail!(
                    "route {}: prerender.deferred=false is meaningless — omit prerender (render once at build) or set deferred: true",
                    r.route
                );
            }
            if r.mode != RouteMode::Static {
                bail!(
                    "route {} has prerender.deferred but mode={:?}; deferred (publish-time) params are only valid on mode:\"static\" — ssr renders per request, spa shells are not instance-addressed",
                    r.route,
                    r.mode
                );
            }
            if !r.route.contains(':') {
                bail!(
                    "route {}: prerender.deferred requires a parametric route (a ':param' segment) — a literal route has exactly one instance, rendered at build",
                    r.route
                );
            }
        }
        if let Some(res) = &r.resilience {
            validate_resilience(r, res)?;
        }
        if r.mode == RouteMode::Spa && r.client_entrypoint.is_none() {
            bail!("route {}: mode 'spa' requires a client_entrypoint", r.route);
        }
    }
    Ok(())
}

fn validate_resilience(r: &RouteEntry, res: &ResiliencePolicy) -> Result<()> {
    if r.mode != RouteMode::Ssr {
        bail!(
            "route {} declares resilience but mode={:?}; resilience is only valid on mode:\"ssr\"",
            r.route,
            r.mode
        );
    }
    if r.placement == Some(Placement::Edge) {
        bail!(
            "route {} declares resilience on placement:\"edge\" — retrying the Worker from the Worker is circular (W181 OQ1)",
            r.route
        );
    }
    if res.queue.is_some() {
        bail!(
            "route {} declares resilience.queue — queue policy is reserved for v2 (W181 § \"v1 scope\")",
            r.route
        );
    }
    if let Some(t) = res.timeout_ms {
        if t == 0 {
            bail!("route {} has resilience.timeout_ms=0; expected a positive number", r.route);
        }
    }
    if let Some(retry) = &res.retry {
        if retry.attempts < 1 {
            bail!(
                "route {} has resilience.retry.attempts={}; expected an integer >= 1",
                r.route,
                retry.attempts
            );
        }
        if retry.backoff_ms.len() as u32 != retry.attempts - 1 {
            bail!(
                "route {} has resilience.retry.backoff_ms of length {}; expected attempts - 1 = {}",
                r.route,
                retry.backoff_ms.len(),
                retry.attempts - 1
            );
        }
        if let Some(retry_on) = &retry.retry_on {
            if !matches!(retry_on.as_str(), "connection" | "5xx" | "any") {
                bail!(
                    "route {} has resilience.retry.retry_on={retry_on:?}; expected \"connection\" | \"5xx\" | \"any\"",
                    r.route
                );
            }
        }
        if let Some(budget) = retry.budget_ms {
            let per_attempt = res.timeout_ms.unwrap_or(DEFAULT_RESILIENCE_TIMEOUT_MS);
            let floor: u64 =
                retry.backoff_ms.iter().sum::<u64>() + u64::from(retry.attempts) * per_attempt;
            if budget < floor {
                bail!(
                    "route {} has resilience.retry.budget_ms={budget} < {floor} (sum(backoff_ms) + attempts × per-attempt timeout)",
                    r.route
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(routes: serde_json::Value) -> RoutesConfig {
        serde_json::from_value(serde_json::json!({ "routes": routes })).unwrap()
    }

    #[test]
    fn deferred_prerender_valid_on_parametric_static() {
        let c = config(serde_json::json!([{
            "route": "/c/:slug",
            "mode": "static",
            "entrypoint": "src/c.ts",
            "cache_policy": { "ttl": 60 },
            "prerender": { "deferred": true },
        }]));
        assert!(matches!(c.routes[0].prerender, Some(Prerender::Deferred { deferred: true })));
        validate_routes_config(&c).expect("deferred on parametric static is valid");
    }

    #[test]
    fn deferred_prerender_rejected_off_static_or_literal_or_false() {
        let ssr = config(serde_json::json!([{
            "route": "/c/:slug",
            "mode": "ssr",
            "entrypoint": "src/c.ts",
            "cache_policy": { "ttl": 0 },
            "prerender": { "deferred": true },
        }]));
        let err = validate_routes_config(&ssr).unwrap_err().to_string();
        assert!(err.contains("only valid on mode:\"static\""), "err: {err}");

        let literal = config(serde_json::json!([{
            "route": "/about",
            "mode": "static",
            "entrypoint": "src/about.ts",
            "cache_policy": { "ttl": 60 },
            "prerender": { "deferred": true },
        }]));
        let err = validate_routes_config(&literal).unwrap_err().to_string();
        assert!(err.contains("parametric route"), "err: {err}");

        let falsy = config(serde_json::json!([{
            "route": "/c/:slug",
            "mode": "static",
            "entrypoint": "src/c.ts",
            "cache_policy": { "ttl": 60 },
            "prerender": { "deferred": false },
        }]));
        let err = validate_routes_config(&falsy).unwrap_err().to_string();
        assert!(err.contains("deferred=false is meaningless"), "err: {err}");
    }
}
