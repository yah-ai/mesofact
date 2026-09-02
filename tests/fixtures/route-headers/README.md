# Route-header parity fixture

`parity.json` is the **one** description of what a domain's declared per-route
response headers must do, exercised against **both front doors**:

| Front door | Suite | Entry point |
|---|---|---|
| `front_door = "worker"` | `packages/mesofact-edge/tests/router.test.ts`, describe `route-header front-door parity fixture` (miniflare) | `ROUTE_HEADERS` binding → `applyRouteHeaders` |
| `front_door = "passway"` | `crates/mesofact/tests/route_headers_parity.rs` (axum) | `MESOFACT_ROUTE_HEADERS` → `Server::with_route_headers` |

Why one fixture rather than two test files that happen to agree: a domain that
flips its `front_door` must not change what the browser receives. Before R749-F3
only the Worker read the table, so the flip silently dropped `COOP`/`COEP` —
correct bytes, 200 OK, and `SharedArrayBuffer` undefined on the global object,
which is a dead wasm app with no server-side symptom (W334). Two suites that
each assert their own door's behaviour cannot catch that; one fixture asserted
by both is exactly the non-equivalence detector `front_door` (R594-F12) was
introduced to provide.

Shape:

```jsonc
{
  "table":  [ { "path": "/app/*", "headers": { … } }, … ],  // verbatim the JSON
                                                            // DomainConfig::route_headers_json emits
  "assets": { "<key>": { "body": "…", "type": "…" } },       // the served tree
  "cases":  [ { "name", "path", "status", "expect": {…}, "absent": [ … ] } ]
}
```

`expect` is a header name → value map that must be present on the response;
`absent` names headers that must NOT be (this is how "first match wins, no
merging across rules" is asserted rather than assumed). Add a case here and
both doors pick it up.
