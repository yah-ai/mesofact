// Load the user-authored `mesofact.routes.ts`. We dynamic-import the file
// directly — Bun transpiles TS on the fly, so no separate compile step is
// needed before discovery. The expected shape is a default export (or named
// `routes`) of `RoutesConfig`.

import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import type { RoutesConfig } from "@mesofact/runtime";

export type LoadedRoutes = {
  config: RoutesConfig;
  // Absolute path of the loaded file, used to anchor relative `entrypoint`s.
  path: string;
};

export async function loadRoutes(routesFile: string): Promise<LoadedRoutes> {
  const path = resolve(routesFile);
  const url = pathToFileURL(path).href;
  const mod = (await import(url)) as Record<string, unknown>;
  const candidate = mod.default ?? mod.routes ?? mod.config;
  if (!isRoutesConfig(candidate)) {
    throw new BuildError(
      `${routesFile}: expected default (or named 'routes') export of RoutesConfig — got ${stringifyKind(candidate)}`,
    );
  }
  return { config: candidate, path };
}

function isRoutesConfig(v: unknown): v is RoutesConfig {
  if (typeof v !== "object" || v === null) return false;
  const obj = v as Record<string, unknown>;
  return Array.isArray(obj.routes);
}

function stringifyKind(v: unknown): string {
  if (v === null) return "null";
  if (Array.isArray(v)) return "array";
  return typeof v;
}

export class BuildError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "BuildError";
  }
}
