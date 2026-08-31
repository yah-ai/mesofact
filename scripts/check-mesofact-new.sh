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
# Two shipped binaries, and only two — `mesofact` and `mesofact-dev` are what
# .yah/qed/release-build.toml builds and what install.sh puts in a store slot.
# `mesofact-build` is NOT shipped, which is why mesofact-dev builds in-process
# (BuildDriver::InProcess, R759-T4) and why this script deliberately does not
# put it on PATH: doing so would test a machine no user has.
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

PASS=0
FAIL=0

cleanup() {
  if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
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

for f in package.json bun.lock mesofact.routes.ts .mesofact-version \
         vendor/mesofact-runtime/index.d.ts src/home.tsx; do
  [ -f "$PROJECT/$f" ]
  check $? "scaffold contains $f"
done

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

# The vended barrel is a `file:` dep, so the materializer symlinks rather than
# fetches it — the one entry in the closure that never touches the registry.
[ -e "$PROJECT/node_modules/@mesofact/runtime" ]
check $? "@mesofact/runtime linked from vendor/ (never fetched)"

# Every version installed on disk is the version the table pinned. This is the
# commitment itself: not "an install happened" but "the install is the pin".
DRIFT=""
for pkg in react react-dom scheduler loose-envify js-tokens typescript; do
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

echo
if [ "$FAIL" -eq 0 ]; then
  echo "PASSED: $PASS passed, 0 failed"
  exit 0
fi
echo "FAILED: $PASS passed, $FAIL failed"
tail -n 40 "$TMP_DIR/serve.log"
exit 1
