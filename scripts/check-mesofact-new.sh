#!/usr/bin/env bash
# check-mesofact-new — the release gate on the curated-JS-set COMMITMENT
# (W225 §2, operator decision 2026-08-28; R759-T4).
#
# The commitment is that a mesofact version pins react / react-dom / the
# @mesofact runtime barrel / matching @types at exact versions, tested per
# release. "An untested pin is decorative" — this is the test.
#
# It runs `mesofact new` and then builds and serves the result on a PATH with
# **no JS runtime and no package manager on it at all**, which is the only way
# to prove the claim rather than assert it: with bun on PATH a broken lockfile
# still installs, and the whole promise silently evaporates.
#
# THREE shipped binaries. This header used to say two, and that a scaffolded
# machine therefore never has `mesofact-build` — R746-F9 changed it and the
# comment did not follow. Verified 2026-09-02 against the published artifact:
# cdn.yah.dev/mesofact/0.8.28.1/aarch64-apple-darwin/…tar.gz unpacks to
# `mesofact`, `mesofact-dev` AND `mesofact-build`.
#
# `mesofact-build` still stays off `CLEAN_PATH`, and the reason is now the
# sharper one: `mesofact-dev` builds IN-PROCESS (BuildDriver::InProcess,
# R759-T4), so a run that could reach a `mesofact-build` on PATH would not
# prove that. Nothing here ever needs it — §6b typechecks through `mes check`,
# which is the same code linked into the dev binary, because that is the only
# name the R759-F2 trampoline will actually run for a user (R832-T4).
#
# Needs network (the materializer fetches tarballs from registry.npmjs.org and
# verifies each against the sha512 in the shipped lock) and a free port.
#
# Usage:  scripts/check-mesofact-new.sh
# Env:    MESOFACT_NEW_PORT   port to serve on (default 8953)
#         MESOFACT_NEW_KEEP=1 keep the scratch dir for inspection
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PORT="${MESOFACT_NEW_PORT:-8953}"
TMP_DIR="$(mktemp -d -t mesofact-new.XXXXXX)"
SERVER_PID=""
LIB_PID=""

PASS=0
FAIL=0

cleanup() {
  for pid in "$SERVER_PID" "$LIB_PID"; do
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    fi
  done
  if [ "${MESOFACT_NEW_KEEP:-0}" = "1" ]; then
    echo "  (kept $TMP_DIR)"
  else
    rm -rf "$TMP_DIR"
  fi
}
trap cleanup EXIT

ok()   { PASS=$((PASS + 1)); echo "  ok   — $1"; }
bad()  { FAIL=$((FAIL + 1)); echo "  FAIL — $1"; }
check() { if [ "$1" = "0" ]; then ok "$2"; else bad "$2"; fi; }

# ── 1. Build the two binaries the release actually ships ────────────────────
echo "==> building mesofact + mesofact-dev"
cargo build --manifest-path "$ROOT/Cargo.toml" -p mesofact --bin mesofact \
  > "$TMP_DIR/build-prod.log" 2>&1 || { cat "$TMP_DIR/build-prod.log"; exit 1; }
cargo build --manifest-path "$ROOT/Cargo.toml" -p mesofact-dev --bin mesofact-dev \
  > "$TMP_DIR/build-dev.log" 2>&1 || { cat "$TMP_DIR/build-dev.log"; exit 1; }

# A store slot holds both binaries under their plain names, which is also what
# the R759-F2 shim execs. Mirroring that here means this test exercises the
# same PATH shape a user gets, not a pair of target/debug/ paths.
SLOT="$TMP_DIR/bin"
mkdir -p "$SLOT"
cp "$ROOT/target/debug/mesofact" "$SLOT/mesofact"
cp "$ROOT/target/debug/mesofact-dev" "$SLOT/mesofact-dev"

# ── 2. The scrubbed PATH ────────────────────────────────────────────────────
# Only the slot plus the base system dirs. Homebrew (/opt/homebrew/bin),
# /usr/local/bin, nvm, fnm, volta and every other place a JS runtime lives are
# all outside this set on both macOS and Linux.
CLEAN_PATH="$SLOT:/usr/bin:/bin:/usr/sbin:/sbin"

