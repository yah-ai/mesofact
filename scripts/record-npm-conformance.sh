#!/usr/bin/env bash
# record-npm-conformance — TIER B of the R773-F7 differential conformance
# corpus. This is the RECORDER, not the gate.
#
# The gate is `scripts/check-npm-conformance.sh`, which is hermetic. This script
# is the opposite of hermetic on purpose: it runs the real `bun install` and
# `pnpm install` against the live npm registry and writes what they produced
# into `crates/mesofact-build/tests/corpus/`. Nothing in QED runs it.
#
# It exists so the corpus has PROVENANCE. Every case below states the manifest
# it was recorded from and the divergence it was authored to stress, so a human
# refreshing the corpus in a year re-runs one command instead of reverse-
# engineering thirteen fixture directories.
#
# Re-running is safe and idempotent-ish: each case is re-recorded from scratch
# (its `packuments/` is cleared first), so the only thing that changes between
# runs is what the registry itself published in the meantime. `case.toml`'s
# `known_divergence` list is REGENERATED EMPTY, so if you have hand-added a
# waiver, re-record that case alone and put the waiver back — see the FAIL
# message the recorder prints, which names the two honest ways forward.
#
# Usage:  scripts/record-npm-conformance.sh [case-name ...]
#         (no arguments records every case)
# Needs:  network, bun, pnpm, and a cargo toolchain.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/target/debug/mesofact-conformance"
WORK="$(mktemp -d -t mesofact-conformance-record.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

PASS=0
FAIL=0

# No host is pinned any more (R773-F9). The corpus used to record linux/x64/
# glibc into every case.toml and filter both sides of the diff through it,
# because resolution was host-specific and the two sides had to be told the same
# lie. Resolution is now portable — the resolver records `os`/`cpu`/`libc` and
# applies neither — so a bun.lock and the resolver's tree contain the same
# twenty-four `@esbuild/*` entries whatever machine records or replays them.

echo "==> building the recorder"
cargo build --manifest-path "$ROOT/Cargo.toml" -p mesofact-build \
  --bin mesofact-conformance > "$WORK/build.log" 2>&1 \
  || { cat "$WORK/build.log"; exit 1; }

# record <name> <description>, manifest on stdin.
record() {
  local name="$1" description="$2"
  # Record only what was asked for, when anything was asked for.
  if [ "$#" -ge 3 ]; then :; fi
  if [ -n "${ONLY:-}" ] && ! printf '%s\n' $ONLY | grep -qx "$name"; then
    return 0
  fi
  cat > "$WORK/$name.json"
  echo "==> $name"
  if MESOFACT_CONFORMANCE_RECORD=1 "$BIN" record \
      --name "$name" \
      --manifest "$WORK/$name.json" \
      --description "$description" \
      ; then
    PASS=$((PASS + 1))
  else
    FAIL=$((FAIL + 1))
  fi
}

ONLY="${*:-}"

# ── 1. Layout: a nested version conflict ────────────────────────────────────
record nested-version-conflict \
"Two requesters force two versions of ansi-styles into one tree: the root wants ^6 and chalk@4 wants ^4, so the hoist must place 6.x at the top and nest 4.x under chalk. The single clearest test that this resolver reproduces bun's LAYOUT and not merely its version choices." <<'EOF'
{
  "name": "nested-version-conflict",
  "version": "1.0.0",
  "dependencies": { "chalk": "^4.0.0", "ansi-styles": "^6.0.0" }
}
EOF

# ── 2. Phase 2: peers that must be instanced ────────────────────────────────
record peer-instancing \
"react-dom and @dnd-kit/core both declare react as a peer, so the W318 phase-2 post-pass has to satisfy three peer edges against one provider. pnpm mints every lock with autoInstallPeers: true and bun installs peers by default, so a resolver that skipped phase 2 would diverge from BOTH oracles here." <<'EOF'
{
  "name": "peer-instancing",
  "version": "1.0.0",
  "dependencies": {
    "react": "^18.3.0",
    "react-dom": "^18.3.0",
    "@dnd-kit/core": "^6.3.0"
  }
}
EOF

# ── 3. Phase 2: an OPTIONAL peer, which must not be auto-installed ──────────
record optional-peer \
"debug@4 declares supports-color as a peer marked optional in peerDependenciesMeta. The tree must contain it only because the root asked for it directly — an implementation that auto-installs optional peers produces the same package for the wrong reason, and only the second case (where nothing asks) tells them apart." <<'EOF'
{
  "name": "optional-peer",
  "version": "1.0.0",
  "dependencies": { "debug": "^4.3.4", "supports-color": "^8.1.1" }
}
EOF

# ── 4. Overrides redirecting a transitive dependency ────────────────────────
record overrides-redirect \
"An npm-style root overrides table pins chalk's transitive ansi-styles to an exact version its declared ^4.1.0 range would not have selected. Overrides are applied before the spec is parsed, so this also proves the override survives into the hoist rather than being re-decided by it." <<'EOF'
{
  "name": "overrides-redirect",
  "version": "1.0.0",
  "dependencies": { "chalk": "^4.1.2" },
  "overrides": { "ansi-styles": "4.2.1" }
}
EOF

