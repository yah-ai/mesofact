//! @yah:ticket(R012-T1, "Mode 3 build: client-tree bundling → dist/hydrate/, shell prerender with __MESOFACT_STATE__ + hydrate-script injection, manifest hydration population")
//! @yah:at(2026-05-26T16:04:16Z)
//! @yah:status(review)
//! @yah:assignee(agent:claude)
//! @yah:phase(P10)
//! @yah:parent(R012)
//! @yah:handoff("Mode 3 build tree shipped end-to-end (TS). (1) bundle.ts: bundleClientEntrypoints() bundles each spa route's client_entrypoint to dist/hydrate/ — browser target, ESM, splitting:true, content-hashed names (entry '<key>.[hash].js', chunks '<key>.chunk-[hash].js'). Returns {script, code_split} (basenames). (2) routes.ts: RouteEntry gains client_entrypoint?; build throws if a spa route omits it or a non-spa route sets it. (3) manifest-build.ts: AssembleInput.hydration map (route→ManifestHydration) populates ManifestRoute.hydration for spa routes. (4) prerender.ts: now prerenders static AND spa (skips ssr); for spa inputs it weaves the shell — serializeState() HTML-escapes initial_state (<,>,&,U+2028/9) into <script id=\"__MESOFACT_STATE__\" type=\"application/json\">, then appends <script type=module src=/{build_id}/hydrate/{script}> before </body>. (5) contract.ts: Hydration.script relaxed to optional (build owns the hashed script/code_split; render ships initial_state) + documents the 6-line client snippet. index.ts wires client bundling + hydration map + spa prerender inputs.")
//! @yah:verify("cd packages/mesofact-runtime && bun run typecheck && bun run build && bun test")
//! @yah:verify("cd packages/mesofact-build && bun run typecheck && bun test")
//! @yah:assumes("Code-split chunks are fetched by the entry script's own relative ESM imports once it's served from /{build_id}/hydrate/, so the shell only injects the entry <script>; the manifest still lists code_split so the publisher uploads every chunk.")
//! @yah:assumes("Runtime dist must be rebuilt (bun run build in mesofact-runtime) for build/worker to see the new client_entrypoint + optional Hydration.script types — @mesofact/runtime resolves via dist/, not src/.")
//!
//! @yah:ticket(R015-F4, "Server/client module boundary lint — host-only API import detection")
//! @yah:assignee(agent:claude)
//! @yah:at(2026-06-04T19:32:49Z)
//! @yah:status(review)
//! @yah:phase(P2)
//! @yah:parent(R015)
//! @yah:next("Build-time walk of the module graph reachable from client_entrypoint. Imports of host-only APIs (configurable list: node:fs, node:net, named db drivers, etc.) fail the build with the offending import chain.")
//! @yah:next("Same machinery also enforces W173 Placement validation: reject placement:\"host\" on a route importing edge-only APIs, and placement:\"edge\" on a route importing host-only APIs.")
//! @yah:next("Implementation: spike Bun.build's artifact `imports` analysis vs `bun build --metafile=path` (esbuild-shape JSON) and pick before shipping. The bundler is already Bun.build (see bundle.ts) so artifact analysis is the lower-friction path if it exposes the import graph.")
//! @yah:next("Soft-gated on R434-F5 (yah-side first SSR consumer) for end-to-end validation, but the SPA-side lint (host-only APIs in a spa client_entrypoint) is testable today without it.")
//! @yah:next("Throwaway when RSC's \"use client\" lands; same idea, finer granularity.")
//! @yah:verify("A test fixture client_entrypoint that imports node:fs fails the build with the import chain in the error")
//! @yah:verify("A test fixture ssr+placement:\"edge\" entrypoint that imports a db driver fails the build")
//! @yah:verify("Existing yah-side SPA routes (../../app/yah/web/marketing, ../../app/yah/web/dashboard) build clean")
//! @yah:handoff("Server/client module boundary lint shipped. New packages/mesofact-build/src/host-lint.ts exports BROWSER_FORBIDDEN (node:* regex + bare-name builtins fs/path/net/...) and EDGE_FORBIDDEN (browser list + db drivers pg/mysql2/mongodb/redis/ioredis/better-sqlite3). Implementation runs Bun.build with a side-effect onResolve plugin and no outdir — zero files hit disk, full transitive import graph captured. If a forbidden specifier doesn't resolve at all (e.g. `pg` not installed), Bun.build throws Bundle failed; we swallow that and surface the recorded violation instead, since the lint error names the offending importer chain (much more actionable than 'Bundle failed'). Wired into build pipeline BEFORE bundleEntrypoints so violations short-circuit the cascade rather than tripping a downstream bundler error. Two fixtures: spa-host-only (client_entrypoint → describe.ts → node:fs — confirms transitive walk + chain in error message) and ssr-edge-host-only (ssr placement:'edge' importing pg). Existing ssr fixture confirms placement:'host' is permissive (no false positive). Verified on real yah-side consumers: app/yah/web/marketing (5 routes, 3 spa) and app/yah/web/dashboard (7 routes, 6 spa) both `bun run build` clean. Test totals: mesofact-build 33 pass (was 30); mesofact-runtime 57 pass; typecheck clean across runtime/build/worker. Configurable-list extensibility (W173 'configurable list') deferred until a real override case surfaces — the named exports BROWSER_FORBIDDEN/EDGE_FORBIDDEN can be re-exported or overridden via a BuildOptions field later without API churn.")
//! @yah:verify("cd packages/mesofact-build && bun test — 33 pass")
//! @yah:verify("cd packages/mesofact-build && bun run typecheck — clean")
//! @yah:verify("cd app/yah/web/marketing && bun run build — clean")
//! @yah:verify("cd app/yah/web/dashboard && bun run build — clean")

