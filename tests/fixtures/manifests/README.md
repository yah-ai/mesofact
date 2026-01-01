# Manifest fixtures

Hand-written manifest JSONs exercised by both the TS validator
(`packages/mesofact-runtime/src/validate.ts`) and the Rust validator
(`crates/mesofact/src/validate.rs`). Adding a rule means adding a fixture
here — both test suites pick it up automatically.

Each fixture is a JSON file shaped like:

```jsonc
{
  "sources": { "<name>": { "scope": "global" | "project" | "user" } },
  "manifest": { ...manifest payload... },
  "expect": "ok" | { "errors": [{ "kind": "<ValidationErrorKind>" }, ...] }
}
```

`expect.errors` is matched as a set (order-insensitive, length-equal). Only
the `kind` is asserted — the path/message strings live in the validators and
are not part of the contract here.

Naming: `accept_*.json` for valid fixtures, `reject_<rule>.json` for failing
ones. One file per rule plus at least one positive.
