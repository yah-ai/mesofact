// Hydration handoff — helpers an SSR route's Fetch handler calls to inline
// server-resolved data into the response body for the client to read on
// mount. Disposable when RSC streaming lands; until then this is the
// Universal cell's contract (W173 § "Hydration handoff").
//
// API surface is intentionally minimal — two pure functions. The handler
// composes them into its own HTML response, which means the consumer keeps
// full control over the document shell. The trade-off is that the consumer
// has to know its own build_id + hashed client-script name (read from the
// manifest at boot, or inject via env). Acceptable for the dogfood window;
// revisit when more consumers exist.

// Build-time SPA shell consumes this id; the prerender weaves it in.
export const SPA_STATE_SCRIPT_ID = "__MESOFACT_STATE__" as const;

// Per-request SSR Universal handoff consumes this id; see W173 § "Hydration
// handoff" — the name is fixed (not configurable) so the client snippet can
// be copy-pasteable across consumers.
export const SSR_DATA_SCRIPT_ID = "__mesofact_data__" as const;

// JSON-encode `value` and escape the characters that could close the parent
// `<script>` tag early or be reinterpreted by the HTML parser:
//
//   `<`, `>`, `&`         — `</script>`, `<!--`, `<![CDATA[`, `&amp;` injection
//   U+2028 / U+2029       — JS line separators valid in JSON but break inline
//                           scripts when not escaped
//
// `JSON.parse` decodes the `\uXXXX` escapes transparently, so the client
// reads back the original value via `JSON.parse(el.textContent)` with no
// special handling. Non-negotiable per W173 § "XSS escape rule".
export function escapeJsonForScriptTag(value: unknown): string {
  return JSON.stringify(value).replace(/[<>&\u2028\u2029]/g, (c) => {
    switch (c) {
      case "<":
        return "\\u003c";
      case ">":
        return "\\u003e";
      case "&":
        return "\\u0026";
      case "\u2028":
        return "\\u2028";
      case "\u2029":
        return "\\u2029";
      default:
        return c;
    }
  });
}

// Serialize `data` into the data-handoff `<script>` tag an SSR route's
// Fetch handler inlines into its response body. Read back on the client via
// `JSON.parse(document.getElementById("__mesofact_data__").textContent)`.
export function hydrationDataTag(data: unknown): string {
  return `<script id="${SSR_DATA_SCRIPT_ID}" type="application/json">${escapeJsonForScriptTag(data)}</script>`;
}

// Build the module-script tag that loads the route's hydrate bundle. `src`
// is escaped against `"`, `<`, and `&` so a manifest-derived URL containing
// a stray quote can't break the attribute or the tag boundary; belt-and-
// braces — the manifest paths the build emits don't contain quotes, but
// consumers may compose paths from request data.
export function hydrationScriptTag(src: string): string {
  const safeSrc = src.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;");
  return `<script type="module" src="${safeSrc}"></script>`;
}
