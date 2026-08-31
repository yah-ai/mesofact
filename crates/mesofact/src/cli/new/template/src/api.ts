// `ssr` — runs ONCE PER REQUEST inside a V8 isolate the runtime owns.
//
// The contract is a plain Fetch handler — `(Request) => Response`, the same
// shape as a Deno / Cloudflare Worker / Bun handler — NOT the `RenderFn` the
// static modes export. That difference is the point: a static render answers
// "what does this page look like", an SSR handler answers "what is the response
// to THIS request", so it gets the whole request and owns the whole response.

export default async function (req: Request): Promise<Response> {
  if (req.method !== "GET" && req.method !== "POST") {
    return new Response("method not allowed", {
      status: 405,
      headers: { allow: "GET, POST" },
    });
  }

  const name =
    req.method === "POST"
      ? ((await req.json().catch(() => ({}))) as { name?: string }).name
      : new URL(req.url).searchParams.get("name");

  return Response.json({
    hello: name ?? "world",
    at: new Date().toISOString(),
    method: req.method,
  });
}
