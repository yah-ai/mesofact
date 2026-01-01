// sqlite adapter — exercises get/query against an in-memory bun:sqlite DB and
// against an injected runner (for tag/timeout assertions). Verifies row + table
// tag emission, noTrack suppression, the 100ms timeout race, query error
// wrapping, and the registry factory.

import { afterEach, describe, expect, test } from "bun:test";
import { Database } from "bun:sqlite";
import {
  SqliteAdapter,
  type SqliteRunner,
  clearSqliteRegistry,
  registerSqlite,
  sqlite,
  runInTrackCtx,
  SourceQueryError,
  SourceTimeoutError,
} from "../src/index.js";

afterEach(() => clearSqliteRegistry());

// A real in-memory DB wrapped in the SqliteRunner seam, so we exercise the
// actual query/all path without a temp file on disk.
function memRunner(seed: (db: Database) => void): SqliteRunner {
  const db = new Database(":memory:");
  seed(db);
  return {
    all: (sql, params) => db.query(sql).all(...(params as never[])),
    close: () => db.close(),
  };
}

function seeded(): SqliteRunner {
  return memRunner((db) => {
    db.run("CREATE TABLE projects (id TEXT PRIMARY KEY, name TEXT)");
    db.run("INSERT INTO projects (id, name) VALUES ('p1', 'alpha'), ('p2', 'beta')");
  });
}

describe("SqliteAdapter.get", () => {
  test("returns the row by id and emits a row-level tag", async () => {
    const adapter = new SqliteAdapter({ name: "project_db", path: ":seam:", runner: seeded() });
    const { value, ctx } = await runInTrackCtx(async () =>
      adapter.get<{ id: string; name: string }>("projects", "p1"),
    );
    expect(value).toEqual({ id: "p1", name: "alpha" });
    expect([...ctx.tags]).toEqual(["sqlite:project_db:projects:p1"]);
  });

  test("missing row returns null (no throw)", async () => {
    const adapter = new SqliteAdapter({ name: "project_db", path: ":seam:", runner: seeded() });
    const { value } = await runInTrackCtx(async () => adapter.get("projects", "nope"));
    expect(value).toBeNull();
  });
});

describe("SqliteAdapter.query", () => {
  test("returns rows and emits one table-wide tag per FROM/JOIN table", async () => {
    const adapter = new SqliteAdapter({ name: "project_db", path: ":seam:", runner: seeded() });
    const { value, ctx } = await runInTrackCtx(async () =>
      adapter.query<{ name: string }>("SELECT name FROM projects ORDER BY id"),
    );
    expect(value).toEqual([{ name: "alpha" }, { name: "beta" }]);
    expect([...ctx.tags]).toEqual(["sqlite:project_db:projects"]);
  });

  test("binds positional params (no interpolation)", async () => {
    const adapter = new SqliteAdapter({ name: "project_db", path: ":seam:", runner: seeded() });
    const { value } = await runInTrackCtx(async () =>
      adapter.query<{ id: string }>("SELECT id FROM projects WHERE name = ?", ["beta"]),
    );
    expect(value).toEqual([{ id: "p2" }]);
  });

  test("query with no extractable table falls back to a source-wide tag", async () => {
    const adapter = new SqliteAdapter({
      name: "project_db",
      path: ":seam:",
      runner: { all: () => [{ n: 1 }] },
    });
    const { ctx } = await runInTrackCtx(async () => adapter.query("SELECT 1 AS n"));
    expect([...ctx.tags]).toEqual(["sqlite:project_db"]);
  });
});

describe("overrides", () => {
  test(".noTrack() suppresses the next call's tag only", async () => {
    const adapter = new SqliteAdapter({ name: "project_db", path: ":seam:", runner: seeded() });
    const { ctx } = await runInTrackCtx(async () => {
      await adapter.noTrack().get("projects", "p1");
      await adapter.get("projects", "p2");
    });
    expect([...ctx.tags]).toEqual(["sqlite:project_db:projects:p2"]);
  });

  test(".timeout(ms) fires before a stalled read resolves", async () => {
    // A runner whose all() never returns — stands in for a wedged DB handle.
    const stalled: SqliteRunner = {
      all: () => {
        // Synchronous return type, but the adapter awaits it; returning a
        // never-resolving thenable wedges the read so the timer wins.
        return new Promise(() => {}) as unknown as unknown[];
      },
    };
    const adapter = new SqliteAdapter({ name: "project_db", path: ":seam:", runner: stalled });
    const start = Date.now();
    await expect(
      runInTrackCtx(async () => adapter.timeout(50).query("SELECT * FROM projects")),
    ).rejects.toBeInstanceOf(SourceTimeoutError);
    expect(Date.now() - start).toBeLessThan(500);
  });

  test("query error is wrapped as SourceQueryError carrying the source name", async () => {
    const adapter = new SqliteAdapter({ name: "project_db", path: ":seam:", runner: seeded() });
    await expect(
      runInTrackCtx(async () => adapter.query("SELECT * FROM does_not_exist")),
    ).rejects.toMatchObject({ name: "SourceQueryError", source: "project_db" });
  });
});

describe("real on-disk file + registry factory", () => {
  test("opens a real file read-only via path and threads through sqlite(name)", async () => {
    const path = `/tmp/mesofact-sqlite-test-${Date.now()}.db`;
    const db = new Database(path);
    db.run("CREATE TABLE config (id TEXT PRIMARY KEY, v TEXT)");
    db.run("INSERT INTO config (id, v) VALUES ('k', 'value')");
    db.close();

    registerSqlite(new SqliteAdapter({ name: "cfg", path }));
    const { value, ctx } = await runInTrackCtx(async () =>
      sqlite("cfg").get<{ v: string }>("config", "k"),
    );
    expect(value).toEqual({ id: "k", v: "value" });
    expect([...ctx.tags]).toEqual(["sqlite:cfg:config:k"]);
  });

  test("sqlite(unknown) throws a name-bearing error", () => {
    expect(() => sqlite("nope")).toThrow(/sqlite source not registered: nope/);
  });
});
