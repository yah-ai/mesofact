export default async function (req: Request): Promise<Response> {
  const url = new URL(req.url);
  const id = url.pathname.split("/").pop() ?? "unknown";
  return new Response(JSON.stringify({ id }), {
    status: 200,
    headers: { "content-type": "application/json" },
  });
}
