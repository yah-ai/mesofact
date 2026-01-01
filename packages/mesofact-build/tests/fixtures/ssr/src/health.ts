// SSR Fetch handler — identical signature under Bun and workerd.
export default async function (_req: Request): Promise<Response> {
  return new Response(JSON.stringify({ status: "ok" }), {
    status: 200,
    headers: { "content-type": "application/json" },
  });
}
