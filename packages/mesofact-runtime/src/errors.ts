// Typed errors adapters throw. Render functions can catch and return
// fallback HTML, or rethrow to let mesofact decide (stale-on-error / 503).
// See `.yah/docs/architecture/mesofact.md` §"Adapter API surface".

export class SourceError extends Error {
  readonly source: string;
  readonly retryable: boolean;

  constructor(
    message: string,
    source: string,
    retryable: boolean,
    options?: { cause?: unknown },
  ) {
    super(message, options as ErrorOptions | undefined);
    this.name = new.target.name;
    this.source = source;
    this.retryable = retryable;
  }
}

export class SourceUnavailableError extends SourceError {
  constructor(source: string, options?: { cause?: unknown }) {
    super(`source unavailable: ${source}`, source, true, options);
  }
}

export class SourceTimeoutError extends SourceError {
  readonly timeout_ms: number;

  constructor(source: string, timeout_ms: number, options?: { cause?: unknown }) {
    super(`source timeout after ${timeout_ms}ms: ${source}`, source, true, options);
    this.timeout_ms = timeout_ms;
  }
}

export class SourceQueryError extends SourceError {
  constructor(source: string, message: string, options?: { cause?: unknown }) {
    super(`source query error (${source}): ${message}`, source, false, options);
  }
}

export class RowNotFoundError extends SourceError {
  readonly table: string;
  readonly id: string;

  constructor(source: string, table: string, id: string) {
    super(`row not found: ${source}.${table}[${id}]`, source, false);
    this.table = table;
    this.id = id;
  }
}
