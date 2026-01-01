// Loads every `*.json` in `tests/fixtures/manifests/` (workspace-shared with
// the Rust validator) and asserts the TS validator's verdict matches the
// fixture's `expect` field.

import { describe, expect, test } from "bun:test";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { validate, type SourceCatalog, type ValidationError } from "../src/index.js";

const FIXTURES_DIR = fileURLToPath(
  new URL("../../../tests/fixtures/manifests/", import.meta.url),
);

type ExpectOk = "ok";
type ExpectErrors = { errors: Array<{ kind: string }> };
type Expect = ExpectOk | ExpectErrors;

type Fixture = {
  sources: SourceCatalog;
  manifest: unknown;
  expect: Expect;
};

function loadFixtures(): Array<{ name: string; fixture: Fixture }> {
  return readdirSync(FIXTURES_DIR)
    .filter((f) => f.endsWith(".json"))
    .sort()
    .map((name) => ({
      name,
      fixture: JSON.parse(readFileSync(join(FIXTURES_DIR, name), "utf8")) as Fixture,
    }));
}

describe("shared manifest fixtures", () => {
  const fixtures = loadFixtures();
  expect(fixtures.length).toBeGreaterThan(0);

  for (const { name, fixture } of fixtures) {
    test(name, () => {
      const result = validate(fixture.manifest, fixture.sources);

      if (fixture.expect === "ok") {
        if (!result.ok) {
          throw new Error(
            `expected ok, got errors: ${JSON.stringify(result.errors, null, 2)}`,
          );
        }
        return;
      }

      expect(result.ok).toBe(false);
      const errs = (result as { ok: false; errors: ValidationError[] }).errors;
      const got = new Set(errs.map((e) => e.kind));
      const want = new Set(fixture.expect.errors.map((e) => e.kind));
      expect([...got].sort()).toEqual([...want].sort());
    });
  }
});
