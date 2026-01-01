// Build a `SourceCatalog` (name → { scope }) for the manifest validator from
// the parsed `mesofact.config.toml`. P4 only ships the `r2` kind, so this is
// trivial today; widening lands as future adapters appear.

import { existsSync } from "node:fs";
import { parseConfig, type MesofactConfig, type SourceCatalog } from "@mesofact/runtime";
import { readFileSync } from "node:fs";

export function catalogFromConfig(config: MesofactConfig): SourceCatalog {
  const out: SourceCatalog = {};
  for (const [name, src] of Object.entries(config.sources)) {
    out[name] = { scope: src.scope };
  }
  return out;
}

// Load `mesofact.config.toml` if present; return an empty catalog otherwise.
// A project with no scoped sources legitimately has no config file.
export function loadCatalog(configPath: string): SourceCatalog {
  if (!existsSync(configPath)) return {};
  const config = parseConfig(readFileSync(configPath, "utf8"));
  return catalogFromConfig(config);
}

// Load the full parsed config (or null when no file). Used by the CLI to
// register adapters before `build()` runs source-derived prerender queries.
export function loadConfigOpt(configPath: string): MesofactConfig | null {
  if (!existsSync(configPath)) return null;
  return parseConfig(readFileSync(configPath, "utf8"));
}
