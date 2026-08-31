# hello — every mesofact mode, once each

Hello world in TSX, one route per mode, runnable on a laptop. This is the
"does this thing actually work on my machine" example; `examples/yah-dev` is
the dependency-free smoke target and stays that way.

| route | mode | what runs, and when |
|---|---|---|
| `/` | `static` | Rendered once at **build** time into `dist/html/index.html`. Nothing runs per request. |
| `/404` | `static` | Same, wired as the 404 error route. |
| `/live` | `ssr` | A Fetch handler run **per request** in a V8 isolate. Returns HTML. |
| `/api/hello` | `ssr` | Same contract, returns JSON. This is what an `/api/*` route looks like. |
| `/app` | `spa` | A shell prerendered like `static`, plus a browser bundle that hydrates it. |

`ssg` (publish-time incremental rerender) is deliberately absent — it needs a
blob store and a front door, which is a deployment shape, not a laptop one.

## Run it

Two binaries and no Node. `mesofact-build` bundles and prerenders;
`mesofact-dev` serves and watches.

```sh
cd oss/mesofact
bun install                                    # once, for the workspace
cargo build -p mesofact-build --bin mesofact-build   # once; `ssr` is on by
cargo build --release -p mesofact-dev                # default for mesofact-dev

./target/debug/mesofact-build build examples/hello
./target/release/mesofact-dev examples/hello --port 4321
```

Then:

```sh
curl -s localhost:4321/                        # static — identical every time
curl -s localhost:4321/live                    # ssr — the timestamp moves
curl -s 'localhost:4321/live?name=you'         # ssr — the query reaches the handler
curl -s 'localhost:4321/api/hello?name=you'    # {"hello":"you","at":…,"method":"GET"}
curl -s -X PUT localhost:4321/api/hello        # 405 — the handler owns its status
curl -s localhost:4321/app | grep hydrate      # spa — the woven <script type=module>
```

Drop `--no-watch` (the default is to watch) and edits to `src/*.tsx` rebuild and
reload. `--port` is the only flag you need; the server binds `127.0.0.1`.

**No JS runtime is needed to serve this.** `mesofact-dev` runs `ssr` routes in
an in-process V8 isolate (R449-F2) — the bun-child era is over, and the claim
that used to sit here that `bun` must be on `PATH` was stale. Verified
2026-08-28 by serving a scaffolded project's `mode:"ssr"` route on a PATH with
no `node`/`bun`/`npm`/`deno` reachable (`scripts/check-mesofact-new.sh`).

`bun install` above is still needed *for this example specifically*, because it
is a workspace member whose `@mesofact/runtime` is a `workspace:*` link. A
standalone project does not need it: `mesofact-dev` materializes
`node_modules` from the project's own lockfile itself.

## The three modes, in one sentence each

**`static`** exports `render: RenderFn` and returns `{ html, cache }`. It runs
at build time with no request in hand — `req.data` carries build-time data
inputs and that is all it gets. Cheapest thing mesofact can serve: a file.

**`ssr`** exports `default async (req: Request) => Response` — the Deno /
Worker / Bun handler shape, **not** `RenderFn`. It owns the whole response:
status, headers, body. Use it when the answer depends on the request.

**`spa`** is two files that must agree. The shell (`entrypoint`) renders at
build time with `renderToString` and ships `hydration.initial_state`; the
client (`client_entrypoint`) reads that state out of the
`<script id="__MESOFACT_STATE__">` tag and calls `hydrateRoot`. Both import the
same `App` component — that shared import is what keeps them from disagreeing,
and disagreement is the entire failure mode of hydration.

## Deploying it: the vanilla bundle

The same tree assembles into a W272 bundle that carries **no binary** and names
the runtime it needs, which a node fetches once and shares across every site at
that version:

```sh
yah cloud bundle build examples/hello --out /tmp/hello-bundle \
  --name hello --runtime-version 0.8.22
./target/debug/mesofact serve --bundle /tmp/hello-bundle --listen 127.0.0.1:4322
```

`manifest.toml` says `runtime = "mesofact/0.8.22"` and there is no `bins/`
directory — that is what "vanilla" means.

> **`mesofact serve --bundle` is static-only today (v0).** Measured against
> this example: `/` and `/app` serve 200, `/live` and `/api/hello` return
> **404**. The bundle arm of `serve` returns before any SSR attach
> (`cli/serve.rs:143`), so this is structural, not a missing build feature — a
> runtime built `--features deploy` behaves the same. Executing
> `mesofact.routes.ts` from a bundle is R599-F6 follow-on work. Until it lands,
> a site with `mode:"ssr"` routes loses them when it moves to the bundle tier,
> silently and with a 404 that looks like a routing typo.

## Making it your own

`mesofact new <dir>` scaffolds this shape outside the monorepo — same route
modes, plus a pre-resolved `bun.lock` pinned to that mesofact version and a
`.mesofact-version` to go with it, so `mesofact-dev .` is the whole loop with
no package manager involved. Copying this directory works too, but it is a
workspace member, so its `@mesofact/runtime` link and its
`[build] command = "mesofact-build build ."` come along and neither is right
outside the repo.

The pieces that matter:

- `mesofact.routes.ts` — the whole config surface. It is a **build input**, not
  runtime config: it decides what gets prerendered, which entrypoints are
  bundled for server vs browser, and which prefixes the SSR runtime owns.
  Changing a route's mode means rebuilding, not restarting.
- `tsconfig.json` — `"jsx": "react-jsx"`. TSX works because the bundler
  resolves `react/jsx-runtime`; there is no mesofact-specific JSX setup.
- `package.json` — React 18 is this example's choice, not a mesofact
  requirement. `render` returns an HTML **string**, so any renderer that can
  produce one works. `examples/yah-dev` uses template literals and no framework
  at all.
- `workload.toml` — only the deploy path reads it. The local loop doesn't.
