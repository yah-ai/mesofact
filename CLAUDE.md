@.yah/CLAUDE.md

# mesofact

Tri-mode web server: static (push-to-CDN) / SSR (Bun render pool behind a Rust
proxy) / SPA shell — one project, one render contract, per-route mode selection.
See [`.yah/docs/working/mesofact.md`](.yah/docs/working/mesofact.md) for the
design doc.

This workspace lives under `yah/external/mesofact/` during early iteration but
is intended to move to its own repo later. yah's root `.gitignore` excludes
`external/`, so this tree carries (or will carry) independent git history.

First dogfood: the yah.dev marketing page (Mode 1 / static / R2).

This workspace is also a **yah subcamp** — `.yah/` here is canonical for
mesofact-scoped tickets, sessions, and arch docs, separate from the parent yah
camp at `/Users/user/ss/yah/.yah/`.