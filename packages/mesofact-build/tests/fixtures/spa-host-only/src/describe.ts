// Transitive offender: a "helper" pulling node:fs that a real client would
// never need but a careless import might bring in (e.g. shared code that
// works server-side but slips into a client bundle).
import { existsSync } from "node:fs";

export function describe(): string {
  return existsSync("/") ? "fs present" : "fs absent";
}
