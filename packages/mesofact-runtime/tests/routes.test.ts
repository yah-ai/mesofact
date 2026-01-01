// defineRoutes schema validation — placement axis (R015-F1 / W173).

import { describe, expect, test } from "bun:test";
import { defineRoutes } from "../src/index.js";

describe("defineRoutes — placement validation", () => {
  test("accepts ssr route with placement: host", () => {
    const cfg = defineRoutes({
      routes: [
        {
          route: "/api/health",
          mode: "ssr",
          entrypoint: "src/health.ts",
          placement: "host",
          cache_policy: { ttl: 0 },
        },
      ],
    });
    expect(cfg.routes[0].placement).toBe("host");
  });

  test("accepts ssr route with placement: edge", () => {
    const cfg = defineRoutes({
      routes: [
        {
          route: "/api/edge",
          mode: "ssr",
          entrypoint: "src/edge.ts",
          placement: "edge",
          cache_policy: { ttl: 60 },
        },
      ],
    });
    expect(cfg.routes[0].placement).toBe("edge");
  });

  test("accepts ssr route with placement: auto", () => {
    const cfg = defineRoutes({
      routes: [
        {
          route: "/api/auto",
          mode: "ssr",
          entrypoint: "src/auto.ts",
          placement: "auto",
          cache_policy: { ttl: 0 },
        },
      ],
    });
    expect(cfg.routes[0].placement).toBe("auto");
  });

  test("accepts ssr route with no placement (defaults at build time)", () => {
    const cfg = defineRoutes({
      routes: [
        {
          route: "/api/default",
          mode: "ssr",
          entrypoint: "src/default.ts",
          cache_policy: { ttl: 0 },
        },
      ],
    });
    expect(cfg.routes[0].placement).toBeUndefined();
  });

  test("rejects placement on a static route", () => {
    expect(() =>
      defineRoutes({
        routes: [
          {
            route: "/",
            mode: "static",
            entrypoint: "src/render.ts",
            placement: "host",
            cache_policy: { ttl: 3600 },
          },
        ],
      }),
    ).toThrow(/placement is only valid on mode:"ssr"/);
  });

  test("rejects placement on a spa route", () => {
    expect(() =>
      defineRoutes({
        routes: [
          {
            route: "/app",
            mode: "spa",
            entrypoint: "src/shell.ts",
            client_entrypoint: "src/app.client.ts",
            placement: "edge",
            cache_policy: { ttl: 0 },
          },
        ],
      }),
    ).toThrow(/placement is only valid on mode:"ssr"/);
  });

  test("error message names the offending route", () => {
    expect(() =>
      defineRoutes({
        routes: [
          {
            route: "/oops",
            mode: "static",
            entrypoint: "src/oops.ts",
            placement: "auto",
            cache_policy: { ttl: 0 },
          },
        ],
      }),
    ).toThrow(/\/oops/);
  });

  test("accepts prerender.from_data when the file is declared in data_inputs", () => {
    const cfg = defineRoutes({
      routes: [
        {
          route: "/items/:id",
          mode: "static",
          entrypoint: "src/items.ts",
          cache_policy: { ttl: 60 },
          data_inputs: ["data/items.json"],
          prerender: { from_data: "data/items.json", items_key: "items", param: "id" },
        },
      ],
    });
    expect(cfg.routes[0].prerender).toEqual({
      from_data: "data/items.json",
      items_key: "items",
      param: "id",
    });
  });

  test("rejects prerender.from_data referencing a path not in data_inputs", () => {
    expect(() =>
      defineRoutes({
        routes: [
          {
            route: "/items/:id",
            mode: "static",
            entrypoint: "src/items.ts",
            cache_policy: { ttl: 60 },
            data_inputs: ["data/other.json"],
            prerender: { from_data: "data/items.json", items_key: "items", param: "id" },
          },
        ],
      }),
    ).toThrow(/prerender\.from_data.*data_inputs/);
  });

  test("rejects prerender.from_data when data_inputs is omitted entirely", () => {
    expect(() =>
      defineRoutes({
        routes: [
          {
            route: "/items/:id",
            mode: "static",
            entrypoint: "src/items.ts",
            cache_policy: { ttl: 60 },
            prerender: { from_data: "data/items.json", items_key: "items", param: "id" },
          },
        ],
      }),
    ).toThrow(/\/items\/:id/);
  });

  test("accepts mixed-mode routes with placement only on ssr", () => {
    const cfg = defineRoutes({
      routes: [
        {
          route: "/",
          mode: "static",
          entrypoint: "src/render.ts",
          cache_policy: { ttl: 3600 },
        },
        {
          route: "/api/data",
          mode: "ssr",
          entrypoint: "src/data.ts",
          placement: "host",
          cache_policy: { ttl: 0 },
        },
        {
          route: "/app",
          mode: "spa",
          entrypoint: "src/shell.ts",
          client_entrypoint: "src/app.client.ts",
          cache_policy: { ttl: 0 },
        },
      ],
    });
    expect(cfg.routes).toHaveLength(3);
  });
});
