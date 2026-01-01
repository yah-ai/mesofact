// Source-inference pass (phase 3 of the build pipeline). For each route's
// `entrypoint`, scan the source for adapter factory calls — `r2('name')`,
// `sqlite('name')`, `pg('name')`, `rpc('name')` — and collect the literal
// names. The result populates the route's `source_reads` unless the user set
// it explicitly in `mesofact.routes.ts`.
//
// Overrides:
//   `// @mesofact-sources foo, bar` anywhere in the file replaces the inferred
//   set entirely. Use when static analysis can't follow the import (third-
//   party module re-exporting an adapter) or when a render reads a source
//   indirectly. The override is a single comma-separated list; multiple
//   directives concatenate.
//
// Scope: regex-based, single-file. Following the import graph is a P6+
// concern — the override comment exists for exactly this case.

import { readFileSync } from "node:fs";

const ADAPTER_NAMES = ["r2", "sqlite", "pg", "rpc"] as const;

// Match e.g. `r2('assets')` or `pg("project_db")`. The leading boundary
// rejects `myR2('assets')` and the like.
const ADAPTER_CALL = new RegExp(
  `(?:^|[^A-Za-z0-9_$])(${ADAPTER_NAMES.join("|")})\\s*\\(\\s*(['"\`])([^'"\`]+)\\2\\s*\\)`,
  "g",
);

const OVERRIDE = /\/\/\s*@mesofact-sources\s+([^\n\r]+)/g;

export type InferenceResult = {
  // Inferred (or override-supplied) source-name set, sorted.
  source_reads: readonly string[];
  // True when the result came from a `// @mesofact-sources …` directive.
  override: boolean;
};

export function inferFromSource(src: string): InferenceResult {
  const overrides: string[] = [];
  for (const m of src.matchAll(OVERRIDE)) {
    for (const name of m[1]!.split(",")) {
      const trimmed = name.trim();
      if (trimmed) overrides.push(trimmed);
    }
  }
  if (overrides.length > 0) {
    return { source_reads: dedupeSorted(overrides), override: true };
  }

  const names: string[] = [];
  for (const m of src.matchAll(ADAPTER_CALL)) {
    names.push(m[3]!);
  }
  return { source_reads: dedupeSorted(names), override: false };
}

export function inferFromFile(path: string): InferenceResult {
  return inferFromSource(readFileSync(path, "utf8"));
}

function dedupeSorted(names: string[]): readonly string[] {
  return [...new Set(names)].sort();
}