# ── 5. Platform gating: a native fan-out ────────────────────────────────────
record optional-platform-gated \
"esbuild@0.24.0 fans out to twenty-four platform-gated optionalDependencies, and a bun.lock records ALL of them with their os/cpu meta. This is the case that proves the resolver does the same (R773-F9): a lock is portable, so resolution records the platform gates instead of applying them and the tree matches bun entry for entry on every machine. It caught the opposite rule directly - under R773-F6 the resolver kept only the host's one variant and diverged from bun by twenty-three entries." <<'EOF'
{
  "name": "optional-platform-gated",
  "version": "1.0.0",
  "dependencies": { "esbuild": "0.24.0" }
}
EOF

# ── 6. Platform gating from the root's own optionalDependencies ─────────────
record optional-dep-excluded-here \
"fsevents is darwin-only and sits in the root's optionalDependencies. bun locks it anyway, with its os meta, and so must the resolver: a lock is portable and the install-time filter is what drops it on a linux machine (R773-F9). A resolver that still gated at resolve time would drop it here and diverge from bun by exactly this entry." <<'EOF'
{
  "name": "optional-dep-excluded-here",
  "version": "1.0.0",
  "dependencies": { "chalk": "^5.3.0" },
  "optionalDependencies": { "fsevents": "^2.3.3" }
}
EOF

# ── 7. Dist-tags resolve before ranges ──────────────────────────────────────
record dist-tag \
"Both dependencies are specified as latest, which resolves through the packument's dist-tag table before any range is considered. Frozen at record time, which is exactly the behaviour wanted: the fixture records what latest meant then, and the replay must still agree with the lock minted at the same instant." <<'EOF'
{
  "name": "dist-tag",
  "version": "1.0.0",
  "dependencies": { "ms": "latest", "escape-string-regexp": "latest" }
}
EOF

# ── 8. A scoped package with a genuinely deep tree ──────────────────────────
record scoped-deep \
"@babel/core is scoped, deep, and full of @babel/* packages that share transitive dependencies at different ranges — so it exercises the dedupe path at a scale hand-written cases do not reach, and it is the case where a / in a package name has to survive the cache's %2f mangling." <<'EOF'
{
  "name": "scoped-deep",
  "version": "1.0.0",
  "dependencies": { "@babel/core": "^7.24.0" }
}
EOF

# ── 9. npm: aliases — install name is not registry name ─────────────────────
record npm-alias \
"An npm: alias installs lodash into a directory called lodash-alias. The install path carries the alias and the lock carries the registry name, which is the one place those two vocabularies provably differ — a diff keyed on the wrong one passes every other case in this corpus." <<'EOF'
{
  "name": "npm-alias",
  "version": "1.0.0",
  "dependencies": { "lodash-alias": "npm:lodash@^4.17.0", "chalk": "^5.3.0" }
}
EOF

# ── 10. The range grammar itself, across its forms ──────────────────────────
record range-forms \
"Six dependencies spanning tilde, x-ranges, a two-sided comparator range, a bare wildcard, an exact pin and a caret. Range-semantics divergence from pnpm is the specific failure R773-F7 exists to catch, and this is the case that catches it directly rather than as a side effect of some other package's tree." <<'EOF'
{
  "name": "range-forms",
  "version": "1.0.0",
  "dependencies": {
    "semver": "~7.5.0",
    "ms": "2.x",
    "debug": ">=4.3.0 <4.4.0",
    "escape-string-regexp": "*",
    "ansi-regex": "5.0.1",
    "strip-ansi": "^6.0.0"
  }
}
EOF

# ── 11-13. Real-world manifests, which is where the surprises live ──────────
record real-express \
"A real express@4 tree: ~30 packages, several shared transitives, and a long tail of tiny single-purpose modules. W318 §7's whole point is that arbitrary npm — not this camp's curated fifteen — is what the resolver has to survive." <<'EOF'
{
  "name": "real-express",
  "version": "1.0.0",
  "dependencies": { "express": "^4.19.2" }
}
EOF

record real-vite \
"vite@5 combines everything the synthetic cases isolate: a deep tree, esbuild's and rollup's platform-gated native fan-outs, and peer dependencies. The single best value-per-byte case in the corpus, and the one most likely to catch a regression nobody predicted." <<'EOF'
{
  "name": "real-vite",
  "version": "1.0.0",
  "dependencies": { "vite": "^5.4.0" }
}
EOF

record real-typescript-toolchain \
"A plausible outside consumer's devDependencies rather than a single library: typescript, eslint and prettier together. devDependencies are followed for the ROOT only, so a resolver that walked a dependency's devDependencies would explode here and nowhere else." <<'EOF'
{
  "name": "real-typescript-toolchain",
  "version": "1.0.0",
  "devDependencies": {
    "typescript": "^5.5.0",
    "prettier": "^3.3.0",
    "rimraf": "^5.0.0"
  }
}
EOF

echo
if [ "$FAIL" -eq 0 ]; then
  echo "PASSED: $PASS cases recorded, 0 failed"
  exit 0
fi
echo "FAILED: $PASS recorded, $FAIL diverged"
echo "A divergence here is a real finding — fix the resolver, or record it as a"
echo "[[known_divergence]] in that case's case.toml with the reason it is acceptable."
exit 1
