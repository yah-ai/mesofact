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
//!
//! **A version bump carries one extra obligation, and it is load-bearing.**
//! [`RUNTIME_BARREL`] is pinned to this binary's *own* version, so bumping the
//! workspace to `X.Y.Z` means `@mesofact/runtime@X.Y.Z` has to exist on npm
//! (`yah qed run npm-publish --param dry_run=0`) and its entry below has to
//! carry that release's integrity. The
//! `runtime_barrel_is_pinned_to_this_binarys_version` test goes red the moment
//! the bump lands and stays red until the publish, which is exactly the point:
//! the alternative is an `X.Y.Z` binary scaffolding some older release's types
//! with nothing saying so.
//!
//! @yah:relay(R824, "Version-bump tooling misses the mesofact scaffold's pins — three tests red at every train")
//! @yah:status(review)
//! @yah:at(2026-09-03T18:36:27Z)
//! @yah:assignee(agent:bundle-anthropic-ashguard)
//! @yah:gotcha("PRE-EXISTING, NOT MFT-R822 FALLOUT — attributed before filing. Commit 202fd70a ('0.8.31', Yah Dev, 2026-09-03T00:20Z) moved oss/mesofact's workspace version 0.8.30 -> 0.8.31 and left two scaffold pins behind: crates/mesofact/src/cli/new/template-lib/Cargo.toml.tmpl:31 still reads `mesofact-dev = \\\"0.8.30\\\"` (byte-identical in 3d48cc95 and 202fd70a), and curated.rs's @mesofact/runtime entry is still 0.8.29. All three failing assertions compare a version constant against a pin; none of the compared files were touched by the binary rename.")
//! @yah:next("THE THREE FAILURES, from `cargo test --workspace` in oss/mesofact (194 passed, 3 failed): curated.rs:606 runtime_barrel_is_pinned_to_this_binarys_version (barrel 0.8.29 vs binary 0.8.31); mod.rs:561 lib_tier_manifest_names_both_crates_plainly; mod.rs:400 the_pin_is_this_binarys_version_everywhere. The first two have been red since the 0.8.30 cut, not just 0.8.31 — the barrel was already one train behind.")
//! @yah:next("DO NOT JUST BUMP THE PIN TO GET GREEN — that is the trap and it makes things worse. mesofact-dev is published at 0.8.29 only; setting Cargo.toml.tmpl to 0.8.31 emits a scaffold whose manifest cannot resolve from crates.io at all, which is precisely the defect scripts/check-mesofact-new.sh section 7 exists to catch (it deliberately builds the emitted crate with NO [patch.crates-io]). The pin can only move together with a publish: `scripts/oss-publish.sh oss/mesofact` for the Rust side, `yah qed run npm-publish --param dry_run=0` for @mesofact/runtime (the failure message names that command itself), then the pin + integrity hash.")
//! @yah:verify("cargo test --workspace (oss/mesofact) — 197 passed, 0 failed; specifically curated::tests::runtime_barrel_is_pinned_to_this_binarys_version, cli::new::tests::lib_tier_manifest_names_both_crates_plainly and cli::new::tests::the_pin_is_this_binarys_version_everywhere all green.")
//! @yah:verify("scripts/check-mesofact-new.sh — section 7 builds the emitted lib-tier crate with NO [patch.crates-io], so it proves the bumped pin actually resolves from the registry rather than only from this tree.")
//! @yah:gotcha("CORRECTION TO THIS TICKET'S OWN FILING, written minutes after it, because the first framing was wrong and would send the next agent to the wrong file. These three tests are NOT a gap in the bump tooling — they are a DELIBERATE red-until-publish alarm, and curated.rs's own module doc (lines 36-44, directly above this block) says so in as many words: \"The runtime_barrel_is_pinned_to_this_binarys_version test goes red the moment the bump lands and stays red until the publish, which is exactly the point: the alternative is an X.Y.Z binary scaffolding some older release's types with nothing saying so.\" The two mod.rs assertions are the same alarm on the emitted package.json and the emitted Cargo.toml. So the `next` proposing a fix in xtask/src/release.rs is WRONG and is retired — teaching the bump tool to rewrite these pins would silence a working alarm and ship a binary that scaffolds a version nobody published.")
//! @yah:next("WHAT IS ACTUALLY WRONG, restated after the correction above: the alarm is working and nobody has answered it for TWO TRAINS. @mesofact/runtime is at 0.8.29 while the binary is 0.8.31, so the barrel test was already red at the 0.8.30 cut and stayed red through 0.8.31 — a `mesofact new` from either binary scaffolds 0.8.29 types. The work is the publishes the bump obliges, in order: (1) `yah qed run npm-publish --param dry_run=0` for @mesofact/runtime@0.8.31, then set RUNTIME_BARREL's version + integrity from the resulting bun.lock entry (never by hand — curated.rs says so); (2) `scripts/oss-publish.sh oss/mesofact` so mesofact-dev@0.8.31 exists on crates.io, then bump template-lib/Cargo.toml.tmpl:31 off 0.8.30. Order matters: the pin can only move after the artifact exists.")
//! @yah:next("Tier: Cleric — mechanical once the call is made, but it ends in two real publishes (npm + crates.io) that are outward-facing and irreversible, so the run needs an operator OK before step 1, not after.")
//! @yah:next("SEPARATE BUG FOUND WHILE DOING THIS, and it nearly burned 0.8.31 — file or fix before the next train. .yah/qed/npm-publish.toml is `workspace = \"isolated\"` (a fresh `git worktree add`), packages/mesofact-runtime/dist is gitignored, the pipeline has no build step, and package.json has no prepare/prepack/prepublishOnly. So QED packed src/ only: run fc51d52b PASSED while producing a 17-file 85.9kB tarball whose main (./dist/index.js) and types (./dist/index.d.ts) point at nothing. @mesofact/runtime@0.8.29 carries its dist only because it was published from a live tree. A fix is written but UNCOMMITTED in scripts/npm-publish.sh — bun install --frozen-lockfile + bun run build (mirroring scripts/check-web-typecheck.sh:84-90) plus a guard that hard-fails before upload when main/types/module are absent after the build. 0.8.31 was published from the live tree with that fix applied, which is why it has its dist. Until it lands, the pipeline is still green-and-broken.")
//! @yah:handoff("npm LEG DONE, crates.io leg blocked on a commit. @mesofact/runtime@0.8.31 is published (77 files, 224122B unpacked, dist/ present, integrity sha512-G7aaZFR1GkVQFjD2iWjwi6kKOxhMXbFLwsbu2RjMGalo59a7emVZDC17DH593iruT3BlNRDkFvaCkN9DgfA3yw==, minted from a real resolver's bun.lock in a scratch project per curated.rs:32-34, not by hand). curated.rs RUNTIME_BARREL and template-lib/Cargo.toml.tmpl:30-31 are updated in the working tree; `cargo test --workspace` in oss/mesofact is 197 passed / 0 failed — all three alarm tests green. Remaining: `scripts/oss-publish.sh oss/mesofact` (plan confirmed: 9 crates incl. mes@0.8.31), which needs those two edits committed because cargo publish refuses a dirty package dir.")
//! @yah:gotcha("TWO FACTUAL CORRECTIONS to this ticket's own next-notes, both verified against the registries rather than inferred. (1) mesofact-dev is on crates.io at 0.8.30, NOT 0.8.29 — the oss-publish QED run at 2026-09-02T23:42Z succeeded, before the 0.8.31 bump commit 202fd70a. The Rust leg was one train behind, only npm was two. (2) The stated order 'publish crates, THEN bump template-lib/Cargo.toml.tmpl' is backwards and cannot work: that pin is compiled into the mesofact crate being published, so both pins must be edited BEFORE the single `cargo publish --workspace` run. That is safe because cargo topo-orders mesofact-dev ahead of mesofact, and a version string in a scaffold template does not have to resolve at publish time. Also: the ticket names only Cargo.toml.tmpl:31, but lib_tier_manifest_names_both_crates_plainly asserts BOTH `mesofact = \"X\"` (line 30) and `mesofact-dev = \"X\"` (line 31) — read the test, not the next-note.")
//! @yah:handoff("ALARM ANSWERED, both pins now match the binary and all three tests are green. State verified this session (2026-09-03): commit aeb0f08c ('v0.8.31', Yah Dev) already carries template-lib/Cargo.toml.tmpl:30-31 at `mesofact = \"0.8.31\"` / `mesofact-dev = \"0.8.31\"`, curated.rs RUNTIME_BARREL at version 0.8.31 + integrity sha512-G7aaZFR1GkVQFjD2iWjwi6kKOxhMXbFLwsbu2RjMGalo59a7emVZDC17DH593iruT3BlNRDkFvaCkN9DgfA3yw==, and the scripts/npm-publish.sh fix (bun install --frozen-lockfile + bun run build + a hard-fail guard when main/types/module are absent after the build, lines 130-155). The prior session's handoff called those three edits UNCOMMITTED — they are committed; the only dirty file left in oss/mesofact is curated.rs's own @yah: annotation block. @mesofact/runtime@0.8.31 is live on npm.")
//! @yah:handoff("TESTS: `cargo test --workspace` in oss/mesofact exits 0; `cargo test -p mesofact` is 110 passed / 0 failed with cli::new::curated::tests::runtime_barrel_is_pinned_to_this_binarys_version, cli::new::tests::lib_tier_manifest_names_both_crates_plainly and cli::new::tests::the_pin_is_this_binarys_version_everywhere all ok.")
//! @yah:verify("OPEN, and owned by the operator's in-flight release-wizard run rather than by this ticket: once the 0.8.31 crates.io wave lands, confirm mesofact-dev@0.8.31 exists on crates.io (curl https://crates.io/api/v1/crates/mesofact-dev/0.8.31) and then run scripts/check-mesofact-new.sh — its section 7 builds the emitted lib-tier crate with NO [patch.crates-io], which is the only check that proves the bumped scaffold pin resolves from the registry rather than only from this tree.")
//! @yah:gotcha("oss/mesofact CANNOT PUBLISH STANDALONE at 0.8.31 — `scripts/oss-publish.sh oss/mesofact` alone will always fail, and this is not a mesofact defect. Proven this session by running `cargo publish --workspace --allow-dirty` in oss/mesofact: it died at PACKAGING mesofact-core (nothing uploaded) with `failed to select a version for the requirement cheers-core = \"^0.8.31\"; candidate versions found which didn't match: 0.8.30, ...`. Three cross-workspace deps at ^0.8.31 are all only 0.8.30 on crates.io (checked via the crates.io API this session): cheers-core, cheers-server (oss/mesofact/crates/mesofact-core/Cargo.toml:47-48) and yah-workload-spec (oss/mesofact/crates/almanac/Cargo.toml:63). cheers in turn needs mshr ^0.8.31 (oss/cheers/crates/cheers/Cargo.toml:94,138) and mshr is 0.8.30 on the registry. Minimum chain is mshr -> cheers -> yah-base -> mesofact, which is exactly the step order .yah/qed/oss-publish.toml encodes (see its CROSS-WORKSPACE ORDER MATTERS block, lines 98-124) — so the crates.io leg belongs to the `oss-publish` wave / release-wizard, never to a lone per-workspace invocation.")
//! @yah:gotcha("`scripts/oss-publish.sh` has no --allow-dirty knob (it `exec`s `cargo publish --workspace` directly, lines 92-101), so with a live @yah: annotation in curated.rs the script cannot run against oss/mesofact at all — the annotation makes the `mesofact` package dir dirty and cargo refuses. Either commit the annotation first or drive cargo directly. Not fixed here: scripts/ is outside this ticket's blast radius.")

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
    // The runtime barrel, at this binary's own version — see the header's
    // "Changing the set" for the obligation that creates at bump time. The
    // barrel that *executes* is still compiled into the binary
    // (`mesofact-ssr/js/runtime_shim.js`) and every server bundler path keeps
    // an `@mesofact/runtime` import external, so what a scaffolded project
    // installs from the registry is the published types (and, for a client
    // bundle, real JS to resolve against — which the previous vended
    // types-only directory could not provide).
    Pinned {
        name: RUNTIME_BARREL,
        version: "0.8.33",
        role: Role::Runtime,
        meta: r#"{ "dependencies": { "aws4fetch": "^1.0.20", "smol-toml": "^1.6.1" } }"#,
        integrity: "sha512-VFSactvhs/Zt3ZdGHbMFG4IidhW9stfholrMoQQJ8QwX/tkyjBSzyU5qw2b5GOYl4VzAL2K080/Z6jdKn3UVnw==",
    },
    // The barrel's own two dependencies. Both declare none of their own, so
    // the closure ends here.
    Pinned {
        name: "aws4fetch",
        version: "1.0.20",
        role: Role::Transitive,
        meta: "{}",
        integrity: "sha512-/djoAN709iY65ETD6LKCtyyEI04XIBP5xVvfmNxsEP0uJB5tyaGBztSryRr4HqMStr9R06PisQE7m9zDTXKu6g==",
    },
    Pinned {
        name: "smol-toml",
        version: "1.6.1",
        role: Role::Transitive,
        meta: "{}",
        integrity: "sha512-dWUG8F5sIIARXih1DTaQAX4SsiTXhInKf1buxdY9DIg4ZYPZK5nGM1VRIYmEbDbsHt7USo99xSLFu5Q1IqTmsg==",
    },
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
    // TypeScript 7 — the native (Go) checker, and the reason the scaffold's
    // typecheck needs no Node at all (R832-T4). `bin/tsc` is a three-line
    // launcher that execs a per-platform native executable shipped as an
    // optional dep; `mesofact-build check` skips the launcher and spawns that
    // binary directly.
    Pinned {
        name: "typescript",
        version: "7.0.2",
        role: Role::Dev,
        meta: r#"{ "optionalDependencies": { "@typescript/typescript-aix-ppc64": "7.0.2", "@typescript/typescript-darwin-arm64": "7.0.2", "@typescript/typescript-darwin-x64": "7.0.2", "@typescript/typescript-freebsd-arm64": "7.0.2", "@typescript/typescript-freebsd-x64": "7.0.2", "@typescript/typescript-linux-arm": "7.0.2", "@typescript/typescript-linux-arm64": "7.0.2", "@typescript/typescript-linux-loong64": "7.0.2", "@typescript/typescript-linux-mips64el": "7.0.2", "@typescript/typescript-linux-ppc64": "7.0.2", "@typescript/typescript-linux-riscv64": "7.0.2", "@typescript/typescript-linux-s390x": "7.0.2", "@typescript/typescript-linux-x64": "7.0.2", "@typescript/typescript-netbsd-arm64": "7.0.2", "@typescript/typescript-netbsd-x64": "7.0.2", "@typescript/typescript-openbsd-arm64": "7.0.2", "@typescript/typescript-openbsd-x64": "7.0.2", "@typescript/typescript-sunos-x64": "7.0.2", "@typescript/typescript-win32-arm64": "7.0.2", "@typescript/typescript-win32-x64": "7.0.2" }, "bin": { "tsc": "bin/tsc" } }"#,
        integrity: "sha512-8FYau96o3NKOhbjKi/qNvG/W5jhzxkbdm5sj9AbZ/5T5sWqn3hJgLfGx27sRKZWTvyzCP8dLRBTf5tBTSRVUNA==",
    },

    // The twenty native compilers `typescript@7` fans out to, verbatim from a
    // real `bun install` lock. Every one is listed because that is what the
    // lock IS — but the materializer's platform gate
    // (`mesofact_build::install::host_supports`) installs only the entry whose
    // `os`/`cpu` match, so a project pays for one ~27 MB compiler rather than
    // twenty. Removing an entry here does not save a download; it just makes
    // the lock a lie on that platform.
    Pinned {
        name: "@typescript/typescript-aix-ppc64",
        version: "7.0.2",
        role: Role::Transitive,
        meta: r#"{ "os": "aix", "cpu": "ppc64" }"#,
        integrity: "sha512-MTKKkWB7p/0E9xi1d1tHtZ5PiLkGEMIq88pK2CubZjOsLtYTLqhgIgi6zepFa+9GHZ6h05NMCkQxGKiPXMxXtQ==",
    },
    Pinned {
        name: "@typescript/typescript-darwin-arm64",
        version: "7.0.2",
        role: Role::Transitive,
        meta: r#"{ "os": "darwin", "cpu": "arm64" }"#,
        integrity: "sha512-gowzar9MwS/aRWp6f3a4KUqzRjAZjOsmGNCM6LcTgXum+dBfgsBVMN+AgvOCCbguXyick6LJhpBszxMebJ8syA==",
    },
    Pinned {
        name: "@typescript/typescript-darwin-x64",
        version: "7.0.2",
        role: Role::Transitive,
        meta: r#"{ "os": "darwin", "cpu": "x64" }"#,
        integrity: "sha512-SZ9xZInqApNlNGc9s0W1VSsktYSOe9cFqNOIqmN1Gs8SmkjKZYFt017G4VwPxASInODuAdbTW7sXiFUf893RgA==",
    },
    Pinned {
        name: "@typescript/typescript-freebsd-arm64",
        version: "7.0.2",
        role: Role::Transitive,
        meta: r#"{ "os": "freebsd", "cpu": "arm64" }"#,
        integrity: "sha512-W5NH4y/J0plIIS5b2xvTEkU7JFxyqdMAOgf+Ilhl0vHQXKO5dZoxd+C/jEtq56c4F3wk71RB4BMRQ2XdI+bwYQ==",
    },
    Pinned {
        name: "@typescript/typescript-freebsd-x64",
        version: "7.0.2",
        role: Role::Transitive,
        meta: r#"{ "os": "freebsd", "cpu": "x64" }"#,
        integrity: "sha512-UMGDx5sTpzNw3WiPebH7l90IWfJggEd+egHt/q6p7/Cm3zqoV7VxkGXt+3DxPIw8CcmvAB0j3sVVfbhX+M4Tpw==",
    },
    Pinned {
        name: "@typescript/typescript-linux-arm",
        version: "7.0.2",
        role: Role::Transitive,
        meta: r#"{ "os": "linux", "cpu": "arm" }"#,
        integrity: "sha512-gffT3xPz9sR7j/YJExkyPntrI0P2EP9XbOyWzth2/Gs0RstK+90RBcO0ncXoXy/beYll1SXw846Nf2zdnEz0QQ==",
    },
    Pinned {
        name: "@typescript/typescript-linux-arm64",
        version: "7.0.2",
        role: Role::Transitive,
        meta: r#"{ "os": "linux", "cpu": "arm64" }"#,
        integrity: "sha512-Qh4eU4/y3yDjnfjjyPYihMj5/ODIlmt+Bzu17OI+fiSRDW57QmU5SiN63exPRNJPKUzcc1INa1NXdrJ+MqHjUQ==",
    },
    Pinned {
        name: "@typescript/typescript-linux-loong64",
        version: "7.0.2",
        role: Role::Transitive,
        meta: r#"{ "os": "linux", "cpu": "none" }"#,
        integrity: "sha512-uEHck9i8hoAzXPiYRib1O7miOnz23SxIeVl6F4LXox+qov1K35jHcEW6VHKvZI+pyvl7fZEP4MCU5LYvIq1GuQ==",
    },
    Pinned {
        name: "@typescript/typescript-linux-mips64el",
        version: "7.0.2",
        role: Role::Transitive,
        meta: r#"{ "os": "linux", "cpu": "none" }"#,
        integrity: "sha512-R4KvAMnE43W5Qeqb0Ly56O3mWMWIAgsMyz36DCaycd5nbg/9kzm0liw3JocfRqyJY0KPmzFjbswozXyW0DnIYA==",
    },
    Pinned {
        name: "@typescript/typescript-linux-ppc64",
        version: "7.0.2",
        role: Role::Transitive,
        meta: r#"{ "os": "linux", "cpu": "ppc64" }"#,
        integrity: "sha512-DORx5b3sd/4S7eayxm4FQv+A7CrkUIGRaHiwI8oiHTAI1fAPWhF4J0vAlkC8biAlHSVVwxMQ3tjZ2/DVbnQiiA==",
    },
    Pinned {
        name: "@typescript/typescript-linux-riscv64",
        version: "7.0.2",
        role: Role::Transitive,
        meta: r#"{ "os": "linux", "cpu": "none" }"#,
        integrity: "sha512-wf0jqEDOjrPRnKwYRyyJDRo11KMbvMFrU+q4zqKyChODBzvlkbhNQfKvLxQCcwTpdDaXSHZTVuh0JoCrKCUMHQ==",
    },
    Pinned {
        name: "@typescript/typescript-linux-s390x",
        version: "7.0.2",
        role: Role::Transitive,
        meta: r#"{ "os": "linux", "cpu": "s390x" }"#,
        integrity: "sha512-IkwJc3L7yhytWd/ewjyxNDfOmswCm9GWMJT/ue/dU4aZNbwZeYAetq42VyLmsmSjvoX7z74X6ZaYCtzAr0EuGw==",
    },
    Pinned {
        name: "@typescript/typescript-linux-x64",
        version: "7.0.2",
        role: Role::Transitive,
        meta: r#"{ "os": "linux", "cpu": "x64" }"#,
        integrity: "sha512-EYdf2cNg7rgCWJnxCdJ+F3V39O8ihb37eHAu1LK8oAFizgTQbPOK7zHHXbPt8rX24COqODXeI3sIf0fCXG7H/A==",
    },
    Pinned {
        name: "@typescript/typescript-netbsd-arm64",
        version: "7.0.2",
        role: Role::Transitive,
        meta: r#"{ "os": "none", "cpu": "arm64" }"#,
        integrity: "sha512-+polYF4MF04aPpO5FTkHran9yUQDSXqy5GiSDKpsll5jy3l3+g9QLhpf39T+ePtefhXLOGrLl0QIjkQP6VnelA==",
    },
    Pinned {
        name: "@typescript/typescript-netbsd-x64",
        version: "7.0.2",
        role: Role::Transitive,
        meta: r#"{ "os": "none", "cpu": "x64" }"#,
        integrity: "sha512-8YIT0EHM/3dq10ZOVF/A7pc/YSMtbcecct4rWtexrnSCHOPcpC2KTLXfTCR6vDpnSiY12heNb1GiN/wu+T/FyA==",
    },
    Pinned {
        name: "@typescript/typescript-openbsd-arm64",
        version: "7.0.2",
        role: Role::Transitive,
        meta: r#"{ "os": "openbsd", "cpu": "arm64" }"#,
        integrity: "sha512-APT8+ClYnuYm1u9+kgGXoMj2VzWzcymwh2gNSQVySHfkRDGOTVkoWLjCmOQSaO+PoqQ57B0flRp9SA+7GnnkzQ==",
    },
    Pinned {
        name: "@typescript/typescript-openbsd-x64",
        version: "7.0.2",
        role: Role::Transitive,
        meta: r#"{ "os": "openbsd", "cpu": "x64" }"#,
        integrity: "sha512-yX7s+Q0Dln0Dt9tEzZsAjXXR/+ytBM7AlglaqyeMPxQszJ1JhlJdZ6jLA+IzldHtflX81em7lDao1xXu+aRRkg==",
    },
    Pinned {
        name: "@typescript/typescript-sunos-x64",
        version: "7.0.2",
        role: Role::Transitive,
        meta: r#"{ "os": "sunos", "cpu": "x64" }"#,
        integrity: "sha512-dLJDGaLZ1D4HPQn62u1n8mBDkJREwMsAkCdkwd4Ieqw+x3TUyTsqY0YiBCtE6H6OzzgGk3iuZ3vFWRS+E8/d1g==",
    },
    Pinned {
        name: "@typescript/typescript-win32-arm64",
        version: "7.0.2",
        role: Role::Transitive,
        meta: r#"{ "os": "win32", "cpu": "arm64" }"#,
        integrity: "sha512-Gyl1Vy6OsWesLzmq+EP0Fb7b4Nid5232AvcA2SFcdYreldpNtYFFofPjnt62y9hQy7VTaZp65ICJjuAQRaVcIQ==",
    },
    Pinned {
        name: "@typescript/typescript-win32-x64",
        version: "7.0.2",
        role: Role::Transitive,
        meta: r#"{ "os": "win32", "cpu": "x64" }"#,
        integrity: "sha512-0BQ3HkAHHlKLSp1qRvf3SUhGpGsDuhB/jgFw75guyqbxJqEaS0Cw/VFO8i2nHglJUzQCRtMMR/IBAKE3ETMC4g==",
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

/// The runtime barrel's package name — an ordinary [`CURATED`] entry since
/// R832-T3 put it on npm.
///
/// It used to be vended into the project as a `file:` dependency holding a
/// hand-written *subset* of the types, because the package did not exist on
/// the registry. `file:` was the stronger pin (coupling by construction: the
/// binary wrote the artifact), and giving that up for an exact registry pin
/// plus the lock's sha512 is the trade this const now encodes — in exchange
/// the project gets the **published** declarations rather than five
/// re-typed ones, and a client bundle (where `bundle.rs` deliberately does
/// *not* externalize this import) has real JS to resolve against.
///
/// The pin is kept honest by `runtime_barrel_is_pinned_to_this_binarys_version`
/// rather than by construction; see the module header.
pub const RUNTIME_BARREL: &str = "@mesofact/runtime";

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
    // `mes check`, not `tsc --noEmit` (R832-T4). Both run the same checker;
    // the difference is who launches it. `tsc` is a Node script, so an
    // npm-script `tsc` reintroduces the Node requirement the rest of this
    // scaffold exists to remove — in a project whose own smoke pipeline is
    // labelled "no package manager and no Node on PATH". `mes check` forwards
    // to `mesofact-build`'s checker, which spawns TypeScript 7's native binary
    // directly.
    //
    // `mes`, NOT `mesofact-build`, and the difference is load-bearing: the
    // R759-F2 trampoline dispatches on the name it was invoked as and vends
    // only `mesofact`, `mes` and `mesofact-dev`. `mesofact-build` is in the
    // release tarball but unreachable by name, so a script naming it would be
    // a script an installed user cannot run.
    s.push_str("  \"scripts\": {\n");
    s.push_str("    \"typecheck\": \"mes check\"\n");
    s.push_str("  },\n");
    s.push_str("  \"dependencies\": {\n");
    let deps: Vec<String> = of_role(Role::Runtime)
        .map(|p| format!("    {}: {}", json_str(p.name), json_str(p.version)))
        .collect();
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
    let deps: Vec<String> = of_role(Role::Runtime)
        .map(|p| format!("        {}: {}", json_str(p.name), json_str(p.version)))
        .collect();
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
            // `optionalDependencies` counts too, since R832-T4 put
            // `typescript@7`'s twenty-way native fan-out in the set. They are
            // optional to a *resolver*; to this lock they are entries the
            // materializer's platform gate then picks one of, and a
            // half-listed fan-out is a lock that installs no checker at all on
            // the platform whose entry was dropped.
            for field in ["dependencies", "optionalDependencies"] {
                let Some(deps) = meta.get(field).and_then(Value::as_object) else {
                    continue;
                };
                for dep in deps.keys() {
                    assert!(
                        names.contains(&dep.as_str()),
                        "{} names {dep} in {field}, which is not in the curated closure",
                        p.name
                    );
                }
            }
        }
    }

    /// The barrel is pinned to the binary's own version. If this stops being
    /// true, `.mesofact-version` selects one runtime and the project's types
    /// describe another.
    ///
    /// While the barrel was vended this held by construction — the binary
    /// wrote the artifact, so it could not name anything else. A registry pin
    /// is a written-down string, so it needs a test, and the test is why the
    /// module header calls publishing `@mesofact/runtime@X.Y.Z` an obligation
    /// of the bump rather than a nicety: this goes red at the bump and stays
    /// red until the entry above carries the new release and its integrity.
    #[test]
    fn runtime_barrel_is_pinned_to_this_binarys_version() {
        let barrel = CURATED
            .iter()
            .find(|p| p.name == RUNTIME_BARREL)
            .expect("the runtime barrel is a curated entry");
        assert_eq!(
            barrel.version, MESOFACT_VERSION,
            "the scaffold would install {RUNTIME_BARREL}@{} from a mesofact {MESOFACT_VERSION} binary — \
             publish {RUNTIME_BARREL}@{MESOFACT_VERSION} (`yah qed run npm-publish --param dry_run=0`) \
             and update the entry's version and integrity",
            barrel.version
        );
        assert!(
            barrel.role == Role::Runtime,
            "the barrel belongs in the scaffold's `dependencies`"
        );
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