echo "==> asserting the PATH really is Node-free"
LEAKED=""
for tool in node bun npm npx pnpm yarn deno mesofact-build; do
  if PATH="$CLEAN_PATH" command -v "$tool" > /dev/null 2>&1; then
    LEAKED="$LEAKED $tool"
  fi
done
if [ -z "$LEAKED" ]; then
  ok "no JS runtime, package manager or mesofact-build on PATH"
else
  bad "PATH leaks:$LEAKED — this run would prove nothing, so it is a failure"
  echo "FAILED: $PASS passed, $FAIL failed"
  exit 1
fi

# ── 3. Scaffold ─────────────────────────────────────────────────────────────
PROJECT="$TMP_DIR/scaffolded"
echo "==> mesofact new"
PATH="$CLEAN_PATH" mesofact new "$PROJECT" > "$TMP_DIR/new.log" 2>&1
check $? "mesofact new exits 0"

for f in package.json bun.lock mesofact.routes.ts .mesofact-version src/home.tsx; do
  [ -f "$PROJECT/$f" ]
  check $? "scaffold contains $f"
done

# R832-T3 retired the vended `vendor/mesofact-runtime/` subset for the
# published package. Asserted as an absence: a leftover copy would still
# typecheck, it would just shadow the registry types with a stale subset.
[ ! -e "$PROJECT/vendor" ]
check $? "no vendored runtime barrel — the types come from the lock"

# The pin has to be the scaffolding binary's own version, or `.mesofact-version`
# selects a mesofact whose curated set is not the one in this bun.lock.
BIN_VERSION="$(PATH="$CLEAN_PATH" mesofact --version | awk '{print $NF}')"
PINNED="$(tr -d '[:space:]' < "$PROJECT/.mesofact-version")"
[ "$BIN_VERSION" = "$PINNED" ]
check $? ".mesofact-version ($PINNED) is the scaffolding binary's version ($BIN_VERSION)"

# Not a single range anywhere in the emitted lock: a range is a resolution
# request, and nothing on the curated path resolves.
! grep -Eq '"[a-z@/-]+@[\^~]' "$PROJECT/bun.lock"
check $? "bun.lock pins exact versions (no ^ or ~ locators)"

# ── 4. Build + serve, on the scrubbed PATH ──────────────────────────────────
echo "==> mesofact-dev (builds in-process, materializes node_modules from the lock)"
# Deliberately `mesofact-dev .` from inside the project, not an absolute path:
# that is the invocation the scaffold's README gives, and it is the one that
# used to lose every mode:"ssr" route (the watcher's relative gen-dir cannot
# become a `file://` module URL — R759-T4). The /api/hello assertion below is
# the regression guard.
(
  cd "$PROJECT" || exit 1
  PATH="$CLEAN_PATH" exec mesofact-dev . --port "$PORT"
) > "$TMP_DIR/serve.log" 2>&1 &
SERVER_PID=$!

# The first build fetches ~10 tarballs and prerenders; give it room, but poll
# so a fast machine is not punished for it.
READY=1
for _ in $(seq 1 120); do
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    break
  fi
  if curl -fsS -o /dev/null "http://127.0.0.1:$PORT/" 2>/dev/null; then
    READY=0
    break
  fi
  sleep 1
done
if [ "$READY" != "0" ]; then
  bad "mesofact-dev never served / (see below)"
  tail -n 60 "$TMP_DIR/serve.log"
  echo "FAILED: $PASS passed, $FAIL failed"
  exit 1
fi
ok "mesofact-dev built and served with no package manager and no Node on PATH"

[ -d "$PROJECT/node_modules/react" ] && [ -d "$PROJECT/node_modules/react-dom" ]
check $? "node_modules materialized from the shipped lock (react + react-dom present)"

# The barrel is an ordinary registry entry since R832-T3, so this is a real
# fetch-and-verify against the sha512 in the lock — and it must land the
# declarations, not just a directory: `dist/index.d.ts` is what `types` points
# at and what a `vendor/` subset used to stand in for.
[ -f "$PROJECT/node_modules/@mesofact/runtime/dist/index.d.ts" ]
check $? "@mesofact/runtime fetched from the registry with its published types"

