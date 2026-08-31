import { describe, expect, test } from "bun:test";
import { prerenderKey, routeKey } from "../src/route-key.js";

describe("routeKey", () => {
  test("maps root to 'index'", () => {
    expect(routeKey("/")).toBe("index");
  });

  test("strips slashes and param markers", () => {
    expect(routeKey("/about")).toBe("about");
    expect(routeKey("/p/:id")).toBe("p_id");
    expect(routeKey("/blog/:slug/*")).toBe("blog_slug_star");
  });
});

describe("prerenderKey", () => {
  // R600-B1: the emission is named by the path it serves at, so
  // `dist/html/<key>.html` is what `assetCandidates(path)` asks the origin for.
  // Keep this table in step with the Rust twin
  // (crates/mesofact-render/src/route_key.rs::prerender_key_is_the_public_path).
  test("is the public path minus its leading slash", () => {
    expect(prerenderKey("/", "/")).toBe("index");
    expect(prerenderKey("/releases", "/releases")).toBe("releases");
    // Nested literal routes were flattened to `blog_nested` and 404'd the same
    // way parametric instances did.
    expect(prerenderKey("/blog/nested", "/blog/nested")).toBe("blog/nested");
    expect(prerenderKey("/docs/", "/docs/")).toBe("docs/index");
    expect(prerenderKey("/p/:id", "/p/42")).toBe("p/42");
    expect(prerenderKey("/issues/:id", "/issues/01KZVGVT0DV61ZGGNVHAWQW2CS")).toBe(
      "issues/01KZVGVT0DV61ZGGNVHAWQW2CS",
    );
    expect(prerenderKey("/x/:a/:b", "/x/1!/2")).toBe("x/1!/2");
  });

  test("percent-encoded param values stay inside their segment", () => {
    // expandRoute runs encodeURIComponent, so a value can neither escape its
    // path segment nor forge an unexpanded-param marker.
    expect(prerenderKey("/p/:id", "/p/a%2Fb")).toBe("p/a%2Fb");
    expect(prerenderKey("/p/:id", "/p/%3Aslug")).toBe("p/%3Aslug");
  });

  test("patterns with no single public path keep the flat routeKey name", () => {
    // SPA shell: rendered once, with no params, for a parametric route —
    // expandRoute returns the pattern verbatim.
    expect(prerenderKey("/item/:id", "/item/:id")).toBe("item_id");
    expect(prerenderKey("/blog/:slug/*", "/blog/x/*")).toBe("blog_slug_star");
  });
});
