#!/usr/bin/env bash
# Outside-consumer smoke test (R003 P1 DoD): an unrelated TS project,
# installed via the packed tarball (not the workspace symlink), can import
# types from @mesofact/runtime and typecheck.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PKG_DIR="$ROOT/packages/mesofact-runtime"
TMP_DIR="$(mktemp -d -t mesofact-smoke.XXXXXX)"
trap 'rm -rf "$TMP_DIR"' EXIT

echo "==> building @mesofact/runtime"
(cd "$PKG_DIR" && bun run build > /dev/null)

echo "==> packing tarball into $TMP_DIR"
(cd "$PKG_DIR" && bun pm pack --destination "$TMP_DIR" --quiet)
TARBALL="$(ls "$TMP_DIR"/mesofact-runtime-*.tgz | head -1)"
echo "    tarball: $(basename "$TARBALL")"

CONSUMER="$TMP_DIR/consumer"
mkdir -p "$CONSUMER/src"

cat > "$CONSUMER/package.json" <<EOF
{
  "name": "outside-consumer",
  "private": true,
  "type": "module",
  "scripts": { "typecheck": "tsc --noEmit" },
  "dependencies": { "@mesofact/runtime": "file:$TARBALL" },
  "devDependencies": { "typescript": "^5.8.3" }
}
EOF

cat > "$CONSUMER/tsconfig.json" <<'EOF'
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "lib": ["ES2022"],
    "strict": true,
    "skipLibCheck": true,
    "esModuleInterop": true,
    "isolatedModules": true,
    "noEmit": true
  },
  "include": ["src/**/*"]
}
EOF

cat > "$CONSUMER/src/test.ts" <<'EOF'
import type {
  RenderRequest,
  RenderResult,
  RenderFn,
  Source,
  CachePolicy,
  Hydration,
} from "@mesofact/runtime";
import {
  SourceError,
  SourceUnavailableError,
  SourceTimeoutError,
  SourceQueryError,
  RowNotFoundError,
} from "@mesofact/runtime";

const render: RenderFn = async (req: RenderRequest) => {
  const cache: CachePolicy = { ttl: 60, tags: [`url:${req.url}`] };
  const result: RenderResult = { html: `<h1>${req.url}</h1>`, cache };
  return result;
};

const hydration: Hydration = { script: "x.js", initial_state: { ok: true } };

const usingSource = async (s: Source) => {
  await s.timeout(50).get<{ id: string }>("t", "1");
  await s.noTrack().fetch("k");
};

const errs: SourceError[] = [
  new SourceUnavailableError("s"),
  new SourceTimeoutError("s", 100),
  new SourceQueryError("s", "bad sql"),
  new RowNotFoundError("s", "t", "1"),
];

void render;
void hydration;
void usingSource;
void errs;
EOF

echo "==> installing tarball"
(cd "$CONSUMER" && bun install > /dev/null 2>&1)

echo "==> typechecking outside consumer"
(cd "$CONSUMER" && bun run typecheck)

echo "==> ✓ outside consumer typechecks against packed @mesofact/runtime"
