# __PROJECT_NAME__

A mesofact service on the **library tier**: your routes are Rust handlers, and
this crate builds its own two binaries. One crate, one route table, two thin
bin targets.

## Run it

```sh
cargo run --bin __PROJECT_NAME__-dev      # the dev binary, on 127.0.0.1:3000
curl -s localhost:3000/
curl -s localhost:3000/api/hello
```

Your routes are Rust, so an edit needs a re-link — there is no in-process
hot reload, and there cannot be one. The loop is:

```sh
cargo watch -x 'run --bin __PROJECT_NAME__-dev'
```

## The two binaries

`cargo build --release` emits both, release-optimized, with **no flags**:

| binary | what it is |
|---|---|
| `__PROJECT_NAME__` | What you ship. `mesofact::serve_app` — your router, the `/livez` + `/readyz` probes, tracing, graceful shutdown. Binds `0.0.0.0`. |
| `__PROJECT_NAME__-dev` | What you run locally. Everything above, plus a local FS-backed object store standing in for R2, with its coordinates published into the process environment. Binds `127.0.0.1`. |

Dev/prod here is a **link-graph** distinction, never a build profile and never
a feature flag. `src/lib.rs` depends on `mesofact` and never on
`mesofact-dev`, so the prod binary's reachable graph cannot contain dev code —
there is no option to forget. The one edit that breaks this is a
`use mesofact_dev` in `src/lib.rs`, and it breaks it silently, so it is worth
knowing that is the thing to watch for in review.

Do not collapse the two into one binary behind a `dev` feature. Default-prod
means typing `--features dev` on every build; default-dev means one forgotten
`--no-default-features` ships a binary with dev affordances in it. Two bins has
no default mode to get wrong.

## The local object store

`__PROJECT_NAME__-dev` starts an S3-compatible surface backed by
`.mesofact-dev/s3/` and exports `R2_ENDPOINT`, `R2_BUCKET`,
`R2_ACCESS_KEY_ID` and `R2_SECRET_ACCESS_KEY` before serving. A handler that
builds its object store from those variables therefore talks to a local bucket
in dev and to real R2 in prod, with no branch in the handler.

Coordinates are also written to `.mesofact-dev/s3.json`, so out-of-process
tools can reach the same bucket:

```sh
aws s3 --endpoint-url "$(jq -r .endpoint .mesofact-dev/s3.json)" ls s3://dev/
```

## Layout

| path | what it is |
|---|---|
| `src/lib.rs` | `router()` and the handlers. The core, in-crate. Never references `mesofact_dev`. |
| `src/bin/__PROJECT_NAME__.rs` | The prod target. Three lines over `router()`. |
| `src/bin/__PROJECT_NAME__-dev.rs` | The dev target. The same three lines, one identifier different. |
| `.github/workflows/ci.yml` | Release-builds both, uploads only the prod binary. |

## Versioning

`Cargo.toml` is the dial: `mesofact` and `mesofact-dev` are pinned there under
cargo's ordinary semver rules, and they must name the same version.

This is the tier for services whose routes are Rust. If you want TypeScript
routes — prerendered pages, SSR handlers in a V8 isolate, a bundler — that is
the *standalone* tier and a different scaffold: `mesofact new <dir>` with no
`--lib`. It needs no consumer Rust at all.
