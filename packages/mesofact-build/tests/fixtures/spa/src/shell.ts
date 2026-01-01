import type { RenderFn } from "@mesofact/runtime";

// Mode 3 shell: a minimal HTML document with an empty mount point and an
// initial state the client hydrates against. The build weaves in the
// __MESOFACT_STATE__ tag + the hydrate <script>; the render only ships state.
export const render: RenderFn = async () => ({
  html:
    "<!doctype html><html><head><title>spa fixture</title></head>" +
    '<body><div id="root"></div></body></html>',
  cache: { ttl: 0 },
  hydration: {
    initial_state: { count: 7, label: "from <b>build</b> & shell" },
  },
});
