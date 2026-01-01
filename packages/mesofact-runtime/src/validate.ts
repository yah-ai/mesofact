// Manifest validator — structural shape + semantic rules. Used by the build
// (fail before bad HTML hits R2) and mirrored in `crates/mesofact/src/validate.rs`
// for the proxy. Shared fixtures live at `tests/fixtures/manifests/`.
//
// Semantic rules:
//   R1. Mode 1 (static) + `source_reads` referencing a non-`global` source
//       is rejected. Mode 1 builds can't enumerate per-user/per-project keys.
//   R2. Mode 1 + `requires` containing "user" is rejected. The build can't
//       know who the user is.
//
// See `.yah/docs/architecture/mesofact.md` §"Render axis × source axis".

import type {
  Manifest,
  ManifestRoute,
  ManifestStaticAsset,
  ManifestErrorRoutes,
} from "./manifest.js";
import { MANIFEST_VERSION } from "./manifest.js";

export type SourceScope = "global" | "project" | "user";

export type SourceCatalog = Record<string, { scope: SourceScope }>;

export type ValidationErrorKind =
  | "shape"
  | "unsupported_version"
  | "mode1_scoped_source"
  | "mode1_requires_user";

export type ValidationError = {
  kind: ValidationErrorKind;
  path: string;
  message: string;
};

export type ValidationResult =
  | { ok: true; manifest: Manifest }
  | { ok: false; errors: ValidationError[] };

const MODES = new Set(["static", "ssr", "spa"]);
const REQUIRES = new Set(["user", "project", "region"]);
const RESOLVED_PLACEMENTS = new Set(["host", "edge"]);

function isObject(x: unknown): x is Record<string, unknown> {
  return typeof x === "object" && x !== null && !Array.isArray(x);
}

function shape(path: string, message: string): ValidationError {
  return { kind: "shape", path, message };
}

function checkRoute(idx: number, raw: unknown, errs: ValidationError[]): ManifestRoute | null {
  const base = `routes[${idx}]`;
  if (!isObject(raw)) {
    errs.push(shape(base, "expected object"));
    return null;
  }

  const out: Partial<ManifestRoute> = {};

  if (typeof raw.route !== "string") errs.push(shape(`${base}.route`, "expected string"));
  else out.route = raw.route;

  if (typeof raw.mode !== "string" || !MODES.has(raw.mode)) {
    errs.push(shape(`${base}.mode`, "expected 'static' | 'ssr' | 'spa'"));
  } else {
    out.mode = raw.mode as ManifestRoute["mode"];
  }

  if (typeof raw.render_entrypoint !== "string") {
    errs.push(shape(`${base}.render_entrypoint`, "expected string"));
  } else {
    out.render_entrypoint = raw.render_entrypoint;
  }

  if (raw.requires !== undefined) {
    if (!Array.isArray(raw.requires) || raw.requires.some((r) => typeof r !== "string" || !REQUIRES.has(r))) {
      errs.push(shape(`${base}.requires`, "expected ('user' | 'project' | 'region')[]"));
    } else {
      out.requires = raw.requires as ManifestRoute["requires"];
    }
  }

  if (raw.source_reads !== undefined) {
    if (!Array.isArray(raw.source_reads) || raw.source_reads.some((s) => typeof s !== "string")) {
      errs.push(shape(`${base}.source_reads`, "expected string[]"));
    } else {
      out.source_reads = raw.source_reads as readonly string[];
    }
  }

  if (raw.data_inputs !== undefined) {
    if (!Array.isArray(raw.data_inputs) || raw.data_inputs.some((s) => typeof s !== "string")) {
      errs.push(shape(`${base}.data_inputs`, "expected string[]"));
    } else {
      out.data_inputs = raw.data_inputs as readonly string[];
    }
  }

  if (!isObject(raw.cache_policy) || typeof raw.cache_policy.ttl !== "number") {
    errs.push(shape(`${base}.cache_policy`, "expected { ttl: number, ... }"));
  } else {
    out.cache_policy = raw.cache_policy as ManifestRoute["cache_policy"];
  }

  if (raw.concurrency !== undefined) {
    if (typeof raw.concurrency !== "number") {
      errs.push(shape(`${base}.concurrency`, "expected number"));
    } else {
      out.concurrency = raw.concurrency;
    }
  }

  if (raw.hydration !== undefined) {
    const h = raw.hydration;
    if (!isObject(h) || typeof h.script !== "string" || !Array.isArray(h.code_split)) {
      errs.push(shape(`${base}.hydration`, "expected { script: string, code_split: string[] }"));
    } else {
      out.hydration = h as ManifestRoute["hydration"];
    }
  }

  if (raw.prerender !== undefined) {
    if (!isObject(raw.prerender)) {
      errs.push(shape(`${base}.prerender`, "expected object"));
    } else {
      out.prerender = raw.prerender as ManifestRoute["prerender"];
    }
  }

  if (raw.placement !== undefined) {
    if (typeof raw.placement !== "string" || !RESOLVED_PLACEMENTS.has(raw.placement)) {
      // The build resolves `"auto"` to a concrete value before emission, so
      // the manifest only ever carries `"host"` or `"edge"`.
      errs.push(shape(`${base}.placement`, "expected 'host' | 'edge'"));
    } else if (out.mode !== undefined && out.mode !== "ssr") {
      errs.push(shape(`${base}.placement`, "placement is only valid on mode:'ssr'"));
    } else {
      out.placement = raw.placement as ManifestRoute["placement"];
    }
  }

  if (errs.length && errs.some((e) => e.path.startsWith(base))) return null;
  return out as ManifestRoute;
}

