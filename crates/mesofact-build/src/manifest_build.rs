//! Manifest assembly — port of `packages/mesofact-build/src/manifest-build.ts`.
//! Builds the `mesofact_core::manifest::Manifest` from the authored routes +
//! bundle outputs, then runs the crate validator (same R1/R2 rules the TS
//! validate() applies).

use anyhow::{anyhow, bail, Result};
use mesofact_core::manifest::{
    Hydration, Manifest, Route, RouteMode, StaticAsset, MANIFEST_VERSION,
};
use mesofact_core::validate::SourceCatalog;
use std::collections::BTreeMap;

use crate::route_config::{ErrorRoutes, Placement, RouteEntry, RoutesConfig};
use crate::ssr_prefix::derive_ssr_prefixes;

pub struct AssembleInput<'a> {
    pub routes: &'a RoutesConfig,
    pub build_id: &'a str,
    /// route → `dist/server/<key>.js`
    pub server_paths: &'a BTreeMap<String, String>,
    /// route → inferred or overridden source_reads
    pub inferred_sources: &'a BTreeMap<String, Vec<String>>,
    /// route → client bundle (hydration block)
    pub hydration: &'a BTreeMap<String, Hydration>,
    pub static_assets: Vec<StaticAsset>,
    pub catalog: &'a SourceCatalog,
}

pub fn assemble_manifest(input: AssembleInput<'_>) -> Result<Manifest> {
    let mut routes = Vec::with_capacity(input.routes.routes.len());
    for entry in &input.routes.routes {
        routes.push(build_route(entry, &input)?);
    }
    let ssr_prefixes = derive_ssr_prefixes(&input.routes.routes);

    let manifest = Manifest {
        version: MANIFEST_VERSION.to_string(),
        build_id: input.build_id.to_string(),
        routes,
        static_assets: input.static_assets,
        error_routes: input.routes.error_routes.as_ref().map(to_manifest_error_routes),
        ssr_prefixes: (!ssr_prefixes.is_empty()).then_some(ssr_prefixes),
    };

    if let Err(errors) = mesofact_core::validate::validate(&manifest, input.catalog) {
        let detail = errors
            .iter()
            .map(|e| format!("  - [{}] {}", e.kind.label(), e))
            .collect::<Vec<_>>()
            .join("\n");
        bail!("manifest validation failed:\n{detail}");
    }
    Ok(manifest)
}

fn to_manifest_error_routes(e: &ErrorRoutes) -> mesofact_core::manifest::ErrorRoutes {
    mesofact_core::manifest::ErrorRoutes {
        not_found: e.not_found.clone(),
        server_error: e.server_error.clone(),
    }
}

fn build_route(entry: &RouteEntry, input: &AssembleInput<'_>) -> Result<Route> {
    let server_path = input
        .server_paths
        .get(&entry.route)
        .ok_or_else(|| anyhow!("route {}: no bundled entrypoint", entry.route))?;

    let source_reads = match &entry.source_reads {
        Some(explicit) => explicit.clone(),
        None => input.inferred_sources.get(&entry.route).cloned().unwrap_or_default(),
    };

    // `auto`/undefined placement resolves to host today (W173 § "auto
    // resolution"); only emitted for ssr routes.
    let placement = (entry.mode == RouteMode::Ssr).then(|| match entry.placement {
        Some(Placement::Edge) => mesofact_core::manifest::ResolvedPlacement::Edge,
        _ => mesofact_core::manifest::ResolvedPlacement::Host,
    });

    Ok(Route {
        route: entry.route.clone(),
        mode: entry.mode,
        render_entrypoint: server_path.clone(),
        requires: entry.requires.clone(),
        source_reads: (!source_reads.is_empty()).then_some(source_reads),
        data_inputs: entry
            .data_inputs
            .as_ref()
            .filter(|d| !d.is_empty())
            .cloned(),
        cache_policy: entry.cache_policy.clone(),
        concurrency: entry.concurrency,
        hydration: input.hydration.get(&entry.route).cloned(),
        prerender: entry.prerender.clone(),
        placement,
        resilience: entry.resilience.clone(),
    })
}
