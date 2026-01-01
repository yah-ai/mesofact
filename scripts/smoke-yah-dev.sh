#!/usr/bin/env bash
# mesofact-static end-to-end smoke (R010-T1, P8 dogfood).
#
# Builds examples/yah-dev/ with @mesofact/build, runs mesofact-publish
# --in-memory against the resulting dist/, and asserts the HTML, manifest,
# and tag-index match what Mode 1 promised. Real-network R2/Cloudflare
# publish lives in .github/workflows/publish-smoke.yml (R008-T7); this
# script is the no-creds local equivalent.
#
# The example used to host yah.dev marketing copy; that moved to yah's
# app/yah/web/ (R254). This example is now mesofact's own minimal
# showcase + smoke target — assertions below match the trimmed content.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
EXAMPLE="$ROOT/examples/yah-dev"
DIST="$EXAMPLE/dist"

step() { printf '==> %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

step "cleaning $DIST"
rm -rf "$DIST"

step "building examples/yah-dev"
(cd "$ROOT" && bun run packages/mesofact-build/src/cli.ts examples/yah-dev > /dev/null)

step "publishing dist/ to in-memory store"
(cd "$ROOT" && cargo run --quiet -p mesofact-publisher --bin mesofact-publish -- \
  --in-memory examples/yah-dev/dist > /dev/null)

step "asserting dist/html/* content"
[ -f "$DIST/html/index.html" ] || fail "missing dist/html/index.html"
[ -f "$DIST/html/404.html" ]   || fail "missing dist/html/404.html"

grep -q 'mesofact · hello'                  "$DIST/html/index.html" \
  || fail "index.html missing landing title"
grep -q 'hello from mesofact'               "$DIST/html/index.html" \
  || fail "index.html missing landing heading"
grep -q '<h1>404</h1>'                      "$DIST/html/404.html" \
  || fail "404.html missing 404 heading"
grep -q 'Back to the example'               "$DIST/html/404.html" \
  || fail "404.html missing back link"

step "asserting manifest shape"
MANIFEST="$DIST/manifest.json"
[ -f "$MANIFEST" ] || fail "missing manifest.json"
ROUTES=$(bun -e 'console.log(JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).routes.map(r=>r.route+":"+r.mode).join(","))' "$MANIFEST")
[ "$ROUTES" = "/:static,/404:static,/app:spa" ] \
  || fail "unexpected route list: $ROUTES"

step "asserting Mode 3 (/app) hydration: manifest + dist/hydrate + woven shell"
# manifest.hydration.script is content-hashed and the hashed file exists.
HYDRATE_SCRIPT=$(bun -e '
  const m = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
  const app = m.routes.find((r) => r.route === "/app");
  if (!app) { console.error("no /app route"); process.exit(1); }
  if (app.mode !== "spa") { console.error("/app not spa"); process.exit(1); }
  const h = app.hydration;
  if (!h || !/^app\.[0-9a-z]+\.js$/.test(h.script) || !Array.isArray(h.code_split)) {
    console.error("bad hydration block: " + JSON.stringify(h)); process.exit(1);
  }
  console.log(h.script);
' "$MANIFEST")
[ -f "$DIST/hydrate/$HYDRATE_SCRIPT" ] || fail "missing dist/hydrate/$HYDRATE_SCRIPT"

# The shell carries the serialized initial state + the module <script> pointing
# at /{build_id}/hydrate/<script>.
APP_HTML="$DIST/html/app.html"
[ -f "$APP_HTML" ] || fail "missing dist/html/app.html"
grep -q '<script id="__MESOFACT_STATE__" type="application/json">' "$APP_HTML" \
  || fail "app.html missing __MESOFACT_STATE__ tag"
grep -q 'hydrated from __MESOFACT_STATE__' "$APP_HTML" \
  || fail "app.html missing serialized initial state"
grep -q "<script type=\"module\" src=\"/[^\"]*/hydrate/$HYDRATE_SCRIPT\"></script>" "$APP_HTML" \
  || fail "app.html missing hydrate module script"

step "asserting tag-index"
TAG_INDEX="$DIST/tag-index.json"
[ -f "$TAG_INDEX" ] || fail "missing tag-index.json"
bun -e '
  const ti = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
  const want = {"page:home": ["/"], "page:404": ["/404"], "site:mesofact-example": ["/", "/404"]};
  for (const [tag, urls] of Object.entries(want)) {
    const got = ti.tags[tag];
    if (!got || JSON.stringify(got) !== JSON.stringify(urls)) {
      console.error(`tag-index mismatch for ${tag}: want ${JSON.stringify(urls)} got ${JSON.stringify(got)}`);
      process.exit(1);
    }
  }
' "$TAG_INDEX"

echo "==> ✓ mesofact-static smoke: build → in-memory publish → HTML/manifest/tag-index all match"