function checkStaticAsset(idx: number, raw: unknown, errs: ValidationError[]): ManifestStaticAsset | null {
  const base = `static_assets[${idx}]`;
  if (!isObject(raw)) {
    errs.push(shape(base, "expected object"));
    return null;
  }
  if (
    typeof raw.key !== "string" ||
    typeof raw.content_hash !== "string" ||
    typeof raw.content_type !== "string" ||
    typeof raw.immutable !== "boolean"
  ) {
    errs.push(shape(base, "expected { key, content_hash, content_type, immutable }"));
    return null;
  }
  return {
    key: raw.key,
    content_hash: raw.content_hash,
    content_type: raw.content_type,
    immutable: raw.immutable,
  };
}

function checkErrorRoutes(raw: unknown, errs: ValidationError[]): ManifestErrorRoutes | undefined {
  if (raw === undefined) return undefined;
  if (!isObject(raw)) {
    errs.push(shape("error_routes", "expected object"));
    return undefined;
  }
  const out: ManifestErrorRoutes = {};
  if (raw["404"] !== undefined) {
    if (typeof raw["404"] !== "string") errs.push(shape("error_routes.404", "expected string"));
    else out["404"] = raw["404"];
  }
  if (raw["5xx"] !== undefined) {
    if (typeof raw["5xx"] !== "string") errs.push(shape("error_routes.5xx", "expected string"));
    else out["5xx"] = raw["5xx"];
  }
  return out;
}

function checkRules(manifest: Manifest, catalog: SourceCatalog, errs: ValidationError[]): void {
  manifest.routes.forEach((route, idx) => {
    if (route.mode !== "static") return;

    if (route.source_reads) {
      for (const name of route.source_reads) {
        const entry = catalog[name];
        if (!entry) {
          errs.push({
            kind: "shape",
            path: `routes[${idx}].source_reads`,
            message: `route ${route.route}: source '${name}' not declared in catalog`,
          });
          continue;
        }
        if (entry.scope !== "global") {
          errs.push({
            kind: "mode1_scoped_source",
            path: `routes[${idx}].source_reads`,
            message: `route ${route.route}: Mode 1 cannot read from non-'global' source '${name}' (scope='${entry.scope}')`,
          });
        }
      }
    }

    if (route.requires?.includes("user")) {
      errs.push({
        kind: "mode1_requires_user",
        path: `routes[${idx}].requires`,
        message: `route ${route.route}: Mode 1 cannot require 'user' (the build can't enumerate users)`,
      });
    }
  });
}

export function validate(input: unknown, catalog: SourceCatalog = {}): ValidationResult {
  const errs: ValidationError[] = [];

  if (!isObject(input)) {
    return { ok: false, errors: [shape("$", "expected object")] };
  }

  if (input.version !== MANIFEST_VERSION) {
    errs.push({
      kind: "unsupported_version",
      path: "version",
      message: `unsupported manifest version '${String(input.version)}' (expected '${MANIFEST_VERSION}')`,
    });
  }

  if (typeof input.build_id !== "string") errs.push(shape("build_id", "expected string"));

  if (!Array.isArray(input.routes)) {
    errs.push(shape("routes", "expected array"));
    return { ok: false, errors: errs };
  }

  const routes: ManifestRoute[] = [];
  input.routes.forEach((r, i) => {
    const route = checkRoute(i, r, errs);
    if (route) routes.push(route);
  });

  const staticAssetsRaw = input.static_assets ?? [];
  if (!Array.isArray(staticAssetsRaw)) {
    errs.push(shape("static_assets", "expected array"));
    return { ok: false, errors: errs };
  }
  const staticAssets: ManifestStaticAsset[] = [];
  staticAssetsRaw.forEach((a, i) => {
    const asset = checkStaticAsset(i, a, errs);
    if (asset) staticAssets.push(asset);
  });

  const errorRoutes = checkErrorRoutes(input.error_routes, errs);

  let ssrPrefixes: readonly string[] | undefined;
  if (input.ssr_prefixes !== undefined) {
    if (!Array.isArray(input.ssr_prefixes) || input.ssr_prefixes.some((s) => typeof s !== "string")) {
      errs.push(shape("ssr_prefixes", "expected string[]"));
    } else {
      ssrPrefixes = input.ssr_prefixes as readonly string[];
    }
  }

  if (errs.length) return { ok: false, errors: errs };

  const manifest: Manifest = {
    version: input.version as Manifest["version"],
    build_id: input.build_id as string,
    routes,
    static_assets: staticAssets,
    ...(errorRoutes ? { error_routes: errorRoutes } : {}),
    ...(ssrPrefixes !== undefined ? { ssr_prefixes: ssrPrefixes } : {}),
  };

  checkRules(manifest, catalog, errs);

  if (errs.length) return { ok: false, errors: errs };
  return { ok: true, manifest };
}
