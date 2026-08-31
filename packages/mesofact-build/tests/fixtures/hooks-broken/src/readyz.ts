// Wrong shape: a named export instead of `export default`. Nothing the engine
// can invoke.
export const readyz = async () => new Response("ok");
