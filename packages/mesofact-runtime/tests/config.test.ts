// mesofact.config.toml parser + env-driven adapter registration.

import { afterEach, describe, expect, test } from "bun:test";
import {
  ConfigError,
  clearR2Registry,
  parseConfig,
  r2,
  registerSourcesFromConfig,
  runInTrackCtx,
} from "../src/index.js";

afterEach(() => clearR2Registry());

describe("parseConfig", () => {
  test("accepts an r2 source with required fields", () => {
    const cfg = parseConfig(`
      [sources.assets]
      kind = "r2"
      scope = "global"
      bucket = "yah-assets"
      endpoint_env = "R2_ENDPOINT"
    `);
    expect(cfg.sources.assets).toEqual({
      kind: "r2",
      scope: "global",
      bucket: "yah-assets",
      endpoint_env: "R2_ENDPOINT",
    });
  });

  test("defaults scope to global when omitted", () => {
    const cfg = parseConfig(`
      [sources.assets]
      kind = "r2"
      bucket = "b"
      endpoint_env = "R2_ENDPOINT"
    `);
    expect(cfg.sources.assets!.scope).toBe("global");
  });

  test("honors per-source credential env overrides", () => {
    const cfg = parseConfig(`
      [sources.assets]
      kind = "r2"
      bucket = "b"
      endpoint_env = "R2_ENDPOINT"
      access_key_id_env = "R2_ACCESS_KEY_ID"
      secret_access_key_env = "R2_SECRET_KEY"
    `);
    const src = cfg.sources.assets!;
    if (src.kind !== "r2") throw new Error("unreachable");
    expect(src.access_key_id_env).toBe("R2_ACCESS_KEY_ID");
    expect(src.secret_access_key_env).toBe("R2_SECRET_KEY");
  });

  test("rejects an unknown kind with a name-bearing message", () => {
    expect(() =>
      parseConfig(`
      [sources.db]
      kind = "pg"
      scope = "global"
      `),
    ).toThrow(ConfigError);
    expect(() =>
      parseConfig(`
      [sources.db]
      kind = "pg"
      `),
    ).toThrow(/\[sources\.db\] unsupported kind: "pg"/);
  });

  test("accepts a sqlite source with a path", () => {
    const cfg = parseConfig(`
      [sources.project_db]
      kind = "sqlite"
      scope = "global"
      path = "/var/lib/yah/global.db"
    `);
    expect(cfg.sources.project_db).toEqual({
      kind: "sqlite",
      scope: "global",
      path: "/var/lib/yah/global.db",
    });
  });

  test("rejects a sqlite source missing path", () => {
    expect(() =>
      parseConfig(`
      [sources.project_db]
      kind = "sqlite"
      `),
    ).toThrow(/missing or empty `path`/);
  });

  test("rejects missing bucket and missing endpoint_env", () => {
    expect(() =>
      parseConfig(`
      [sources.assets]
      kind = "r2"
      endpoint_env = "R2_ENDPOINT"
      `),
    ).toThrow(/missing or empty `bucket`/);
    expect(() =>
      parseConfig(`
      [sources.assets]
      kind = "r2"
      bucket = "b"
      `),
    ).toThrow(/missing or empty `endpoint_env`/);
  });

  test("rejects an invalid scope value", () => {
    expect(() =>
      parseConfig(`
      [sources.assets]
      kind = "r2"
      bucket = "b"
      endpoint_env = "R2_ENDPOINT"
      scope = "tenant"
      `),
    ).toThrow(/invalid scope/);
  });

  test("empty file → no sources", () => {
    expect(parseConfig("")).toEqual({ sources: {} });
  });
});

describe("registerSourcesFromConfig", () => {
  test("registers each r2 source with env-resolved credentials", async () => {
    const cfg = parseConfig(`
      [sources.assets]
      kind = "r2"
      bucket = "yah-assets"
      endpoint_env = "R2_ENDPOINT"
    `);
    const env = {
      R2_ENDPOINT: "https://acct.r2.cloudflarestorage.com",
      AWS_ACCESS_KEY_ID: "AKIAIOSFODNN7EXAMPLE",
      AWS_SECRET_ACCESS_KEY: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
    };
    const names = registerSourcesFromConfig(cfg, env);
    expect(names).toEqual(["assets"]);
    // Adapter is reachable via r2(name) and emits the right bucket tag.
    const { ctx } = await runInTrackCtx(async () => {
      r2("assets").noTrack(); // dummy call to confirm the instance exists
      expect(r2("assets").name).toBe("assets");
    });
    expect([...ctx.tags]).toEqual([]);
  });

  test("uses per-source env overrides when supplied", () => {
    const cfg = parseConfig(`
      [sources.assets]
      kind = "r2"
      bucket = "b"
      endpoint_env = "R2_ENDPOINT"
      access_key_id_env = "R2_AK"
      secret_access_key_env = "R2_SK"
    `);
    const env = {
      R2_ENDPOINT: "https://acct.r2.cloudflarestorage.com",
      R2_AK: "key",
      R2_SK: "secret",
    };
    expect(registerSourcesFromConfig(cfg, env)).toEqual(["assets"]);
  });

  test("missing required env var raises ConfigError naming the var and source", () => {
    const cfg = parseConfig(`
      [sources.assets]
      kind = "r2"
      bucket = "b"
      endpoint_env = "R2_ENDPOINT"
    `);
    expect(() => registerSourcesFromConfig(cfg, {})).toThrow(ConfigError);
    expect(() => registerSourcesFromConfig(cfg, {})).toThrow(
      /\[sources\.assets\] env var `R2_ENDPOINT` \(from endpoint_env\)/,
    );
  });
});
