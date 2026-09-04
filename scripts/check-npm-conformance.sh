#!/usr/bin/env bash
# check-npm-conformance — TIER A of the R773-F7 differential conformance
# corpus. This is the GATE.
#
# W318 §7 is explicit that the resolution algorithm is not the long pole for
# the stage-2 npm resolver — proving it agrees with what the ecosystem's own
# installers do, on real manifests, is. This script is that proof, run
# recurrently: it replays a checked-in corpus of manifests and diffs `rnpm`'s
# resolved tree against the `bun.lock` and `pnpm-lock.yaml` that `bun install`
# and `pnpm install` actually produced for each one.
#
# **It needs no network, no bun, no pnpm and no JS toolchain at all.** Every
# case ships its own recorded packuments and is replayed through
# `rnpm::CachePolicy::Offline` over a transport that ERRORS on any request, so
# a missing fixture fails by name instead of quietly re-resolving against
# whatever the registry holds today. That is deliberate and it is the whole
# reason this is a gate rather than a script somebody runs when they remember:
# a differential harness that needs the network, plus two package managers, to
# produce a verdict goes red when the network blinks, and a gate that cries
# wolf gets ignored.
#
# The other tier — `scripts/record-npm-conformance.sh` — is the recorder. It
# does all the things this one refuses to do, and nothing in QED runs it.
#
# Why its own pipeline rather than a step in check.toml: oss/mesofact is an
# independent cargo workspace excluded from the yah root workspace, so a step
# here costs a separate `cargo build` of `mesofact-build` — which links
# rolldown and, through `mesofact-ssr`, V8. That is minutes of build the
# ordinary per-push bar should not carry. `release-check.toml` runs it, the
# same way it runs `mesofact-new-smoke`, and anyone touching the resolver runs
# it by hand first:
#
#     yah qed run mesofact-npm-conformance
#     # or, directly:
#     oss/mesofact/scripts/check-npm-conformance.sh
#
# Usage:  scripts/check-npm-conformance.sh
# Env:    MESOFACT_CONFORMANCE_CASE   replay only this case (debugging)
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CORPUS="$ROOT/crates/mesofact-build/tests/corpus"
TMP_DIR="$(mktemp -d -t mesofact-npm-conformance.XXXXXX)"
trap 'rm -rf "$TMP_DIR"' EXIT

# The floor the corpus may not silently fall through. R773-F7 sized it at 10-20
# cases covering nested version conflicts, peer instancing, overrides, platform
# gating, dist-tags, scoped names, npm: aliases and real-world trees. A corpus
# that has emptied out — a bad merge, a .gitignore that swallowed the fixtures —
# would otherwise read as a pass, and a gate that cannot fail is not a gate.
MIN_CASES=10

PASS=0
FAIL=0

ok()    { PASS=$((PASS + 1)); echo "  ok   — $1"; }
bad()   { FAIL=$((FAIL + 1)); echo "  FAIL — $1"; }
check() { if [ "$1" = "0" ]; then ok "$2"; else bad "$2"; fi; }

# ── 1. The corpus is actually there ─────────────────────────────────────────
echo "==> corpus"
[ -d "$CORPUS" ]
check $? "the corpus directory exists ($CORPUS)"

CASES=0
if [ -d "$CORPUS" ]; then
  CASES="$(find "$CORPUS" -mindepth 2 -maxdepth 2 -name case.toml | wc -l | tr -d ' ')"
fi
[ "$CASES" -ge "$MIN_CASES" ]
check $? "the corpus holds at least $MIN_CASES cases (found $CASES)"

# Every case must ship the three recorded artifacts it is replayed from. A case
# missing its packuments would fail inside the harness anyway, but it would fail
# as "offline and no cached packument" — which reads like a resolver problem and
# is really a missing file.
INCOMPLETE=""
if [ -d "$CORPUS" ]; then
  for case_dir in "$CORPUS"/*/; do
    [ -f "$case_dir/case.toml" ] || continue
    for artifact in package.json bun.lock pnpm-lock.yaml packuments; do
      if [ ! -e "$case_dir/$artifact" ]; then
        INCOMPLETE="$INCOMPLETE $(basename "$case_dir")/$artifact"
      fi
    done
  done
fi
[ -z "$INCOMPLETE" ]
check $? "every case ships package.json + both lockfiles + packuments/${INCOMPLETE:+ — MISSING:$INCOMPLETE}"

# ── 2. Build the harness ────────────────────────────────────────────────────
echo "==> building mesofact-conformance"
cargo build --manifest-path "$ROOT/Cargo.toml" -p mesofact-build \
  --bin mesofact-conformance > "$TMP_DIR/build.log" 2>&1
check $? "mesofact-conformance builds"
if [ ! -x "$ROOT/target/debug/mesofact-conformance" ]; then
  tail -n 40 "$TMP_DIR/build.log"
  echo
  echo "FAILED: $PASS passed, $((FAIL + 1)) failed"
  exit 1
fi

# ── 3. Replay ───────────────────────────────────────────────────────────────
echo "==> replaying the corpus (offline, no bun, no pnpm)"
# The replay's own per-case report goes straight to stdout: when a case
# diverges it names the install path, the package and BOTH versions, which is
# the entire deliverable of this gate. "Trees differ" would cost the next
# reader a full re-derivation.
#
# Two explicit invocations rather than an array spliced with "${A[@]}": under
# `set -u`, bash 3.2 — which is what /bin/bash is on macOS — treats an EMPTY
# array's expansion as an unbound variable and kills the pipeline, so the
# common (no --case) path failed on exactly the machines this is developed on.
if [ -n "${MESOFACT_CONFORMANCE_CASE:-}" ]; then
  "$ROOT/target/debug/mesofact-conformance" check \
    --case "$MESOFACT_CONFORMANCE_CASE" 2>&1 | tee "$TMP_DIR/replay.log"
else
  "$ROOT/target/debug/mesofact-conformance" check 2>&1 | tee "$TMP_DIR/replay.log"
fi
REPLAY=${PIPESTATUS[0]}
check "$REPLAY" "every recorded case resolves to the tree bun and pnpm produced"

# ── 4. The gate really did stay offline ─────────────────────────────────────
# Not a promise about the code — a check on what this run did. `record` is the
# only subcommand that reaches the network and it refuses to run without
# MESOFACT_CONFORMANCE_RECORD=1, which nothing here sets.
[ -z "${MESOFACT_CONFORMANCE_RECORD:-}" ]
check $? "the recorder was not armed (MESOFACT_CONFORMANCE_RECORD is unset)"

echo
if [ "$FAIL" -eq 0 ]; then
  echo "PASSED: $PASS passed, 0 failed"
  exit 0
fi
echo "FAILED: $PASS passed, $FAIL failed"
tail -n 40 "$TMP_DIR/replay.log" 2>/dev/null
exit 1
