// `mesofact.config.toml` parser. Warden writes this file atomically and
// SIGHUPs the proxy + Bun pool on change; mesofact reads it on boot to
// instantiate adapters. Credentials come in via env vars (warden injects);
// the config carries only the env var *names*, never the secrets themselves.
//
// See `.yah/docs/architecture/mesofact.md` §"Warden owns config and credentials".

import { readFileSync } from "node:fs";
import { parse as parseToml } from "smol-toml";
import { R2Adapter, registerR2 } from "./adapters/r2.js";
import { SqliteAdapter, registerSqlite } from "./adapters/sqlite.js";
import type { SourceScope } from "./validate.js";

export type R2SourceConfig = {
  kind: "r2";
  scope: SourceScope;
  bucket: string;
  // Env var name resolved at register-time. R2's endpoint is account-scoped
  // (`https://<account_id>.r2.cloudflarestorage.com`), so warden injects it
  // rather than us guessing.
  endpoint_env: string;
  // Env var names for credentials. Defaults are the AWS-standard names so a
  // simple deployment can omit them.
  access_key_id_env?: string;
  secret_access_key_env?: string;
};

export type SqliteSourceConfig = {
  kind: "sqlite";
  scope: SourceScope;
  // Filesystem path to the database file. For `scope = "global"` it's a literal
  // path; the design templates `{project_id}` for scoped sources, deferred until
  // the first scoped SSR dogfood (no credentials — sqlite is a local file).
  path: string;
};

// Future kinds (pg, rpc) extend this union as their adapters land.
export type SourceConfig = R2SourceConfig | SqliteSourceConfig;

export type MesofactConfig = {
  sources: Record<string, SourceConfig>;
};

export function loadConfig(path: string): MesofactConfig {
  return parseConfig(readFileSync(path, "utf8"));
}

export function parseConfig(toml: string): MesofactConfig {
  const raw = parseToml(toml) as unknown;
  if (!isPlainObject(raw)) {
    throw new ConfigError("config must be a TOML object at top level");
  }
  const sourcesRaw = raw.sources;
  if (sourcesRaw === undefined) return { sources: {} };
  if (!isPlainObject(sourcesRaw)) {
    throw new ConfigError("[sources] must be a table");
  }
  const sources: Record<string, SourceConfig> = {};
  for (const [name, body] of Object.entries(sourcesRaw)) {
    sources[name] = parseSource(name, body);
  }
  return { sources };
}

function parseSource(name: string, body: unknown): SourceConfig {
  if (!isPlainObject(body)) {
    throw new ConfigError(`[sources.${name}] must be a table`);
  }
  const kind = body.kind;
  const scope = parseScope(name, body.scope);
  if (kind === "r2") return parseR2(name, body, scope);
  if (kind === "sqlite") return parseSqlite(name, body, scope);
  // P4 shipped r2; P9 adds sqlite. pg/rpc widen this union later.
  throw new ConfigError(
    `[sources.${name}] unsupported kind: ${JSON.stringify(kind)} (supported: "r2", "sqlite")`,
  );
}

function parseScope(name: string, raw: unknown): SourceScope {
  const scope = raw ?? "global";
  if (scope !== "global" && scope !== "project" && scope !== "user") {
    throw new ConfigError(
      `[sources.${name}] invalid scope: ${JSON.stringify(scope)} (expected "global" | "project" | "user")`,
    );
  }
  return scope;
}

function parseR2(name: string, body: Record<string, unknown>, scope: SourceScope): R2SourceConfig {
  const bucket = body.bucket;
  if (typeof bucket !== "string" || bucket === "") {
    throw new ConfigError(`[sources.${name}] missing or empty \`bucket\` (string)`);
  }
  const endpoint_env = body.endpoint_env;
  if (typeof endpoint_env !== "string" || endpoint_env === "") {
    throw new ConfigError(`[sources.${name}] missing or empty \`endpoint_env\` (string)`);
  }
  const access_key_id_env = optionalString(body.access_key_id_env, name, "access_key_id_env");
  const secret_access_key_env = optionalString(body.secret_access_key_env, name, "secret_access_key_env");
  return {
    kind: "r2",
    scope,
    bucket,
    endpoint_env,
    ...(access_key_id_env !== undefined ? { access_key_id_env } : {}),
    ...(secret_access_key_env !== undefined ? { secret_access_key_env } : {}),
  };
}

function parseSqlite(
  name: string,
  body: Record<string, unknown>,
  scope: SourceScope,
): SqliteSourceConfig {
  const path = body.path;
  if (typeof path !== "string" || path === "") {
    throw new ConfigError(`[sources.${name}] missing or empty \`path\` (string)`);
  }
  return { kind: "sqlite", scope, path };
}

function optionalString(value: unknown, source: string, field: string): string | undefined {
  if (value === undefined) return undefined;
  if (typeof value !== "string" || value === "") {
    throw new ConfigError(`[sources.${source}] \`${field}\` must be a non-empty string`);
  }
  return value;
}

// Resolve credentials from env and register an R2Adapter for each r2 source.
// Returns the resolved adapter names so callers can verify a known set landed.
export function registerSourcesFromConfig(
  config: MesofactConfig,
  env: Record<string, string | undefined> = process.env,
): string[] {
  const registered: string[] = [];
  for (const [name, src] of Object.entries(config.sources)) {
    if (src.kind === "r2") {
      const endpoint = requireEnv(env, src.endpoint_env, name, "endpoint_env");
      const accessKeyId = requireEnv(
        env,
        src.access_key_id_env ?? "AWS_ACCESS_KEY_ID",
        name,
        "access_key_id_env",
      );
      const secretAccessKey = requireEnv(
        env,
        src.secret_access_key_env ?? "AWS_SECRET_ACCESS_KEY",
        name,
        "secret_access_key_env",
      );
      registerR2(new R2Adapter({ name, bucket: src.bucket, endpoint, accessKeyId, secretAccessKey }));
      registered.push(name);
    } else if (src.kind === "sqlite") {
      // No credentials — sqlite is a local file. The DB opens lazily on first
      // read, so registration never touches disk.
      registerSqlite(new SqliteAdapter({ name, path: src.path }));
      registered.push(name);
    }
  }
  return registered;
}

function requireEnv(
  env: Record<string, string | undefined>,
  varName: string,
  source: string,
  field: string,
): string {
  const v = env[varName];
  if (v === undefined || v === "") {
    throw new ConfigError(
      `[sources.${source}] env var \`${varName}\` (from ${field}) is unset or empty`,
    );
  }
  return v;
}

export class ConfigError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ConfigError";
  }
}

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}
