import type { RenderFn } from "@mesofact/runtime";

// Inference will pick `project_blobs` — declared as `scope = "project"` in
// this fixture's mesofact.config.toml. validate() must reject before any
// HTML is written.
export const render: RenderFn = async () => {
  // r2('project_blobs') is what the regex picks up; we don't actually invoke
  // the adapter at test time (validation fails first).
  return { html: "should-not-render", cache: { ttl: 60 } };
};

// keep the inference match in source even though we don't run it
// @mesofact-sources project_blobs
