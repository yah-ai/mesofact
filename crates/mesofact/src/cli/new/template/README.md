# __PROJECT_NAME__

A mesofact project. Two binaries, no package manager, no Node.

## Run it

```sh
mes --port 3000
```

That is the whole loop. `mes` with no subcommand builds the project in-process
on startup, serves it, and rebuilds on every edit under `src/`. The first build
materializes `node_modules/` from the committed `bun.lock` — it fetches
tarballs from the npm registry and verifies each against the sha512 integrity
in the lock, which is why it needs no `npm`, `bun`, `pnpm` or `node` anywhere
on `PATH`.

## The two binaries

`mes` is the toolchain you use while developing; `mesofact` is what runs in
production. `mes` is a superset — it carries the prod verbs too, so there is
one CLI to learn — but the reverse is not true, and deliberately so: the
shipped `mesofact` binary never links a file watcher, a dev S3 surface or a
bundler.

| command | what it does |
|---|---|
| `mes` | `mes dev .` — the loop above. What you type 95% of the time. |
| `mes build` | One-shot build into `dist/`. No watcher, no server. |
| `mes serve` | Serve a built bundle / host SSR routes — the prod path, locally. |
| `mes new <dir>` | Scaffold another project at this same version. |
| `mes publish` | Upload `dist/`, swap the manifest pointer, purge CDN tags. |
| `mesofact …` | The same `serve` / `publish`, plus `proxy`, with no dev weight. |

`mesofact-dev` is the old name for `mes` and still works; it will go away.

```sh
curl -s localhost:3000/                        # static — identical every time
curl -s 'localhost:3000/api/hello?name=you'    # {"hello":"you","at":…,"method":"GET"}
curl -s -X PUT localhost:3000/api/hello        # 405 — the handler owns its status
```

## The version pin

`.mesofact-version` selects which mesofact this project builds and serves
with. The shim on your `PATH` reads it on every invocation, so `mes` and
`mesofact` in this directory are always the pair that version names — switch
with `mesofact-vm use <version>`, or per-project by editing that file.

The pin covers the **JS set too**. A mesofact version pins react, react-dom,
the `@mesofact/runtime` barrel and the matching `@types/*` at exact versions,
tested together per release, which is why `bun.lock` is committed and was
never resolved on your machine. One dial, honest for the binaries and for the
app they serve.

Reaching outside that set — adding an arbitrary npm package — is supported but
carries no compatibility promise. It is the escape hatch, not the main road.

## Layout

| path | what it is |
|---|---|
| `mesofact.routes.ts` | The whole routing surface. A **build input**, not runtime config: it decides what is prerendered, what is bundled for the server, and which prefixes SSR owns. Changing a route's mode means rebuilding. |
| `src/home.tsx` | `mode: "static"` — exports `render: RenderFn`, runs at build time, returns `{ html, cache }`. |
| `src/api.ts` | `mode: "ssr"` — a Fetch handler run per request in a V8 isolate. Owns status, headers, body. |
| `src/Page.tsx` | The shared component both modes render through. |
| `bun.lock` | The shipped, pre-resolved lock. Nothing on this path resolves. |
| `vendor/mesofact-runtime/` | `@mesofact/runtime` **types**, vended at your mesofact version. The barrel that runs is compiled into the binary; imports of it stay external through every bundler path. |
| `workload.toml` | Read by the deploy path only. The local loop ignores it. |

## Typechecking

`npm run typecheck` (or `bun run typecheck`) runs `tsc --noEmit`. TypeScript
is in the curated set and installs with everything else — but running it does
need a JS runtime, so this is the one command in this README that wants `node`
or `bun` present. The build and the dev server do not.
