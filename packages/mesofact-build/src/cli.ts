#!/usr/bin/env bun
// `mesofact-build <project-dir>` — thin wrapper around `build()`. Exits with
// a non-zero status (and prints validation details) on failure.

import { join, resolve } from "node:path";
import { registerSourcesFromConfig } from "@mesofact/runtime";
import { build, BuildError, ValidationFailed } from "./index.js";
import { loadConfigOpt } from "./source-catalog.js";

async function main(): Promise<number> {
  const args = process.argv.slice(2);
  if (args.length === 0 || args[0] === "-h" || args[0] === "--help") {
    process.stdout.write("Usage: mesofact-build <project-dir>\n");
    return args.length === 0 ? 2 : 0;
  }
  const projectRoot = args[0]!;
  try {
    // Register adapters from config so source-derived prerender.query routes
    // can call into them. Build itself is registry-agnostic — tests register
    // stubs the same way.
    const config = loadConfigOpt(join(resolve(projectRoot), "mesofact.config.toml"));
    if (config && Object.keys(config.sources).length > 0) {
      registerSourcesFromConfig(config);
    }
    const result = await build({ projectRoot });
    process.stdout.write(
      `mesofact build ok — build_id=${result.buildId}\n` +
        `  manifest:  ${result.manifestPath}\n` +
        `  tag-index: ${result.tagIndexPath}\n` +
        `  html:      ${result.htmlPaths.length} file(s)\n`,
    );
    return 0;
  } catch (err) {
    if (err instanceof ValidationFailed) {
      process.stderr.write(`mesofact build failed: ${err.message}\n`);
      return 1;
    }
    if (err instanceof BuildError) {
      process.stderr.write(`mesofact build failed: ${err.message}\n`);
      return 1;
    }
    process.stderr.write(
      `mesofact build crashed: ${err instanceof Error ? err.stack ?? err.message : String(err)}\n`,
    );
    return 2;
  }
}

main().then((code) => process.exit(code));
