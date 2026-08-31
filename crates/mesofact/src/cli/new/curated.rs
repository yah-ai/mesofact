//! The curated JS dependency set — **the pin**, and the single place it is
//! written down.
//!
//! W225 §2 ("The curated JS set is a COMMITMENT, not advisory", operator
//! decision 2026-08-28): a mesofact version pins react, react-dom, the
//! `@mesofact/*` runtime barrel and the matching `@types/*` at exact
//! versions, and that pairing is tested per release. The reason is
//! `.mesofact-version`'s coherence — R759 makes it the one dial that selects
//! the `mesofact` / `mesofact-dev` pair, and a floating JS set would make
//! that dial honest for the binaries and a lie for the app they serve.
//!
//! **Both emitted artifacts are derived from [`CURATED`].** `package.json`
//! and `bun.lock` are generated from one table rather than kept as two
//! hand-maintained template files, because two files is exactly how a pin
//! stops being a pin: `package.json` says `18.3.1`, the lock says something
//! else, and nothing notices. Here a version can only be changed in one
//! place, and [`bun_lock`] cannot emit an entry [`package_json`] does not
//! know about.
//!
//! Nobody resolves on this path. The lock ships pre-resolved with sha512
//! integrity for every entry, and `mesofact-build`'s lockfile-driven
//! materializer (`install.rs`) installs it with no external package manager
//! and no Node on PATH. Reaching *outside* the set is the escape hatch —
//! R757's resolver — which is supported but carries no compatibility promise.
//!
//! ## Changing the set
//!
//! The set changes **only on a mesofact version bump**; that is what lets a
//! consumer treat one pin as sufficient. To change it: edit [`CURATED`], run
//! `scripts/check-mesofact-new.sh` (which scaffolds, materializes and serves
//! against the new set on a Node-free PATH), and bump the workspace version.
//! The integrity strings come from a real resolver — mint them with
//! `bun install` in a scratch project and copy the `packages` entries out of
//! the resulting `bun.lock`, never by hand.

/// One pinned package: everything both emitted artifacts need to name it
/// exactly, and everything the materializer needs to fetch it safely.
pub struct Pinned {
    /// Registry name, e.g. `@types/react`.
    pub name: &'static str,
    /// The exact version. Never a range — a range in a lockfile-driven
    /// install is a resolution request, and nothing here resolves.
    pub version: &'static str,
    /// Where this package appears in the scaffolded `package.json`.
    pub role: Role,
    /// The package's own dependency metadata, verbatim as bun writes it into
    /// a `bun.lock` entry's third slot. `{}` when it has none. The
    /// materializer ignores this field (it walks the lock's install-path
    /// keys, it does not resolve), but a real `bun.lock` carries it and a
    /// consumer who later runs `bun install` reads it.
    pub meta: &'static str,
    /// sha512 subresource integrity for the registry tarball. The
    /// materializer refuses by name any registry entry that lacks one, so
    /// this is load-bearing, not decoration.
    pub integrity: &'static str,
}

/// Where a pinned package lands in the scaffolded `package.json`.
#[derive(PartialEq, Eq, Clone, Copy)]
pub enum Role {
    /// Listed under `dependencies`.
    Runtime,
    /// Listed under `devDependencies`.
    Dev,
    /// In the closure and in the lock, but named by nothing in
    /// `package.json` — a transitive dependency of one of the above.
    Transitive,
}

