# @mesofact/worker

Bun render-pool worker. Listens on a Unix-domain socket, speaks the
NDJSON IPC protocol documented in
[`.yah/docs/architecture/mesofact.md` §"IPC protocol"](../../.yah/docs/architecture/mesofact.md),
and dispatches `render` messages to server bundles named in a manifest.

This package is **internal to mesofact**. The public seam is
`@mesofact/runtime` (types only); the proxy spawns this worker as a
subprocess.

## Run

```bash
bun src/worker.ts --socket /tmp/mesofact.sock --manifest dist/manifest.json
```

The worker emits `{ id: 0, kind: "ready", ... }` once all route
entrypoints are loaded; until then it is not accepting renders.
