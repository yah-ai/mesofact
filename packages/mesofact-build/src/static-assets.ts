// Static-asset discovery (R490-F4). Walk the workload's `public/` dir
// (override via `[build] public_dir` in mesofact.config.toml), copy every
// file verbatim into `dist/html/` preserving relative paths, and return the
// manifest `static_assets` entries — `{ key, content_hash, content_type,
// immutable }` per file.
//
// Files are copied byte-for-byte (no minify, no rename): public/ is the
// "serve exactly this" overlay, matching the historical
// `cp -R public/. dist/html/` step the marketing app carried in its build
// script. Content hashes are sha-256 hex so the publisher can diff uploads;
// `immutable: false` because the keys are NOT content-addressed (a new build
// can change the bytes behind the same key).

import { createHash } from "node:crypto";
import { mkdir, readdir, readFile, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { extname, join } from "node:path";
import type { ManifestStaticAsset } from "@mesofact/runtime";

export const DEFAULT_PUBLIC_DIR = "public";

// Minimal extension → MIME map for the asset kinds a mesofact workload
// ships. Unknown extensions fall back to application/octet-stream.
const CONTENT_TYPES: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".json": "application/json",
  ".txt": "text/plain; charset=utf-8",
  ".xml": "application/xml",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".gif": "image/gif",
  ".webp": "image/webp",
  ".avif": "image/avif",
  ".ico": "image/x-icon",
  ".woff": "font/woff",
  ".woff2": "font/woff2",
  ".ttf": "font/ttf",
  ".otf": "font/otf",
  ".pdf": "application/pdf",
  ".webmanifest": "application/manifest+json",
};

export function contentTypeFor(relPath: string): string {
  return CONTENT_TYPES[extname(relPath).toLowerCase()] ?? "application/octet-stream";
}

// Copy `<projectRoot>/<publicDir>/**` → `<outDir>/html/**` and return sorted
// manifest entries. A missing public dir is not an error — workloads without
// a static overlay (e.g. the dashboard) simply emit `static_assets: []`.
export async function discoverStaticAssets(
  projectRoot: string,
  outDir: string,
  publicDir: string = DEFAULT_PUBLIC_DIR,
): Promise<ManifestStaticAsset[]> {
  const srcRoot = join(projectRoot, publicDir);
  if (!existsSync(srcRoot)) return [];

  const keys = await walkFiles(srcRoot, "");
  keys.sort();

  const htmlDir = join(outDir, "html");
  const assets: ManifestStaticAsset[] = [];
  for (const key of keys) {
    const bytes = await readFile(join(srcRoot, key));
    const dest = join(htmlDir, key);
    await mkdir(join(dest, ".."), { recursive: true });
    await writeFile(dest, bytes);
    assets.push({
      key,
      content_hash: createHash("sha256").update(bytes).digest("hex"),
      content_type: contentTypeFor(key),
      immutable: false,
    });
  }
  return assets;
}

async function walkFiles(absDir: string, relPrefix: string): Promise<string[]> {
  const out: string[] = [];
  const entries = await readdir(absDir, { withFileTypes: true });
  for (const entry of entries) {
    const rel = relPrefix === "" ? entry.name : `${relPrefix}/${entry.name}`;
    if (entry.isDirectory()) {
      out.push(...(await walkFiles(join(absDir, entry.name), rel)));
    } else if (entry.isFile()) {
      out.push(rel);
    }
  }
  return out;
}
