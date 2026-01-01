// Internal API surface for embedders (currently only the worker binary itself
// and the test harness via direct bun:exec).

export {
  encode,
  NdjsonDecoder,
} from "./protocol.ts";
export type {
  ErrorCode,
  ErrorPayload,
  RenderMsg,
  OkMsg,
  ErrMsg,
  ReadyMsg,
  PingMsg,
  PongMsg,
  DrainMsg,
  ProxyToWorker,
  WorkerToProxy,
} from "./protocol.ts";

export {
  Pool,
  OverflowError,
  DrainingError,
  DEFAULT_CONCURRENCY,
  DEFAULT_QUEUE_DEPTH,
} from "./pool.ts";

export { runWorker } from "./worker.ts";
export type { WorkerOptions } from "./worker.ts";
