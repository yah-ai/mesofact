// Mode 2 — `ssr`. Runs ONCE PER REQUEST, inside a V8 isolate the runtime owns.
//
// The contract is a plain Fetch handler — `(Request) => Response`, the same
// shape as a Deno / Cloudflare Worker / Bun handler — NOT the `RenderFn` the
// static and spa modes export. That difference is the point: a static render
// answers "what does this page look like", an SSR handler answers "what is the
// response to THIS request", so it gets the whole request and owns the whole
// response (status, headers, body).
//
// Reload it: the timestamp changes. That is the only observable difference
// between this file and src/home.tsx, and it is the entire distinction.

import { renderToStaticMarkup } from "react-dom/server";

import { Page, documentOf } from "./Page.js";

export default async function (req: Request): Promise<Response> {
  const url = new URL(req.url);
  const name = url.searchParams.get("name") ?? "world";

  const html = documentOf(
    renderToStaticMarkup(
      <Page title={`hello, ${name}`} mode="ssr — rendered per request">
        <p>
          Rendered at <code>{new Date().toISOString()}</code>. Reload and the
          timestamp moves.
        </p>
        <p>
          Try <code>/live?name=you</code> — the query string reaches the handler
          because SSR gets the real <code>Request</code>.
        </p>
      </Page>,
    ),
  );

  return new Response(html, {
    status: 200,
    headers: {
      "content-type": "text/html; charset=utf-8",
      // Per-request output that a CDN must not hold onto.
      "cache-control": "no-store",
    },
  });
}
