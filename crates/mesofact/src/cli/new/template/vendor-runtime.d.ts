// `@mesofact/runtime` — types only, vended by `mesofact new` at version
// __MESOFACT_VERSION__.
//
// WHY THIS IS VENDORED AND NOT AN npm DEPENDENCY. The barrel that actually
// executes is compiled into the mesofact binary (`runtime_shim.js` inside
// `mesofact-ssr`), and every bundler path that sees an `@mesofact/runtime`
// import keeps the specifier external — the SSG and SSR module loaders map it
// to that embedded shim at execution time. So nothing is ever fetched for it,
// and the only thing a project needs on disk is the declarations below, for
// your editor and `tsc`.
//
// That also makes the version pin unforgeable: this file was written by the
// mesofact binary that scaffolded the project, so the types can only describe
// the runtime that will actually run. `.mesofact-version` selects both.
//
// It is a SUBSET — the surface the scaffold itself uses, plus the shapes you
// need to write a route. The full barrel (source adapters, `r2`, `weaveHead`,
// `defineReadyz`, the manifest types) is real and callable at runtime; it is
// simply not declared here yet. Regenerate this file, or drop in the published
// `@mesofact/runtime` types, when you need more of it.

declare module "@mesofact/runtime" {
  export type Region = string;

  export type User = { id: string; attrs: Record<string, unknown> };

  export type Project = {
    id: string;
    home_region: Region;
    generation: string;
  };

  /** What a `mode: "static"` render is handed. Not a `Request`. */
  export type RenderRequest = {
    url: string;
    params: Record<string, string>;
    query: Record<string, string>;
    headers: Record<string, string>;
    cookies: Record<string, string>;
    user?: User;
    project?: Project;
    region?: Region;
    ctx?: Record<string, unknown>;
    /** Build-time data artifacts; populated only during prerender. */
    data?: Record<string, unknown>;
  };

  export type CachePolicy = { ttl: number; tags?: readonly string[] };

  /** `mode: "spa"` only — the state the shell serializes for the client. */
  export type Hydration = { script?: string; initial_state?: unknown };

  export type RenderResult = {
    html: string;
    headers?: Record<string, string>;
    cache: CachePolicy;
    hydration?: Hydration;
    head?: Record<string, unknown>;
  };

  export type RenderFn = (req: RenderRequest) => Promise<RenderResult>;

  export type RouteMode = "static" | "ssr" | "spa" | "ssg";

  export type RouteEntry = {
    route: string;
    mode: RouteMode;
    entrypoint: string;
    /** `mode: "spa"` only — the browser bundle that takes the shell over. */
    client_entrypoint?: string;
    cache_policy?: { ttl: number; swr?: number; tags?: readonly string[] };
    [key: string]: unknown;
  };

  export type RoutesConfig = {
    routes: readonly RouteEntry[];
    error_routes?: Record<string, string>;
    /** Mode-2 endpoint callbacks — hook name → entrypoint. */
    hooks?: Record<string, string>;
    /** Absolute origin (scheme + host) enabling sitemap emission. */
    site_url?: string;
    [key: string]: unknown;
  };

  export function defineRoutes(config: RoutesConfig): RoutesConfig;
}
