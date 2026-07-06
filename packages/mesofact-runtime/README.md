# @mesofact/runtime

Render contract types and adapter API for [mesofact](../../). Consumed by
render entrypoints (e.g. `examples/yah-dev/`) and by any outside TS project
that needs the contract — frontends never import a mesofact SDK; they just
type their `render(req)` against `RenderRequest` / `RenderResult`.

See [`../../.yah/docs/architecture/mesofact.md`](../../.yah/docs/architecture/mesofact.md)
§"The shared seam: one render contract" and §"Adapter API surface".

## Client-safe subpath: `@mesofact/runtime/hydration`

The package root (`.`) barrel re-exports server-only code (`track-ctx.ts` →
`node:async_hooks`, `config.ts` → `node:fs`) that a browser bundler can't
tree-shake past — importing *anything* from `@mesofact/runtime` pulls those
Node builtins into a client bundle (R513-B11). Browser host adapters that
only need the hydration handoff constants/helpers (`SPA_STATE_SCRIPT_ID`,
`hydrationScriptTag`, ...) must import from `@mesofact/runtime/hydration`
instead — it resolves to `src/hydration.ts`, which has no imports of its own.
