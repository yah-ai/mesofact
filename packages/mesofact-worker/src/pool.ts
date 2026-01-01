// Per-route concurrency cap + bounded queue.
// Default 4 concurrent renders per route, 64-item queue. Overflow returns
// `queue_overflow` (the proxy maps that to 503).
// See `.yah/docs/architecture/mesofact.md` §"Concurrency per worker".

export const DEFAULT_CONCURRENCY = 4;
export const DEFAULT_QUEUE_DEPTH = 64;

type Waiter = { resolve: () => void; reject: (e: unknown) => void };

export class OverflowError extends Error {
  constructor(public readonly route: string) {
    super(`queue overflow for route ${route}`);
    this.name = "OverflowError";
  }
}

export class DrainingError extends Error {
  constructor() {
    super("worker draining; not accepting new renders");
    this.name = "DrainingError";
  }
}

class RouteSemaphore {
  private inFlight = 0;
  private readonly waiters: Waiter[] = [];

  constructor(
    private readonly route: string,
    private readonly concurrency: number,
    private readonly queueDepth: number,
  ) {}

  async acquire(): Promise<void> {
    if (this.inFlight < this.concurrency) {
      this.inFlight++;
      return;
    }
    if (this.waiters.length >= this.queueDepth) {
      throw new OverflowError(this.route);
    }
    await new Promise<void>((resolve, reject) => {
      this.waiters.push({ resolve, reject });
    });
    this.inFlight++;
  }

  release(): void {
    this.inFlight--;
    const next = this.waiters.shift();
    if (next) next.resolve();
  }

  // For drain: reject everything queued; renders already past acquire() get
  // to complete on their own.
  rejectQueued(err: Error): void {
    while (this.waiters.length > 0) {
      this.waiters.shift()!.reject(err);
    }
  }

  get busy(): number {
    return this.inFlight;
  }
}

export class Pool {
  private readonly perRoute = new Map<string, RouteSemaphore>();
  private inFlightTotal = 0;
  private drainResolvers: Array<() => void> = [];
  private draining = false;

  configureRoute(route: string, concurrency?: number, queueDepth?: number): void {
    this.perRoute.set(
      route,
      new RouteSemaphore(
        route,
        concurrency ?? DEFAULT_CONCURRENCY,
        queueDepth ?? DEFAULT_QUEUE_DEPTH,
      ),
    );
  }

  async run<T>(route: string, fn: () => Promise<T>): Promise<T> {
    if (this.draining) throw new DrainingError();
    const sem = this.perRoute.get(route);
    if (!sem) {
      throw new Error(`route not configured in pool: ${route}`);
    }
    await sem.acquire();
    this.inFlightTotal++;
    try {
      return await fn();
    } finally {
      this.inFlightTotal--;
      sem.release();
      if (this.draining && this.inFlightTotal === 0) {
        const resolvers = this.drainResolvers;
        this.drainResolvers = [];
        for (const r of resolvers) r();
      }
    }
  }

  // Stop accepting new renders, reject anything queued, and resolve once all
  // in-flight renders finish. Callers `await` this before exiting.
  drain(): Promise<void> {
    this.draining = true;
    const err = new DrainingError();
    for (const sem of this.perRoute.values()) sem.rejectQueued(err);
    if (this.inFlightTotal === 0) return Promise.resolve();
    return new Promise((resolve) => {
      this.drainResolvers.push(resolve);
    });
  }

  get isDraining(): boolean {
    return this.draining;
  }

  get inFlight(): number {
    return this.inFlightTotal;
  }
}
