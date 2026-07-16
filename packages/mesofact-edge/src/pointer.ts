// @mesofact/edge — pointer-store resolution at the edge.
//
// Part of R595-F3 — annotation in
// .yah/docs/working/W270-yah-share-mesofact-gap-closure.md
//
// TS mirror of the wire format in
// `oss/mesofact/crates/mesofact-publisher/src/pointer.rs`. Kept byte-parallel
// with that Rust module (the same lockstep discipline as the hydration/head
// helper ports): a change to the record shape, key rules, or version MUST land
// in both. The publisher flips pointers; the edge resolves them.

/** Storage prefix for pointer records — mirrors `POINTER_PREFIX`. */
export const POINTER_PREFIX = "p/";

/** Record schema version this edge speaks — mirrors `POINTER_RECORD_V`. A
 *  record with a different `v` is refused (→ 5xx) rather than misread. */
export const POINTER_RECORD_V = 1;

/** What a live pointer names — mirrors `pointer.rs::Pointer`. `content_root` is
 *  the serving root the edge fetches bytes from; `source_root` optionally
 *  records the durable source bundle it was derived from. */
export type Pointer = {
  content_root: string;
  source_root?: string;
  published_at?: string;
};

/** On-the-wire record: a pointer or its tombstone — mirrors
 *  `pointer.rs::PointerRecord`. */
type PointerRecord = {
  v: number;
  pointer?: Pointer;
  deleted_at?: string;
};

/** Resolution outcome — mirrors `pointer.rs::PointerState`. `deleted` exists so
 *  serving can answer 410 (was published, now gone) distinctly from 404. */
export type PointerState =
  | { kind: "present"; pointer: Pointer }
  | { kind: "deleted" }
  | { kind: "absent" };

/** Thrown when a pointer record is malformed or speaks an unknown version —
 *  the caller answers 5xx rather than guessing. */
export class PointerMalformed extends Error {}

/**
 * Reject keys that could alias other storage areas — mirrors
 * `pointer.rs::validate_key`. Keys are minted by the domain (URL paths), but
 * the edge still refuses traversal / empty / whitespace shapes before any
 * fetch. Empty is rejected: the site root pointer (`key = ""`) is not unified
 * onto this store yet. Returns a reason string when invalid, else `null`.
 */
export function validateKey(key: string): string | null {
  if (key.length === 0) {
    return "empty key is reserved for the site root pointer";
  }
  if (key.startsWith("/") || key.endsWith("/")) {
    return "leading/trailing slash";
  }
  for (const seg of key.split("/")) {
    if (seg === "" || seg === "." || seg === "..") {
      return "empty or dot path segment";
    }
  }
  // `is_control() || is_whitespace()` in the Rust twin. `\s` covers whitespace;
  // `\x00-\x1f\x7f` covers the C0 controls + DEL. Hyphens/underscores common in
  // slugs are deliberately allowed.
  if (/[\s\x00-\x1f\x7f]/.test(key)) {
    return "control or whitespace character";
  }
  return null;
}

/**
 * Resolve `key` → pointer state by reading `${pointerOrigin}/p/<key>`.
 *
 * The pointer is the one mutable object in the store; freshness is governed by
 * the record's own `no-cache` header (set by the publisher at PUT time), so a
 * plain `fetch` here revalidates on every read while every content-addressed
 * byte stays cacheable forever. A `404` is `absent` (never existed); a
 * malformed body or unknown `v` throws [`PointerMalformed`]; a record with a
 * `pointer` is `present`, otherwise it is a tombstone (`deleted`).
 *
 * An un-mintable key can never have named a pointer, so it resolves `absent`
 * (→ 404) without a fetch.
 */
export async function resolvePointer(
  pointerOrigin: string,
  key: string,
): Promise<PointerState> {
  if (validateKey(key) !== null) {
    return { kind: "absent" };
  }
  const url = `${pointerOrigin}/${POINTER_PREFIX}${key}`;
  const resp = await fetch(url);
  if (resp.status === 404) {
    return { kind: "absent" };
  }
  if (!resp.ok) {
    throw new PointerMalformed(`pointer read ${url} -> ${resp.status}`);
  }
  let record: PointerRecord;
  try {
    record = (await resp.json()) as PointerRecord;
  } catch {
    throw new PointerMalformed(`pointer ${key}: malformed JSON`);
  }
  if (record.v !== POINTER_RECORD_V) {
    throw new PointerMalformed(
      `pointer ${key}: record version ${record.v} (edge speaks ${POINTER_RECORD_V})`,
    );
  }
  return record.pointer
    ? { kind: "present", pointer: record.pointer }
    : { kind: "deleted" };
}
