// `sqlite` adapter — read-only KeyValueSource over a local SQLite file (or a
// Litestream / LiteFS replica). Emits `sqlite:<source>:<table>:<id>` (row) and
// `sqlite:<source>:<table>` (table-wide) tags into the ambient trackCtx, honors
// per-call `.noTrack()` / `.timeout(ms)`, 100 ms default timeout.
//
// See `.yah/docs/architecture/mesofact.md` §"Adapter API surface", §"Adapter
// read-set provenance" (tag taxonomy), and §"Cache-key composition" (the
// proxy folds the file mtime into the cache key — generation is computed
// proxy-side from the source path, not here).

import { BaseSource, type KeyValueSource } from "../source.js";
import { SourceQueryError, SourceTimeoutError, SourceUnavailableError } from "../errors.js";

const DEFAULT_TIMEOUT_MS = 100;

// Minimal slice of `bun:sqlite` we depend on. Declared locally so the published
// contract package keeps `types: ["node"]` (no Bun globals leak into the types
// an outside TS consumer sees). The real `Database` is reached via a runtime
// dynamic import below; this is only its read surface.
export interface SqliteRunner {
  // Run `sql` with positional `params` and return every row as a plain object.
  all(sql: string, params: unknown[]): unknown[];
  close?(): void;
}

export type SqliteConfig = {
  // Logical source name from `mesofact.config.toml` (e.g. "project_db"). Used as
  // the registry key, the `sqlite:<name>:...` tag prefix, and surfaced in errors.
  name: string;
  // Filesystem path to the SQLite database. Opened read-only and lazily on the
  // first read so registration never touches disk.
  path: string;
  // Test seam — inject a runner to observe queries without a real DB file.
  // Defaults to opening `path` via `bun:sqlite`.
  runner?: SqliteRunner;
};

export class SqliteAdapter extends BaseSource implements KeyValueSource {
  readonly path: string;
  private runner: SqliteRunner | null;
  private opening: Promise<SqliteRunner> | null = null;

  constructor(config: SqliteConfig) {
    super(config.name);
    this.path = config.path;
    this.runner = config.runner ?? null;
  }

  async get<T>(table: string, id: string): Promise<T | null> {
    const { track, timeout_ms } = this.consumeOverrides(DEFAULT_TIMEOUT_MS);
    this.emitTag(`sqlite:${this.name}:${table}:${id}`, track);
    const rows = await this.run(
      `SELECT * FROM ${quoteIdent(table)} WHERE id = ? LIMIT 1`,
      [id],
      timeout_ms,
    );
    return (rows[0] as T | undefined) ?? null;
  }

  async query<T>(sql: string, params: unknown[] = []): Promise<T[]> {
    const { track, timeout_ms } = this.consumeOverrides(DEFAULT_TIMEOUT_MS);
    // Tag every table the query reads. When no table can be extracted (CTEs,
    // exotic SQL), fall back to a source-wide tag so invalidation stays
    // conservative — over-purging is recoverable; a missed tag is stale-forever.
    const tables = extractTables(sql);
    if (tables.length === 0) {
      this.emitTag(`sqlite:${this.name}`, track);
    } else {
      for (const table of tables) this.emitTag(`sqlite:${this.name}:${table}`, track);
    }
    return (await this.run(sql, params, timeout_ms)) as T[];
  }

  private async run(sql: string, params: unknown[], timeout_ms: number): Promise<unknown[]> {
    const exec = (async () => {
      const runner = await this.ensureRunner();
      try {
        return runner.all(sql, params);
      } catch (err) {
        throw new SourceQueryError(this.name, sqliteMessage(sql, err), { cause: err });
      }
    })();
    return this.race(exec, timeout_ms);
  }

  private async ensureRunner(): Promise<SqliteRunner> {
    if (this.runner) return this.runner;
    if (!this.opening) {
      this.opening = openBunSqlite(this.path)
        .then((r) => (this.runner = r))
        .catch((err) => {
          this.opening = null;
          throw new SourceUnavailableError(this.name, { cause: err });
        });
    }
    return this.opening;
  }

  private async race<T>(call: Promise<T>, timeout_ms: number): Promise<T> {
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      return await Promise.race<T>([
        call,
        new Promise<T>((_, reject) => {
          timer = setTimeout(
            () => reject(new SourceTimeoutError(this.name, timeout_ms)),
            timeout_ms,
          );
        }),
      ]);
    } finally {
      if (timer) clearTimeout(timer);
    }
  }
}

function sqliteMessage(sql: string, err: unknown): string {
  const head = sql.length > 80 ? `${sql.slice(0, 77)}...` : sql;
  return `sqlite query failed (${head}): ${err instanceof Error ? err.message : String(err)}`;
}

// Quote a table identifier so a table named like a keyword (or containing a
// dot) can't break the generated SQL. Table names come from server-side render
// code, not request input, so this is defensive rather than an injection guard;
// `id` and `query` params are always bound, never interpolated.
function quoteIdent(name: string): string {
  return `"${name.replace(/"/g, '""')}"`;
}

// Pull table names out of a SELECT for table-wide tag emission. A regex over
// `FROM`/`JOIN` clauses — same pragmatic posture as the build's regex source
// inference and the r2 list-XML parser. Misses are absorbed by the source-wide
// fallback in `query`.
function extractTables(sql: string): string[] {
  const out = new Set<string>();
  for (const m of sql.matchAll(/\b(?:from|join)\s+["'`]?([A-Za-z_][\w$]*)/gi)) {
    out.add(m[1]!);
  }
  return [...out];
}

// Reach `bun:sqlite` at runtime without making it a static import — the
// published runtime types stay Bun-free, and a non-Bun consumer that never
// calls a sqlite source never resolves the module. The `as string` defeats
// tsc's static module resolution (dynamic specifier → Promise<any>); the cast
// to the constructor shape restores type-safety on the result.
type BunStatement = { all(...params: unknown[]): unknown[] };
type BunDatabase = { query(sql: string): BunStatement; close(): void };

async function openBunSqlite(path: string): Promise<SqliteRunner> {
  const mod = (await import("bun:sqlite" as string)) as {
    Database: new (filename: string, options?: { readonly?: boolean }) => BunDatabase;
  };
  const db = new mod.Database(path, { readonly: true });
  return {
    all: (sql, params) => db.query(sql).all(...params),
    close: () => db.close(),
  };
}

// Per-process registry, mirroring the r2 adapter. `registerSourcesFromConfig`
// populates it from `[sources.*]` of `kind = "sqlite"`; render code looks up by
// name via `sqlite(name)`.
const registry = new Map<string, SqliteAdapter>();

export function registerSqlite(adapter: SqliteAdapter): void {
  registry.set(adapter.name, adapter);
}

export function clearSqliteRegistry(): void {
  registry.clear();
}

export function sqlite(name: string): KeyValueSource {
  const adapter = registry.get(name);
  if (!adapter) {
    throw new Error(
      `sqlite source not registered: ${name} (declare it in mesofact.config.toml under [sources.${name}])`,
    );
  }
  return adapter;
}
