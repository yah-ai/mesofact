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
  test("sorts param keys for determinism", () => {
    const a = prerenderKey("/p/:id", { id: "42" });
    expect(a).toBe("p_id__42");
    // Same param set, different insertion order → same key.
    const b = prerenderKey("/p/:id/:k", { k: "x", id: "1" });
    const c = prerenderKey("/p/:id/:k", { id: "1", k: "x" });
    expect(b).toEqual(c);
  });
});