/// The set, closed under dependency. Direct entries and their transitive
/// closure are one list because the lock needs every one of them and the
/// distinction is only about which section of `package.json` names it.
///
/// React 18.3.1 rather than 19 is deliberate: it is the pairing this repo's
/// examples and prerender path are actually exercised against
/// (`examples/hello`), and the commitment is only worth making about a
/// pairing that is tested. Moving to 19 is a version-bump decision with its
/// own release test, not a silent float.
pub const CURATED: &[Pinned] = &[
    Pinned {
        name: "react",
        version: "18.3.1",
        role: Role::Runtime,
        meta: r#"{ "dependencies": { "loose-envify": "^1.1.0" } }"#,
        integrity: "sha512-wS+hAgJShR0KhEvPJArfuPVN1+Hz1t0Y6n5jLrGQbkb4urgPE/0Rve+1kMB1v/oWgHgm4WIcV+i7F2pTVj+2iQ==",
    },
    Pinned {
        name: "react-dom",
        version: "18.3.1",
        role: Role::Runtime,
        meta: r#"{ "dependencies": { "loose-envify": "^1.1.0", "scheduler": "^0.23.2" }, "peerDependencies": { "react": "^18.3.1" } }"#,
        integrity: "sha512-5m4nQKp+rZRb09LNH59GM4BxTh9251/ylbKIbpe7TpGxfJ+9kv6BLkLBXIjjspbgbnIBNqlI23tRnTWT0snUIw==",
    },
    Pinned {
        name: "scheduler",
        version: "0.23.2",
        role: Role::Transitive,
        meta: r#"{ "dependencies": { "loose-envify": "^1.1.0" } }"#,
        integrity: "sha512-UOShsPwz7NrMUqhR6t0hWjFduvOzbtv7toDH1/hIrfRNIDBnnBWd0CwJTGvTpngVlmwGCdP9/Zl/tVrDqcuYzQ==",
    },
    Pinned {
        name: "loose-envify",
        version: "1.4.0",
        role: Role::Transitive,
        meta: r#"{ "dependencies": { "js-tokens": "^3.0.0 || ^4.0.0" }, "bin": { "loose-envify": "cli.js" } }"#,
        integrity: "sha512-lyuxPGr/Wfhrlem2CL/UcnUc1zcqKAImBDzukY7Y5F/yQiNdko6+fRLevlw1HgMySw7f611UIY408EtxRSoK3Q==",
    },
    Pinned {
        name: "js-tokens",
        version: "4.0.0",
        role: Role::Transitive,
        meta: "{}",
        integrity: "sha512-RdJUflcE3cUzKiMqQgsCu06FPu9UdIJO0beYbPhHN4k6apgJtifcoCtT9bcxOpYBtpD2kCM6Sbzg4CausW/PKQ==",
    },
    Pinned {
        name: "typescript",
        version: "5.9.3",
        role: Role::Dev,
        meta: r#"{ "bin": { "tsc": "bin/tsc", "tsserver": "bin/tsserver" } }"#,
        integrity: "sha512-jl1vZzPDinLr9eUt3J/t7V6FgNEw9QjvBPdysz9KfQDD41fQrC2Y4vKQdiaUpFT4bXlb1RHhLpp8wtm6M5TgSw==",
    },
    Pinned {
        name: "@types/react",
        version: "18.3.31",
        role: Role::Dev,
        meta: r#"{ "dependencies": { "@types/prop-types": "*", "csstype": "^3.2.2" } }"#,
        integrity: "sha512-vfEqpXTvwT91yhmwdfouStN2hSKwTvyRs8qpLfADyrq/kxDw0hZM7Wk9Ug1FELj8hIby+S/+kQCSRFF32nv2Qw==",
    },
    Pinned {
        name: "@types/react-dom",
        version: "18.3.7",
        role: Role::Dev,
        meta: r#"{ "peerDependencies": { "@types/react": "^18.0.0" } }"#,
        integrity: "sha512-MEe3UeoENYVFXzoXEWsvcpg6ZvlrFNlOQ7EOsvhI3CfAXwzPfO8Qwuxd40nepsYKqyyVQnTdEfv68q91yLcKrQ==",
    },
    Pinned {
        name: "@types/prop-types",
        version: "15.7.15",
        role: Role::Transitive,
        meta: "{}",
        integrity: "sha512-F6bEyamV9jKGAFBEmlQnesRPGOQqS2+Uwi0Em15xenOxHaf2hv6L8YCVn3rPdPJOiJfPiCnLIRyvwVaqMY3MIw==",
    },
    Pinned {
        name: "csstype",
        version: "3.2.3",
        role: Role::Transitive,
        meta: "{}",
        integrity: "sha512-z1HGKcYy2xA8AGQfwrn0PAy+PB7X/GSj3UVJW9qKyn43xWa+gl5nXmU4qqLMRzWVLFC8KusUX8T/0kCiOYpAIQ==",
    },
];