# Every version installed on disk is the version the table pinned. This is the
# commitment itself: not "an install happened" but "the install is the pin".
DRIFT=""
for pkg in react react-dom scheduler loose-envify js-tokens typescript \
           @mesofact/runtime aws4fetch smol-toml; do
  want="$(python3 -c "
import json,sys
lock=open('$PROJECT/bun.lock').read()
import re
m=re.search(r'\"$pkg\": \[\"$pkg@([^\"]+)\"', lock)
print(m.group(1) if m else '')
")"
  got="$(python3 -c "
import json
try:
    print(json.load(open('$PROJECT/node_modules/$pkg/package.json'))['version'])
except Exception:
    print('')
")"
  if [ -n "$want" ] && [ "$want" = "$got" ]; then
    ok "$pkg on disk is $got — exactly what the lock pins"
  else
    DRIFT="$DRIFT $pkg(want=$want got=$got)"
  fi
done
[ -z "$DRIFT" ]
check $? "no curated package drifted from its pin"

# The obligation R832-T3 created, asserted on the artifact the registry
# actually served rather than on the table: a mesofact X.Y.Z binary must
# install @mesofact/runtime X.Y.Z. Goes red on a version bump whose npm
# publish has not happened yet, which is the whole reason it is here.
BARREL_VERSION="$(python3 -c "
import json
try:
    print(json.load(open('$PROJECT/node_modules/@mesofact/runtime/package.json'))['version'])
except Exception:
    print('')
")"
[ "$BARREL_VERSION" = "$BIN_VERSION" ]
check $? "@mesofact/runtime on disk is $BARREL_VERSION — the scaffolding binary's version ($BIN_VERSION)"

# ── 5. The routes the scaffold declares actually answer ─────────────────────
BODY="$(curl -fsS "http://127.0.0.1:$PORT/")"
printf '%s' "$BODY" | grep -q "<!doctype html>"
check $? "GET / serves the prerendered static route (react-dom/server ran at build time)"

API="$(curl -fsS "http://127.0.0.1:$PORT/api/hello?name=you")"
printf '%s' "$API" | grep -q '"hello":"you"'
check $? "GET /api/hello runs the ssr handler in-process (got: $API)"

STATUS="$(curl -s -o /dev/null -w '%{http_code}' -X PUT "http://127.0.0.1:$PORT/api/hello")"
[ "$STATUS" = "405" ]
check $? "PUT /api/hello is 405 — the handler owns its status (got $STATUS)"

# ── 6. An edit rebuilds, and does NOT re-install ────────────────────────────
# InstallMode::Auto must not re-fetch on every rebuild; if it did, the dev loop
# would hit the registry on every keystroke. Measured off the installer's own
# marker file rather than a log line — mesofact-build's tracing target is not
# in mesofact-dev's default env filter, so a grep for its INFO would pass
# vacuously.
MARKER="$PROJECT/node_modules/.mesofact-install.json"
[ -f "$MARKER" ]
check $? "the installer wrote its idempotence marker"
GENS_BEFORE="$(ls -d "$PROJECT"/.mesofact-dev/gen-* 2>/dev/null | wc -l | tr -d ' ')"
MARKER_BEFORE="$(cat "$MARKER" 2>/dev/null)"
touch "$PROJECT/src/home.tsx"
sleep 8
GENS_AFTER="$(ls -d "$PROJECT"/.mesofact-dev/gen-* 2>/dev/null | wc -l | tr -d ' ')"
MARKER_AFTER="$(cat "$MARKER" 2>/dev/null)"
grep -q "source change detected" "$TMP_DIR/serve.log"
check $? "an edit under src/ triggers a rebuild (gens $GENS_BEFORE -> $GENS_AFTER)"
[ "$MARKER_BEFORE" = "$MARKER_AFTER" ]
check $? "the rebuild did not re-install (install marker unchanged)"
curl -fsS -o /dev/null "http://127.0.0.1:$PORT/api/hello"
check $? "the ssr route still answers after a rebuild (gen respawn kept the isolate)"