// Phase 1 — bundle each route's server entrypoint to ESM under `dist/server/`.
// Uses Bun's bundler (Bun.build) which natively understands TS, JSX, and
// workspace imports. The Vite/Bun split mentioned in the architecture is a
// future concern (Mode 3 client tree); P5 only needs the server tree.
//
// Each entrypoint becomes a deterministic output filename
// (`<route_key>.js`) so the manifest's `render_entrypoint` can name a stable
// path regardless of source layout. `route_key` comes from the route pattern
// (see `route-key.ts`).
//
// External dep: `@mesofact/runtime` is kept external — the build process
// already has it loaded, and bundling it would duplicate the
// `AsyncLocalStorage` registry that backs `runInTrackCtx`. The publisher
// (P6) zips the entrypoint + node_modules as a deployable unit.

import { mkdir } from "node:fs/promises";
import { basename, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { BuildError } from "./load-routes.js";
import { routeKey } from "./route-key.js";

// SSR routes export a Web Fetch handler as `default` —
// `(req: Request) => Promise<Response>`. Static/spa routes export a `render`
// fn (validated lazily by the prerender pass). The Fetch signature can't be
// proven from the module's static shape — only that `default` is callable —
// so this verifies the call shape and leaves runtime behavior to the dev
// proxy / pond container / Worker. Catches the common authoring slip:
// shipping `render` for a `mode:"ssr"` route, or no default export at all.
export async function assertSsrEntrypoint(route: string, bundlePath: string): Promise<void> {
  let mod: { default?: unknown };
  try {
    mod = (await import(pathToFileURL(bundlePath).href)) as { default?: unknown };
  } catch (e) {
    throw new BuildError(
      `route ${route}: failed to import SSR bundle at ${bundlePath}: ${(e as Error).message}`,
    );
  }
  if (typeof mod.default !== "function") {
    throw new BuildError(
      `route ${route}: mode:"ssr" entrypoint must \`export default\` a Fetch handler ` +
        `\`(req: Request) => Promise<Response>\` (got ${describe(mod.default)})`,
    );
  }
}

function describe(v: unknown): string {
  if (v === undefined) return "no default export";
  if (v === null) return "null";
  return typeof v;
}

export type BundleInput = {
  route: string;
  entrypoint: string; // path resolved against `projectRoot`
};

export type BundleOutput = {
  route: string;
  // Path written under `outDir/server/`. Used as the manifest's
  // `render_entrypoint`.
  serverPath: string;
  // Absolute path for the prerender driver to dynamic-import.
  absolutePath: string;
};

export async function bundleEntrypoints(
  projectRoot: string,
  outDir: string,
  inputs: readonly BundleInput[],
): Promise<BundleOutput[]> {
  if (inputs.length === 0) return [];
  const serverDir = join(outDir, "server");
  await mkdir(serverDir, { recursive: true });

  // Bun.build emits one file per entrypoint, named after the source basename.
  // We want a deterministic name per route — bundle one at a time and rename
  // via `naming`. (Bun's naming token `[name]` defaults to source basename;
  // setting it explicitly per-call gives us the route_key shape we want.)
  const outputs: BundleOutput[] = [];
  for (const input of inputs) {
    const key = routeKey(input.route);
    const absEntry = resolve(projectRoot, input.entrypoint);
    const result = await Bun.build({
      entrypoints: [absEntry],
      outdir: serverDir,
      target: "bun",
      format: "esm",
      naming: `${key}.js`,
      external: ["@mesofact/runtime"],
      splitting: false,
      sourcemap: "none",
    });
    if (!result.success) {
      const msg = result.logs.map((l) => l.message).join("\n");
      throw new BuildError(`bundle failed for route ${input.route}: ${msg}`);
    }
    const written = result.outputs.find((o) => o.kind === "entry-point");
    if (!written) {
      throw new BuildError(`bundle for route ${input.route} produced no entry-point output`);
    }
    const absolutePath = fileURLToPath(pathToFileURL(written.path).href);
    outputs.push({
      route: input.route,
      serverPath: `dist/server/${key}.js`,
      absolutePath,
    });
  }
  return outputs;
}

// ─── Mode 3 client tree ───────────────────────────────────────────────────────

export type ClientBundleInput = {
  route: string;
  clientEntrypoint: string; // path resolved against `projectRoot`
};

export type ClientBundleOutput = {
  route: string;
  // Entry-point filename, content-hashed, relative to `dist/hydrate/`. Goes in
  // the manifest's `hydration.script` and is referenced by the shell's
  // `<script type="module" src="/{build_id}/hydrate/<script>">`.
  script: string;
  // Code-split chunk filenames (also under `dist/hydrate/`). The entry imports
  // them by relative path, so the browser fetches them from the same prefix;
  // the manifest lists them so the publisher uploads every chunk.
  code_split: string[];
};

// Phase 1b — bundle each Mode 3 route's *client* entrypoint to `dist/hydrate/`.
// Unlike the server tree, the client tree targets the browser, content-hashes
// outputs (immutable CDN caching, see §"Static asset handling"), and enables
// code splitting so shared chunks are emitted once. `@mesofact/runtime` is NOT
// external here — the client never touches the server-only adapter registry;
// any runtime imports in a client entry must be type-only (erased at build).
export async function bundleClientEntrypoints(
  projectRoot: string,
  outDir: string,
  inputs: readonly ClientBundleInput[],
): Promise<ClientBundleOutput[]> {
  if (inputs.length === 0) return [];
  const hydrateDir = join(outDir, "hydrate");
  await mkdir(hydrateDir, { recursive: true });

  const outputs: ClientBundleOutput[] = [];
  for (const input of inputs) {
    const key = routeKey(input.route);
    const absEntry = resolve(projectRoot, input.clientEntrypoint);
    const result = await Bun.build({
      entrypoints: [absEntry],
      outdir: hydrateDir,
      target: "browser",
      format: "esm",
      splitting: true,
      sourcemap: "none",
      naming: {
        entry: `${key}.[hash].js`,
        chunk: `${key}.chunk-[hash].js`,
        asset: "[name].[hash].[ext]",
      },
    });
    if (!result.success) {
      const msg = result.logs.map((l) => l.message).join("\n");
      throw new BuildError(`client bundle failed for route ${input.route}: ${msg}`);
    }
    const entry = result.outputs.find((o) => o.kind === "entry-point");
    if (!entry) {
      throw new BuildError(
        `client bundle for route ${input.route} produced no entry-point output`,
      );
    }
    const code_split = result.outputs
      .filter((o) => o.kind === "chunk")
      .map((o) => basename(o.path))
      .sort();
    outputs.push({
      route: input.route,
      script: basename(entry.path),
      code_split,
    });
  }
  return outputs;
}
