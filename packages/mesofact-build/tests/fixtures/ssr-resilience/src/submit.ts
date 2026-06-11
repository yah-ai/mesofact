// SSR Fetch handler — the resilience block wraps the proxy hop in front of
// this handler; the handler itself stays a dumb single-shot origin.
export default async function (_req: Request): Promise<Response> {
  return new Response(JSON.stringify({ accepted: true }), {
    status: 200,
    headers: { "content-type": "application/json" },
  });
}
