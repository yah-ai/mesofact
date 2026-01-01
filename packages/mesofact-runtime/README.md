# @mesofact/runtime

Render contract types and adapter API for [mesofact](../../). Consumed by
render entrypoints (e.g. `examples/yah-dev/`) and by any outside TS project
that needs the contract — frontends never import a mesofact SDK; they just
type their `render(req)` against `RenderRequest` / `RenderResult`.

See [`../../.yah/docs/architecture/mesofact.md`](../../.yah/docs/architecture/mesofact.md)
§"The shared seam: one render contract" and §"Adapter API surface".