# ── 6b. The scaffold typechecks, on a PATH with no JS runtime ───────────────
# R832-T4. The scaffold used to emit `typecheck: tsc --noEmit`, which needs a
# resolvable `tsc` and therefore Node — inside a project whose whole claim is
# the opposite — and no gate ran it, so nothing said so. It now emits
# `mesofact-build check .`, which spawns TypeScript 7's per-platform NATIVE
# binary directly. This cell is what keeps that true.
echo
echo "==> mes check (TypeScript 7's native compiler, still no Node)"
# `mes`, on the SAME slot and the SAME scrubbed PATH as everything above —
# nothing new goes on it. That is the point of the verb living on the dev
# binary: the R759-F2 trampoline vends `mesofact`, `mes` and `mesofact-dev` and
# exits 70 on any other name, so `mes` is a name a user provably has and
# `mesofact-build` is not. A store slot holds the dev binary as `mesofact-dev`
# and the trampoline maps `mes` onto it; a symlink is that shape here.
ln -sf mesofact-dev "$SLOT/mes"

grep -q '"typecheck": "mes check"' "$PROJECT/package.json"
check $? "the emitted typecheck script is 'mes check' — no tsc, so no Node"

# The materializer's platform gate, observed rather than unit-tested:
# typescript@7 fans out to twenty native compilers at ~27 MB each and the lock
# lists all twenty. Installing them all would be ~550 MB per project.
NATIVES="$(find "$PROJECT/node_modules/@typescript" -maxdepth 1 -mindepth 1 -type d 2>/dev/null | wc -l | tr -d ' ')"
[ "$NATIVES" = "1" ]
check $? "exactly one of typescript@7's twenty native compilers installed (got $NATIVES)"

(cd "$PROJECT" && PATH="$CLEAN_PATH" exec mes check) > "$TMP_DIR/typecheck.log" 2>&1
CHECK_CODE=$?
if [ "$CHECK_CODE" = "0" ]; then
  ok "the scaffold typechecks with no node and no bun on PATH"
else
  bad "mes check failed on a freshly scaffolded project (exit $CHECK_CODE)"
  tail -n 30 "$TMP_DIR/typecheck.log"
fi

# A green that cannot go red is not a check. Seeded in its own file so the
# running dev server's routes are untouched; `tsconfig.json` includes `src/**/*`.
cat > "$PROJECT/src/typecheck_probe.ts" <<'PROBE'
// Deleted immediately below — see check-mesofact-new.sh §6b.
export const seeded: number = "this is not a number";
PROBE
(cd "$PROJECT" && PATH="$CLEAN_PATH" exec mes check) > "$TMP_DIR/typecheck-red.log" 2>&1
RED_CODE=$?
rm -f "$PROJECT/src/typecheck_probe.ts"
[ "$RED_CODE" != "0" ]
check $? "a seeded type error fails the check (exit $RED_CODE) — this cell can go red"

# ── 7. The LIBRARY tier (`mesofact new --lib`, R832-T2) ─────────────────────
# Everything above is the STANDALONE tier, whose claim is "no package manager,
# no Node" — hence the scrubbed PATH. This section is the other tier, and its
# claim is different: the emitted crate COMPILES and both binaries run. That
# needs cargo and rustc, so it deliberately runs on the ambient PATH. A scaffold
# whose Rust half is untested is exactly the decorative promise W225 §2 rules
# out, and asserting it under a PATH with no toolchain would prove nothing.
echo
echo "==> mesofact new --lib (ambient PATH — this tier needs a Rust toolchain)"
LIBPROJ="$TMP_DIR/scaffolded-lib"
PATH="$CLEAN_PATH" mesofact new --lib "$LIBPROJ" > "$TMP_DIR/new-lib.log" 2>&1
check $? "mesofact new --lib exits 0"

for f in Cargo.toml src/lib.rs src/bin/scaffolded-lib.rs src/bin/scaffolded-lib-dev.rs \
         .github/workflows/ci.yml; do
  [ -f "$LIBPROJ/$f" ]
  check $? "lib scaffold contains $f"
done

# NO [patch.crates-io] BLOCK HERE, deliberately, and that is the point of this
# cell. Between 2026-08-28 and 2026-09-01 `mesofact-dev` carried
# `publish = false` and was absent from crates.io, so an emitted manifest
# resolved in-tree only and this script had to patch both deps to workspace
# paths. R832-T1 flipped the flag and `mesofact-dev@0.8.29` is now published —
# so the scaffold's plain version deps resolve for an OUTSIDE consumer, and
# this build proves it rather than assuming it.
#
# If you find yourself re-adding a patch block to make this pass: don't. It
# would mean the emitted manifest names something the registry does not have,
# which is a real defect for every consumer and exactly what this cell exists
# to catch. Fix the manifest or publish the missing crate.

