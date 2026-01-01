// `pg` is on EDGE_FORBIDDEN — workerd has no node:net so the native bindings
// can't link. The lint must catch this before the bundle is shipped. `pg`
// isn't installed in the test fixture; Bun.build's onResolve still sees the
// specifier and the lint surfaces a useful error.
// @ts-expect-error fixture intentionally imports an uninstalled module.
import { Pool } from "pg";

export default async function (_req: Request): Promise<Response> {
  const pool = new Pool();
  return new Response(JSON.stringify({ ok: typeof pool }));
}