/// The runtime barrel's package name. It is **not** in [`CURATED`] because it
/// is not acquired from npm: the barrel that actually executes is compiled
/// into the mesofact binary (`mesofact-ssr/js/runtime_shim.js`), and every
/// bundler path that sees an `@mesofact/runtime` import keeps it external
/// (`mesofact-build/src/bundle.rs`). What a scaffolded project needs on disk
/// is only the *types*, which is why the scaffold vends them as a `file:`
/// dependency pinned to this binary's own version — the strongest possible
/// form of "pinned per mesofact version", since it cannot be anything else.
pub const RUNTIME_BARREL: &str = "@mesofact/runtime";

/// Where the scaffold vends [`RUNTIME_BARREL`], relative to the project root.
pub const RUNTIME_BARREL_DIR: &str = "vendor/mesofact-runtime";

/// The mesofact version this binary pins the set to. Written into
/// `.mesofact-version` and into the vendored barrel's `package.json`, so a
/// scaffolded project's JS set and its binaries name the same version.
pub const MESOFACT_VERSION: &str = env!("CARGO_PKG_VERSION");

fn of_role(role: Role) -> impl Iterator<Item = &'static Pinned> {
    CURATED.iter().filter(move |p| p.role == role)
}

/// Render the scaffolded project's `package.json`.
///
/// Direct deps are written at their **exact** version with no range operator.
/// A `^` here would mean "some npm resolver may pick something else", which
/// is precisely the float the commitment rules out — and since nothing on
/// this path resolves, a range would also be a claim no code in the pipeline
/// could honour.
pub fn package_json(project: &str) -> String {
    let mut s = String::new();
    s.push_str("{\n");
    s.push_str(&format!("  \"name\": {},\n", json_str(project)));
    s.push_str("  \"version\": \"0.0.0\",\n");
    s.push_str("  \"private\": true,\n");
    s.push_str("  \"type\": \"module\",\n");
    s.push_str("  \"scripts\": {\n");
    s.push_str("    \"typecheck\": \"tsc --noEmit\"\n");
    s.push_str("  },\n");
    s.push_str("  \"dependencies\": {\n");
    let mut deps: Vec<String> = vec![format!(
        "    {}: \"file:{RUNTIME_BARREL_DIR}\"",
        json_str(RUNTIME_BARREL)
    )];
    deps.extend(of_role(Role::Runtime).map(|p| format!("    {}: {}", json_str(p.name), json_str(p.version))));
    s.push_str(&deps.join(",\n"));
    s.push_str("\n  },\n");
    s.push_str("  \"devDependencies\": {\n");
    let dev: Vec<String> = of_role(Role::Dev)
        .map(|p| format!("    {}: {}", json_str(p.name), json_str(p.version)))
        .collect();
    s.push_str(&dev.join(",\n"));
    s.push_str("\n  }\n");
    s.push_str("}\n");
    s
}