# One invocation, no flags, both binaries — the two-bin pattern's actual claim.
# Shares the workspace target dir so this reuses artifacts the build at the top
# of this script already produced rather than compiling V8 a second time.
echo "==> cargo build (one command, no flags, two binaries)"
LIBTARGET="$ROOT/target"
(cd "$LIBPROJ" && CARGO_TARGET_DIR="$LIBTARGET" cargo build) \
  > "$TMP_DIR/lib-build.log" 2>&1
check $? "the emitted crate compiles (see $TMP_DIR/lib-build.log)"

PROD_BIN="$LIBTARGET/debug/scaffolded-lib"
DEV_BIN="$LIBTARGET/debug/scaffolded-lib-dev"
[ -x "$PROD_BIN" ] && [ -x "$DEV_BIN" ]
check $? "one flagless build emitted BOTH binaries"

# The dev affordance is observable, and its absence from prod is the point:
# the dev binary stands up a local object store under .mesofact-dev/ and the
# prod binary must not, from the same working directory. This is the tier's
# security claim reduced to a file that either exists or does not.
LIBPORT=$((PORT + 1))
echo "==> running the prod binary on $LIBPORT"
rm -rf "$LIBPROJ/.mesofact-dev"
(cd "$LIBPROJ" && PORT="$LIBPORT" exec "$PROD_BIN") > "$TMP_DIR/lib-prod.log" 2>&1 &
LIB_PID=$!
LIB_READY=1
for _ in $(seq 1 30); do
  kill -0 "$LIB_PID" 2>/dev/null || break
  if curl -fsS -o /dev/null "http://127.0.0.1:$LIBPORT/" 2>/dev/null; then LIB_READY=0; break; fi
  sleep 1
done
check "$LIB_READY" "the prod binary serves the crate's router"
curl -fsS -o /dev/null "http://127.0.0.1:$LIBPORT/readyz"
check $? "mesofact::serve_app added the standard probes"
[ ! -e "$LIBPROJ/.mesofact-dev" ]
check $? "the prod binary started NO dev object store (.mesofact-dev absent)"
kill "$LIB_PID" 2>/dev/null || true
wait "$LIB_PID" 2>/dev/null || true

echo "==> running the dev binary on $LIBPORT"
(cd "$LIBPROJ" && PORT="$LIBPORT" exec "$DEV_BIN") > "$TMP_DIR/lib-dev.log" 2>&1 &
LIB_PID=$!
LIB_READY=1
for _ in $(seq 1 30); do
  kill -0 "$LIB_PID" 2>/dev/null || break
  if curl -fsS -o /dev/null "http://127.0.0.1:$LIBPORT/" 2>/dev/null; then LIB_READY=0; break; fi
  sleep 1
done
check "$LIB_READY" "the dev binary serves the same router"
[ -f "$LIBPROJ/.mesofact-dev/s3.json" ]
check $? "the dev binary DID start the local object store (.mesofact-dev/s3.json written)"
# And the coordinates in it are live, not a stale file left by a previous run.
# Any HTTP status proves the socket is a running S3 surface; asserting on the
# body would be re-testing s3s-fs, which has its own tests.
S3_ENDPOINT="$(sed -n 's/.*"endpoint":"\([^"]*\)".*/\1/p' "$LIBPROJ/.mesofact-dev/s3.json")"
S3_CODE="$(curl -s -o /dev/null -w '%{http_code}' "$S3_ENDPOINT/dev/" 2>/dev/null)"
[ -n "$S3_CODE" ] && [ "$S3_CODE" != "000" ]
check $? "the advertised object-store endpoint answers ($S3_ENDPOINT -> HTTP $S3_CODE)"
kill "$LIB_PID" 2>/dev/null || true
wait "$LIB_PID" 2>/dev/null || true

echo
if [ "$FAIL" -eq 0 ]; then
  echo "PASSED: $PASS passed, 0 failed"
  exit 0
fi
echo "FAILED: $PASS passed, $FAIL failed"
tail -n 40 "$TMP_DIR/serve.log"
exit 1
