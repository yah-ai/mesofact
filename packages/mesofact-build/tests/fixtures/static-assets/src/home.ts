import type { RenderRequest, RenderResult } from "@mesofact/runtime";

export function render(_req: RenderRequest): RenderResult {
  return {
    html: "<!doctype html><html><body><h1>overlay test</h1></body></html>",
    cache: { ttl: 3600 },
  };
}