/// Render the shipped `bun.lock` — the artifact that makes this a commitment
/// rather than a suggestion.
///
/// `mesofact-build`'s materializer reads two things out of each entry: the
/// locator (`name@version`, or `name@file:path` for a link) and the sha512
/// integrity. Everything else here exists so the file is a *real* bun.lock
/// rather than a shape that happens to satisfy one reader — a consumer who
/// runs `bun install` gets the same closure back.
pub fn bun_lock(project: &str) -> String {
    let mut s = String::new();
    s.push_str("{\n");
    s.push_str("  \"lockfileVersion\": 1,\n");
    s.push_str("  \"configVersion\": 1,\n");
    s.push_str("  \"workspaces\": {\n");
    s.push_str("    \"\": {\n");
    s.push_str(&format!("      \"name\": {},\n", json_str(project)));
    s.push_str("      \"dependencies\": {\n");
    let mut deps: Vec<String> = vec![format!(
        "        {}: \"file:{RUNTIME_BARREL_DIR}\"",
        json_str(RUNTIME_BARREL)
    )];
    deps.extend(
        of_role(Role::Runtime).map(|p| format!("        {}: {}", json_str(p.name), json_str(p.version))),
    );
    s.push_str(&deps.join(",\n"));
    s.push_str("\n      },\n");
    s.push_str("      \"devDependencies\": {\n");
    let dev: Vec<String> = of_role(Role::Dev)
        .map(|p| format!("        {}: {}", json_str(p.name), json_str(p.version)))
        .collect();
    s.push_str(&dev.join(",\n"));
    s.push_str("\n      },\n");
    s.push_str("    },\n");
    s.push_str("  },\n");
    s.push_str("  \"packages\": {\n");
    // The vended barrel: a link, so no registry and no integrity — nothing is
    // fetched for it. `install.rs` symlinks `file:` locators.
    s.push_str(&format!(
        "    {}: [\"{RUNTIME_BARREL}@file:{RUNTIME_BARREL_DIR}\", {{}}],\n\n",
        json_str(RUNTIME_BARREL)
    ));
    // Sorted so the emitted lock has a stable, reviewable order regardless of
    // where a package sits in CURATED.
    let mut entries: Vec<&Pinned> = CURATED.iter().collect();
    entries.sort_by_key(|p| p.name);
    for p in entries {
        s.push_str(&format!(
            "    {}: [\"{}@{}\", \"\", {}, {}],\n\n",
            json_str(p.name),
            p.name,
            p.version,
            p.meta,
            json_str(p.integrity),
        ));
    }
    // Trailing blank line from the last entry is the shape bun emits; the
    // trailing comma is JSONC, which install.rs strips.
    s.push_str("  }\n");
    s.push_str("}\n");
    s
}

/// The vendored runtime barrel's own `package.json` — types-only, versioned
/// with the binary that wrote it.
pub fn barrel_package_json() -> String {
    format!(
        r#"{{
  "name": "{RUNTIME_BARREL}",
  "version": "{MESOFACT_VERSION}",
  "private": true,
  "type": "module",
  "types": "./index.d.ts",
  "exports": {{
    ".": {{
      "types": "./index.d.ts"
    }}
  }}
}}
"#
    )
}

