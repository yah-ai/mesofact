// W173 Universal SSR handler. Renders HTML per request and inlines
// hostile-looking data through hydrationDataTag's escape. The fixture uses
// values containing `</script>` and `<b>` to exercise the XSS rule.
import { hydrationDataTag, hydrationScriptTag } from "@mesofact/runtime";

export default async function (_req: Request): Promise<Response> {
  const data = {
    user: "ada",
    count: 7,
    label: "<b>html in data</b>",
    evil: "</script><img src=x onerror=alert(1)>",
  };
  // In a real consumer this URL would be derived from the manifest's
  // hydration.script + build_id; the fixture hardcodes a placeholder since
  // the test asserts on the tag shape, not on resolution.
  const body =
    `<!doctype html><html><body>` +
    `<div id="root"></div>` +
    hydrationDataTag(data) +
    hydrationScriptTag("/build-test/hydrate/dashboard.test.js") +
    `</body></html>`;
  return new Response(body, {
    status: 200,
    headers: { "content-type": "text/html; charset=utf-8" },
  });
}
