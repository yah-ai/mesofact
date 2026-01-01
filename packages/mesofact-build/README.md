# @mesofact/build

Build pipeline for mesofact projects. Implements P5 of the
[mesofact MVP rollout](../../.yah/docs/working/mesofact.md):

1. **Bundle** every `render_entrypoint` to ESM under `dist/server/`
2. **Route discovery** — load `mesofact.routes.ts`
3. **Source inference** — scan adapter calls (`r2('name')`, `sqlite('name')`,
   …) inside each entrypoint to populate `source_reads`. Honors
   `// @mesofact-sources foo, bar` override comments.
4. **Validate** — runtime's `validate()` over the assembled manifest, with
   the source catalog from `mesofact.config.toml`. Fails the build on Mode 1
   + scoped source / Mode 1 + `requires:user` violations.
5. **Mode 1 prerender** — wrap each `render()` in `runInTrackCtx`, expand the
   route's `prerender.params`, write HTML to `dist/html/<key>.html`.
6. **Manifest + tag-index** — emit `dist/manifest.json` and
   `dist/tag-index.json`.

## CLI

```
mesofact-build <project-dir>
```

Conventions inside the project dir:

| File | Purpose |
|---|---|
| `mesofact.routes.ts` | route table (exports `RoutesConfig` default) |
| `mesofact.config.toml` | source catalog (optional; only `r2` in P4) |
| `src/<entrypoint>.ts` | files referenced by each route's `entrypoint` |

Output lands in `<project-dir>/dist/`.

## Scope

P5 only — literal `prerender.params` lists. Source-derived
`prerender.query` (running `r2.list` at build time) ships in P6 alongside
the publisher. Mode 3 client tree is a placeholder; full hydration build
ships in P10.
