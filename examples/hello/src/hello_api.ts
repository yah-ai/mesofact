// Mode 2 — `ssr`, the JSON-API shape. Same Fetch-handler contract as
// src/live.tsx; no React, no HTML. This is what a `/api/*` route looks like.

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
