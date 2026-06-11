// Bun wrapper for the mesofact-dev SSR subprocess (R434-F3, W173).
//
// Reads the manifest from MESOFACT_GEN_DIR, dynamic-imports each mode:"ssr"
// route's render_entrypoint, and serves Fetch handlers on MESOFACT_SSR_PORT.
//
// Process boundary: stdout/stderr are inherited so the parent (mesofact-dev)
// can ring-buffer stderr through its LogBuffer. The wrapper itself logs to
// stderr for any startup/import errors; runtime per-request errors return
// 500 with the message in the response body.

interface RouteEntry {
  route: string;
  mode: string;
  render_entrypoint?: string;
}

interface Manifest {
  routes: RouteEntry[];
  ssr_prefixes?: string[];
}

const genDir = process.env.MESOFACT_GEN_DIR;
const portStr = process.env.MESOFACT_SSR_PORT;
if (!genDir || !portStr) {
  console.error(
    "[mesofact-dev/ssr] MESOFACT_GEN_DIR and MESOFACT_SSR_PORT required",
  );
  process.exit(2);
}
const port = Number(portStr);

const manifestPath = `${genDir}/manifest.json`;
let manifest: Manifest;
try {
  const raw = await Bun.file(manifestPath).text();
  manifest = JSON.parse(raw);
} catch (err) {
  console.error(`[mesofact-dev/ssr] failed to read ${manifestPath}: ${err}`);
  process.exit(2);
}

// Strip the first segment of render_entrypoint (the build out_dir, conventionally
// "dist") and join with the gen dir to get the on-disk path.
function resolveEntrypoint(rel: string): string {
  const slash = rel.indexOf("/");
  const sub = slash >= 0 ? rel.slice(slash + 1) : rel;
  return `${genDir}/${sub}`;
}

interface Handler {
  prefix: string;
  fetch: (req: Request) => Response | Promise<Response>;
}

const handlers: Handler[] = [];
for (const r of manifest.routes) {
  if (r.mode !== "ssr") continue;
  if (!r.render_entrypoint) {
    console.error(
      `[mesofact-dev/ssr] ssr route ${r.route} has no render_entrypoint; skipping`,
    );
    continue;
  }
  const path = resolveEntrypoint(r.render_entrypoint);
  let mod;
  try {
    mod = await import(path);
  } catch (err) {
    console.error(`[mesofact-dev/ssr] import ${path} failed: ${err}`);
    continue;
  }
  const fn = mod.default;
  if (typeof fn !== "function") {
    console.error(
      `[mesofact-dev/ssr] ${path}: default export is not a function`,
    );
    continue;
  }
  handlers.push({ prefix: derivePrefix(r.route), fetch: fn });
}

// W173 derivation rule, kept identical to the Rust side as a safety net.
function derivePrefix(route: string): string {
  let out = "";
  for (const seg of route.split("/")) {
    if (seg.length === 0) continue;
    if (seg.startsWith(":") || seg.startsWith("*")) {
      if (!out.endsWith("/")) out += "/";
      return out;
    }
    out += "/" + seg;
  }
  return out;
}

function matchesPrefix(path: string, prefix: string): boolean {
  if (path === prefix) return true;
  if (prefix.endsWith("/")) return path.startsWith(prefix);
  return path.startsWith(prefix + "/");
}

console.error(
  `[mesofact-dev/ssr] ready: ${handlers.length} handler(s) on port ${port}`,
);

Bun.serve({
  port,
  hostname: "127.0.0.1",
  async fetch(req) {
    const url = new URL(req.url);
    for (const h of handlers) {
      if (matchesPrefix(url.pathname, h.prefix)) {
        try {
          return await h.fetch(req);
        } catch (err) {
          console.error(`[mesofact-dev/ssr] handler error: ${err}`);
          return new Response(`SSR handler error: ${err}`, { status: 500 });
        }
      }
    }
    return new Response("Not Found", { status: 404 });
  },
});
