// W173 § "SSR_PREFIXES derivation rule" — verify the table in the doc.

import { describe, expect, test } from "bun:test";
import type { RouteEntry } from "@mesofact/runtime";
import { deriveSsrPrefix, deriveSsrPrefixes } from "../src/ssr-prefix.js";

describe("deriveSsrPrefix", () => {
  test("non-parametric route → full route (exact-match prefix)", () => {
    expect(deriveSsrPrefix("/api/health")).toBe("/api/health");
  });

  test("trailing parametric segment → truncated at first `:foo`", () => {
    expect(deriveSsrPrefix("/api/users/:id")).toBe("/api/users/");
  });

  test("middle parametric segment → truncated (over-broad by design)", () => {
    expect(deriveSsrPrefix("/x/:a/y")).toBe("/x/");
  });

  test("wildcard segment → truncated", () => {
    expect(deriveSsrPrefix("/feed/*")).toBe("/feed/");
  });

  test("root parametric → truncated at root", () => {
    expect(deriveSsrPrefix("/:id")).toBe("/");
  });

  test("root → root", () => {
    expect(deriveSsrPrefix("/")).toBe("/");
  });
});

describe("deriveSsrPrefixes", () => {
  function r(route: string, mode: RouteEntry["mode"]): RouteEntry {
    return {
      route,
      mode,
      entrypoint: "src/x.ts",
      cache_policy: { ttl: 0 },
    };
  }

  test("filters to mode:'ssr' routes only", () => {
    expect(
      deriveSsrPrefixes([
        r("/", "static"),
        r("/api/health", "ssr"),
        r("/app", "spa"),
      ]),
    ).toEqual(["/api/health"]);
  });

  test("dedupes and sorts", () => {
    expect(
      deriveSsrPrefixes([
        r("/api/users/:id", "ssr"),
        r("/api/users/:slug", "ssr"), // same derived prefix
        r("/api/health", "ssr"),
      ]),
    ).toEqual(["/api/health", "/api/users/"]);
  });

  test("returns empty when no SSR routes", () => {
    expect(
      deriveSsrPrefixes([r("/", "static"), r("/app", "spa")]),
    ).toEqual([]);
  });
});
