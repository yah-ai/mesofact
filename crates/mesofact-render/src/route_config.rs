//! Authored route-table types (`mesofact.routes.ts` after evaluation) +
//! validation. Mirrors `packages/mesofact-runtime/src/routes.ts`: the shim's
//! `defineRoutes` is an identity inside the SSG runtime, so the authoring
//! rules are enforced here on the extracted JSON instead — same rules, same
//! build-failure semantics, different (earlier-vs-later) error site.

use anyhow::{bail, Result};
use mesofact_core::manifest::{
    CachePolicy, Prerender, Requires, ResiliencePolicy, RouteMode,
    DEFAULT_RESILIENCE_TIMEOUT_MS,
};
use serde::Deserialize;
use std::collections::BTreeMap;

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
    /// Mode 2 endpoint callbacks (W311 §2 / R756-F6): hook name → entrypoint
    /// path. A hook is engine-addressed, not path-addressed — Rust decides
    /// when to call it and owns the HTTP around it — so it is declared here
    /// beside `routes` rather than inside one. `BTreeMap` for a deterministic
    /// bundle order; the set of legal names is [`HOOK_NAMES`].
    #[serde(default)]
    pub hooks: Option<BTreeMap<String, String>>,
}

/// Every Mode 2 hook name the engine knows how to invoke (R756-F6). Mirrors
/// `HOOK_NAMES` in `packages/mesofact-runtime/src/hooks.ts`; the set is closed
/// because only the engine invokes hooks, so a name it does not know would
/// never be called.
pub const HOOK_NAMES: &[&str] = &["readyz"];

/// Hooks that may *alternatively* be declared by claiming a route, which is
/// how `/readyz` shipped (R756-F5) before hooks existed. Declaring both is
/// rejected — two opt-ins for one verdict is ambiguous, not additive.
pub const HOOK_ROUTE_CLAIMS: &[(&str, &str)] = &[("readyz", "/readyz")];

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
    if let Some(hooks) = &config.hooks {
        validate_hooks(config, hooks)?;
    }
    Ok(())
}

/// R756-F6 — the `hooks` block. Same rules `defineRoutes` enforces in TS.
fn validate_hooks(config: &RoutesConfig, hooks: &BTreeMap<String, String>) -> Result<()> {
    for (name, entrypoint) in hooks {
        if !HOOK_NAMES.contains(&name.as_str()) {
            bail!(
                "unknown hook {name:?} — known hooks are {}. A hook name is engine-defined: only \
                 mesofact invokes hooks, so a name it does not know would never be called.",
                HOOK_NAMES.join(", ")
            );
        }
        if entrypoint.trim().is_empty() {
            bail!(
                "hooks.{name} must be a non-empty entrypoint path relative to the project root \
                 (e.g. \"src/{name}.ts\")"
            );
        }
        if let Some((_, claimed)) = HOOK_ROUTE_CLAIMS.iter().find(|(h, _)| *h == name) {
            if config.routes.iter().any(|r| r.route == *claimed) {
                bail!(
                    "hook {name:?} is declared twice — as hooks.{name} and by claiming the route \
                     {claimed:?}. Both mean \"this app contributes a {name} verdict\"; pick one. \
                     The hooks declaration is usually the one you want — the module stays out of \
                     ssr_prefixes, so the edge never forwards {claimed} to the SSR origin and the \
                     Rust probe route never shadows it."
                );
            }
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

    // ── R756-F6: the Mode 2 hook declaration site ────────────────────────────
    //
    // The Rust-native pipeline is the sole production build path and its
    // `defineRoutes` shim is an identity, so these rules are only enforced
    // here — the TS `defineRoutes` throws for `bun test`, not for real builds.

    fn config_with_hooks(routes: serde_json::Value, hooks: serde_json::Value) -> RoutesConfig {
        serde_json::from_value(serde_json::json!({ "routes": routes, "hooks": hooks })).unwrap()
    }

    fn static_route() -> serde_json::Value {
        serde_json::json!({
            "route": "/",
            "mode": "static",
            "entrypoint": "src/home.ts",
            "cache_policy": { "ttl": 60 },
        })
    }

    #[test]
    fn a_declared_hook_is_accepted() {
        let c = config_with_hooks(
            serde_json::json!([static_route()]),
            serde_json::json!({ "readyz": "src/readyz.ts" }),
        );
        validate_routes_config(&c).expect("a known hook name is valid");
    }

    #[test]
    fn an_unknown_hook_name_is_rejected() {
        let c = config_with_hooks(
            serde_json::json!([static_route()]),
            serde_json::json!({ "onRequest": "src/on_request.ts" }),
        );
        let err = validate_routes_config(&c).unwrap_err().to_string();
        assert!(err.contains("unknown hook"), "err: {err}");
        assert!(err.contains("onRequest"), "err should name the offender: {err}");
    }

    #[test]
    fn an_empty_hook_entrypoint_is_rejected() {
        let c = config_with_hooks(
            serde_json::json!([static_route()]),
            serde_json::json!({ "readyz": "   " }),
        );
        let err = validate_routes_config(&c).unwrap_err().to_string();
        assert!(err.contains("non-empty entrypoint"), "err: {err}");
    }

    /// Two opt-ins for one verdict is ambiguous, not additive — and silently
    /// preferring one would leave the other looking wired up when it is not.
    #[test]
    fn declaring_readyz_twice_is_rejected() {
        let c = config_with_hooks(
            serde_json::json!([
                static_route(),
                {
                    "route": "/readyz",
                    "mode": "ssr",
                    "entrypoint": "src/readyz.ts",
                    "cache_policy": { "ttl": 0 },
                }
            ]),
            serde_json::json!({ "readyz": "src/readyz.ts" }),
        );
        let err = validate_routes_config(&c).unwrap_err().to_string();
        assert!(err.contains("declared twice"), "err: {err}");
    }

    /// The pre-R756-F6 opt-in still stands on its own.
    #[test]
    fn claiming_the_readyz_route_alone_is_still_valid() {
        let c = config(serde_json::json!([{
            "route": "/readyz",
            "mode": "ssr",
            "entrypoint": "src/readyz.ts",
            "cache_policy": { "ttl": 0 },
        }]));
        validate_routes_config(&c).expect("route-claim opt-in still works");
    }
}
