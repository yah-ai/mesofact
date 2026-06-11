// @mesofact/runtime — render contract types and adapter API.
// See `.yah/docs/architecture/mesofact.md`.

export type {
  Region,
  User,
  Project,
  RenderRequest,
  CachePolicy,
  Hydration,
  RenderResult,
  RenderFn,
} from "./contract.js";

export type {
  ListOpts,
  R2Object,
  Source,
  BlobSource,
  KeyValueSource,
} from "./source.js";
export { BaseSource } from "./source.js";

export {
  SourceError,
  SourceUnavailableError,
  SourceTimeoutError,
  SourceQueryError,
  RowNotFoundError,
} from "./errors.js";

export type {
  RouteMode,
  Placement,
  Requires,
  CachePolicyConfig,
  PrerenderConfig,
  RouteEntry,
  ErrorRoutes,
  RoutesConfig,
  RetryOn,
  RetryPolicy,
  QueuePolicy,
  ResiliencePolicy,
} from "./routes.js";

export { defineRoutes, DEFAULT_RESILIENCE_TIMEOUT_MS } from "./routes.js";

export type {
  ManifestVersion,
  ManifestCachePolicy,
  ManifestHydration,
  ManifestPrerender,
  ManifestRoute,
  ManifestStaticAsset,
  ManifestErrorRoutes,
  Manifest,
  ResolvedPlacement,
} from "./manifest.js";

export { MANIFEST_VERSION } from "./manifest.js";

export type {
  SourceScope,
  SourceCatalog,
  ValidationError,
  ValidationErrorKind,
  ValidationResult,
} from "./validate.js";

export { validate } from "./validate.js";

export { runInTrackCtx, currentTrackCtx } from "./track-ctx.js";
export type { TrackCtx } from "./track-ctx.js";

export {
  SPA_STATE_SCRIPT_ID,
  SSR_DATA_SCRIPT_ID,
  escapeJsonForScriptTag,
  hydrationDataTag,
  hydrationScriptTag,
} from "./hydration.js";

export { R2Adapter, r2, registerR2, clearR2Registry } from "./adapters/r2.js";
export type { R2Config } from "./adapters/r2.js";

export { SqliteAdapter, sqlite, registerSqlite, clearSqliteRegistry } from "./adapters/sqlite.js";
export type { SqliteConfig, SqliteRunner } from "./adapters/sqlite.js";

export {
  loadConfig,
  parseConfig,
  registerSourcesFromConfig,
  ConfigError,
} from "./config.js";
export type {
  BuildConfig,
  MesofactConfig,
  R2SourceConfig,
  SqliteSourceConfig,
  SourceConfig,
} from "./config.js";
