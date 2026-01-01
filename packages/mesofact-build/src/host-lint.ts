// Server/client module boundary lint — W173 § "Server/client module
// boundary lint" + Placement validation. Runs a discovery-only Bun.build
// over a target entrypoint with an onResolve plugin that records every
// import; any specifier matching the forbidden list becomes a build error
// naming the importer. Disposable when RSC's "use client" lands (same idea,
// finer granularity).
//
// We don't bundle for the actual target (browser vs workerd) here — only
// the import graph matters. Bun's bundler silently passes node:* through a
// browser-target bundle (verified empirically), so this lint isn't
// redundant with the existing Bun.build call.

import type { BunPlugin } from "bun";
import { BuildError } from "./load-routes.js";

// Either a RegExp matched against the raw specifier, or an exact string match.
export type ForbiddenMatcher = RegExp | string;

// Bare-name node builtins. Not exhaustive — extend when a false negative
// surfaces. `node:` prefix forms are caught by the regex below.
const NODE_BUILTIN_NAMES = [
  "fs",
  "path",
  "os",
  "net",
  "tls",
  "crypto",
  "child_process",
  "dgram",
  "dns",
  "http",
  "https",
  "http2",
  "stream",
  "url",
  "worker_threads",
  "cluster",
  "v8",
  "vm",
  "zlib",
  "module",
  "perf_hooks",
  "readline",
  "repl",
  "tty",
  "process",
  "buffer",
  "querystring",
  "string_decoder",
  "timers",
  "events",
] as const;

// Browser-bundled code (spa client_entrypoint) cannot reach any of these.
export const BROWSER_FORBIDDEN: readonly ForbiddenMatcher[] = [
  /^node:/,
  ...NODE_BUILTIN_NAMES,
];

// SSR + placement:"edge" runs inside workerd, which has no node builtins
// and can't link native db drivers. Inherits the browser list + flags
// common host-only db clients. Configurable list per W173; v1 starts with
// the obvious offenders and grows when a real case surfaces.
export const EDGE_FORBIDDEN: readonly ForbiddenMatcher[] = [
  ...BROWSER_FORBIDDEN,
  "better-sqlite3",
  "pg",
  "mysql2",
  "mysql",
  "mongodb",
  "redis",
  "ioredis",
];

export type Violation = {
  // Absolute path of the file that issued the import.
  importer: string;
  // The raw specifier (e.g. `"node:fs"` or `"pg"`).
  specifier: string;
};

function matches(specifier: string, m: ForbiddenMatcher): boolean {
  if (typeof m === "string") return specifier === m;
  return m.test(specifier);
}

function isForbidden(specifier: string, forbidden: readonly ForbiddenMatcher[]): boolean {
  for (const m of forbidden) {
    if (matches(specifier, m)) return true;
  }
  return false;
}

export async function detectForbiddenImports(
  absEntry: string,
  target: "browser" | "bun",
  forbidden: readonly ForbiddenMatcher[],
): Promise<readonly Violation[]> {
  const violations: Violation[] = [];
  const seen = new Set<string>();
  const plugin: BunPlugin = {
    name: "mesofact-host-lint",
    setup(build) {
      build.onResolve({ filter: /.*/ }, (args) => {
        if (isForbidden(args.path, forbidden)) {
          const key = `${args.importer}\0${args.path}`;
          if (!seen.has(key)) {
            seen.add(key);
            violations.push({ importer: args.importer, specifier: args.path });
          }
        }
        return null;
      });
    },
  };

  // No `outdir` → Bun returns artifacts in memory and writes nothing. A
  // forbidden import that's not installed in node_modules (e.g. `pg` in a
  // fixture) makes Bun.build throw at resolution; the plugin still recorded
  // the violation before the throw, and that's the more actionable error to
  // surface — so we swallow the bundle failure when we have violations.
  let bundleError: unknown;
  try {
    await Bun.build({
      entrypoints: [absEntry],
      target,
      format: "esm",
      splitting: false,
      sourcemap: "none",
      plugins: [plugin],
    });
  } catch (e) {
    bundleError = e;
  }

  if (violations.length === 0 && bundleError) throw bundleError;
  return violations;
}

export async function assertNoForbiddenImports(opts: {
  route: string;
  absEntry: string;
  target: "browser" | "bun";
  forbidden: readonly ForbiddenMatcher[];
  // Short noun describing what the entrypoint is — e.g. "client_entrypoint"
  // or `ssr placement:"edge" entrypoint`. Surfaced in the error message.
  kind: string;
}): Promise<void> {
  const violations = await detectForbiddenImports(opts.absEntry, opts.target, opts.forbidden);
  if (violations.length === 0) return;
  const lines = violations.map(
    (v) => `  - ${v.importer} imports ${JSON.stringify(v.specifier)}`,
  );
  throw new BuildError(
    `route ${opts.route}: ${opts.kind} pulls in host-only API(s):\n${lines.join("\n")}`,
  );
}
