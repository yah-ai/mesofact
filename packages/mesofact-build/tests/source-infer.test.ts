import { describe, expect, test } from "bun:test";
import { inferFromSource } from "../src/source-infer.js";

describe("inferFromSource", () => {
  test("extracts adapter names by factory call", () => {
    const src = `
      import { r2 } from "@mesofact/runtime";
      export async function render() {
        const a = await r2('assets').fetch('home.json');
        const b = await r2("more").fetch('x.json');
        return { html: a + b, cache: { ttl: 60 } };
      }
    `;
    expect(inferFromSource(src).source_reads).toEqual(["assets", "more"]);
  });

  test("dedupes repeated names and sorts", () => {
    const src = `r2('z'); r2('a'); r2('z'); r2('m')`;
    expect(inferFromSource(src).source_reads).toEqual(["a", "m", "z"]);
  });

  test("does not match suffixed identifiers", () => {
    const src = `myR2('shouldNotMatch')`;
    expect(inferFromSource(src).source_reads).toEqual([]);
  });

  test("// @mesofact-sources override wins over inference", () => {
    const src = `
      // @mesofact-sources project_db, assets
      r2('inferred_but_overridden').fetch('x');
    `;
    const r = inferFromSource(src);
    expect(r.override).toBe(true);
    expect(r.source_reads).toEqual(["assets", "project_db"]);
  });

  test("recognizes sqlite / pg / rpc factory calls", () => {
    const src = `
      sqlite('project_db').get('config', '1');
      pg('profile_pg').query('select 1');
      rpc('camp_host').get('whatever', 'x');
    `;
    expect(inferFromSource(src).source_reads).toEqual([
      "camp_host",
      "profile_pg",
      "project_db",
    ]);
  });
});