/// Minimal JSON string escaping — the inputs here are package names,
/// versions and a project name that [`super::validate_name`] has already
/// restricted to `[a-z0-9._-]`, so quote and backslash are the whole
/// alphabet of trouble.
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// The whole point of deriving both artifacts from one table: they cannot
    /// disagree. This test would still pass if someone re-hand-wrote them, so
    /// it is here to catch the emitters drifting, not the table.
    #[test]
    fn package_json_and_lock_pin_the_same_versions() {
        let pkg: Value = serde_json::from_str(&package_json("demo")).expect("package.json is JSON");
        let lock_src = bun_lock("demo");
        let lock: Value =
            serde_json::from_str(&strip_trailing_commas(&lock_src)).expect("bun.lock is JSONC");
        let packages = lock["packages"].as_object().expect("packages map");

        for p in CURATED {
            let entry = packages
                .get(p.name)
                .unwrap_or_else(|| panic!("{} missing from the emitted lock", p.name));
            let locator = entry[0].as_str().expect("locator");
            assert_eq!(locator, format!("{}@{}", p.name, p.version));

            let section = match p.role {
                Role::Runtime => &pkg["dependencies"],
                Role::Dev => &pkg["devDependencies"],
                // A transitive is in the closure but named by neither section.
                Role::Transitive => {
                    assert!(pkg["dependencies"].get(p.name).is_none());
                    assert!(pkg["devDependencies"].get(p.name).is_none());
                    continue;
                }
            };
            assert_eq!(
                section[p.name].as_str(),
                Some(p.version),
                "{} disagrees between package.json and the table",
                p.name
            );
        }
    }

    /// Every registry entry must carry sha512 integrity: `install.rs` refuses
    /// by name any entry without one rather than fetching on trust, so a
    /// missing integrity here is a lock that cannot install at all.
    #[test]
    fn every_registry_entry_carries_sha512_integrity() {
        for p in CURATED {
            assert!(
                p.integrity.starts_with("sha512-"),
                "{} has integrity {:?}; the materializer only verifies sha512",
                p.name,
                p.integrity
            );
        }
    }

    /// A range would be a resolution request on a path where nothing
    /// resolves — the pin has to be exact or it is not a pin.
    #[test]
    fn no_pinned_version_is_a_range() {
        for p in CURATED {
            assert!(
                !p.version.contains(['^', '~', '*', '>', '<', '|', ' ']),
                "{} is pinned to {:?}, which is a range, not a version",
                p.name,
                p.version
            );
        }
    }

    /// The closure has to be closed: react's `loose-envify`, react-dom's
    /// `scheduler`, `@types/react`'s `csstype` — every name any entry's
    /// metadata depends on must itself be in the lock, or the first
    /// `import "react"` in a client bundle fails to resolve after a
    /// materialize that reported success.
    #[test]
    fn the_set_is_closed_under_dependency() {
        let names: Vec<&str> = CURATED.iter().map(|p| p.name).collect();
        for p in CURATED {
            let meta: Value = serde_json::from_str(p.meta)
                .unwrap_or_else(|e| panic!("{} meta is not JSON: {e}", p.name));
            let Some(deps) = meta.get("dependencies").and_then(Value::as_object) else {
                continue;
            };
            for dep in deps.keys() {
                assert!(
                    names.contains(&dep.as_str()),
                    "{} depends on {dep}, which is not in the curated closure",
                    p.name
                );
            }
        }
    }

    /// The vended barrel is versioned with the binary. If this ever stops
    /// being true, `.mesofact-version` selects one runtime and the project's
    /// types describe another.
    #[test]
    fn vended_barrel_is_versioned_with_this_binary() {
        let v: Value = serde_json::from_str(&barrel_package_json()).expect("barrel pkg is JSON");
        assert_eq!(v["version"].as_str(), Some(MESOFACT_VERSION));
        assert_eq!(v["name"].as_str(), Some(RUNTIME_BARREL));
    }

    /// bun.lock is JSONC (trailing commas); `install.rs` has a string-aware
    /// stripper for exactly this. Mirrored here so the emitter's shape is
    /// checked without depending on the optional `build` feature.
    fn strip_trailing_commas(src: &str) -> String {
        let bytes = src.as_bytes();
        let mut out = String::with_capacity(src.len());
        let (mut in_string, mut escaped, mut i) = (false, false, 0);
        while i < bytes.len() {
            let c = bytes[i] as char;
            if in_string {
                out.push(c);
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    in_string = false;
                }
                i += 1;
                continue;
            }
            match c {
                '"' => {
                    in_string = true;
                    out.push(c);
                }
                ',' => {
                    let mut j = i + 1;
                    while j < bytes.len() && (bytes[j] as char).is_whitespace() {
                        j += 1;
                    }
                    if !(j < bytes.len() && (bytes[j] == b'}' || bytes[j] == b']')) {
                        out.push(c);
                    }
                }
                _ => out.push(c),
            }
            i += 1;
        }
        out
    }
}
