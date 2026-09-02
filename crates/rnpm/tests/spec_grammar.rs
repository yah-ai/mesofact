//! R773-F2's two verification criteria, as tests.
//!
//! 1. A table-driven test over npm range syntax — caret, tilde, x-ranges,
//!    hyphen ranges, prerelease rules, `||` — matching `node-semver`'s
//!    behaviour.
//! 2. Every spec form the grammar does not support is refused with a message
//!    naming the form, never silently mis-parsed.
//!
//! Everything here goes through the public API, and nothing here touches a
//! network or a filesystem: the grammar is parse-only by design.

use rnpm::spec::{GitHost, GitSource, PackageSpec, SpecError, SpecErrorKind, Version, VersionSpec};

/// Parse a manifest entry the way R773-F3's walk will: `("react", "^18.0.0")`.
fn dep(spec: &str) -> VersionSpec {
    match PackageSpec::from_dependency("p", spec) {
        Ok(PackageSpec::Npm { requested: Some(v), .. }) => v,
        other => panic!("{spec:?} did not parse to a version spec: {other:?}"),
    }
}

fn satisfies(range: &str, version: &str) -> bool {
    dep(range).matches(&Version::parse(version).expect("test version parses"))
}

/// The table. Each row is `(range, version, expected)`; a row that disagrees
/// with `node-semver` fails naming itself, so a future range-semantics
/// divergence from pnpm is a named failure rather than a mystery in
/// R773-F7's conformance corpus.
const RANGES: &[(&str, &str, bool)] = &[
    // ---- caret: compatible-with, pinned at the leftmost non-zero digit ----
    ("^1.2.3", "1.2.3", true),
    ("^1.2.3", "1.2.4", true),
    ("^1.2.3", "1.9.9", true),
    ("^1.2.3", "1.2.2", false),
    ("^1.2.3", "2.0.0", false),
    ("^0.2.3", "0.2.3", true),
    ("^0.2.3", "0.2.9", true),
    ("^0.2.3", "0.3.0", false),
    ("^0.0.3", "0.0.3", true),
    ("^0.0.3", "0.0.4", false),
    ("^1.2.x", "1.2.0", true),
    ("^1.2.x", "1.3.0", true),
    ("^1.2.x", "2.0.0", false),
    ("^0.0.x", "0.0.5", true),
    ("^0.0.x", "0.1.0", false),
    // ---- tilde: patch-level within a stated minor ----
    ("~1.2.3", "1.2.3", true),
    ("~1.2.3", "1.2.99", true),
    ("~1.2.3", "1.3.0", false),
    ("~1.2", "1.2.5", true),
    ("~1.2", "1.3.0", false),
    ("~1", "1.5.0", true),
    ("~1", "2.0.0", false),
    ("~0.2.3", "0.2.4", true),
    ("~0.2.3", "0.3.0", false),
    // `~>` is RubyGems' pessimistic operator, not npm syntax — but
    // node-semver 2.2.0 accepts it and reads it as plain `~`. Recorded rather
    // than pre-filtered: rejecting it here would be precisely the silent
    // divergence from pnpm that pinning 2.2.0 exists to prevent (W318 §8).
    ("~>1.2.3", "1.2.9", true),
    ("~>1.2.3", "1.3.0", false),
    // ---- x-ranges and partials ----
    ("1.2.x", "1.2.7", true),
    ("1.2.x", "1.3.0", false),
    ("1.x", "1.9.9", true),
    ("1.x", "2.0.0", false),
    ("1.2", "1.2.9", true),
    ("1.2", "1.3.0", false),
    ("1", "1.0.0", true),
    ("1", "2.0.0", false),
    ("*", "0.0.1", true),
    ("*", "99.9.9", true),
    // ---- hyphen ranges, inclusive both ends, partials widening the upper ----
    ("1.2.3 - 2.3.4", "1.2.3", true),
    ("1.2.3 - 2.3.4", "2.3.4", true),
    ("1.2.3 - 2.3.4", "2.3.5", false),
    ("1.2.3 - 2.3.4", "1.2.2", false),
    ("1.2 - 2.3.4", "1.2.0", true),
    ("1.2.3 - 2.3", "2.3.4", true),
    ("1.2.3 - 2.3", "2.4.0", false),
    // ---- comparators ----
    (">=1.2.3 <2.0.0", "1.5.0", true),
    (">=1.2.3 <2.0.0", "2.0.0", false),
    (">1.2.3", "1.2.4", true),
    (">1.2.3", "1.2.3", false),
    ("=1.2.3", "1.2.3", true),
    ("=1.2.3", "1.2.4", false),
    // ---- `||` alternatives ----
    ("^1.0.0 || ^3.0.0", "1.5.0", true),
    ("^1.0.0 || ^3.0.0", "3.1.0", true),
    ("^1.0.0 || ^3.0.0", "2.0.0", false),
    ("<1.0.0 || >=2.0.0", "0.9.0", true),
    ("<1.0.0 || >=2.0.0", "1.5.0", false),
    // ---- prerelease: the rule implementations diverge on ----
    //
    // npm's rule (node-semver-2.2.0/src/range.rs:90-122): a prerelease
    // version satisfies a comparator set only when some bound carries the SAME
    // major.minor.patch AND is itself a prerelease. Everything below is that
    // one rule seen from several angles.
    ("^1.2.3", "1.2.4-beta.1", false),
    ("^1.2.3", "1.2.3-beta.1", false),
    (">=1.0.0", "2.0.0-beta", false),
    ("*", "1.0.0-rc.1", false),
    ("1.x", "1.5.0-alpha", false),
    ("^1.2.3-alpha", "1.2.3-alpha", true),
    ("^1.2.3-alpha", "1.2.3-alpha.1", true),
    ("^1.2.3-alpha", "1.2.3-beta", true),
    ("^1.2.3-alpha", "1.2.4-beta", false),
    // A non-prerelease inside the same bounds is unaffected by any of this.
    ("^1.2.3-alpha", "1.3.0", true),
    (">=1.2.3-alpha <2.0.0", "1.2.3-beta", true),
    (">=1.2.3-alpha <2.0.0", "1.9.0", true),
    ("~1.2.3-beta.2", "1.2.3-beta.4", true),
    ("~1.2.3-beta.2", "1.2.3-beta.1", false),
    ("~1.2.3-beta.2", "1.2.4-beta.2", false),
    // Prerelease ordering is numeric-before-alphanumeric, and any prerelease
    // sorts below its own release.
    (">1.0.0-alpha.1", "1.0.0-alpha.2", true),
    (">1.0.0-alpha.beta", "1.0.0-alpha.2", false),
];

#[test]
fn npm_range_syntax_matches_node_semver() {
    let mut wrong = Vec::new();
    for (range, version, expected) in RANGES {
        let got = satisfies(range, version);
        if got != *expected {
            wrong.push(format!(
                "  {range:?} vs {version:?}: expected {expected}, got {got}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {} range rows disagree with node-semver:\n{}",
        wrong.len(),
        RANGES.len(),
        wrong.join("\n")
    );
}

#[test]
fn the_range_table_actually_covers_every_form_it_claims_to() {
    for form in ["^", "~", ".x", " - ", "||", ">=", "-beta", "-alpha", "*"] {
        assert!(
            RANGES.iter().any(|(r, _, _)| r.contains(form)),
            "the table lost its {form:?} rows"
        );
    }
}

#[test]
fn an_exact_version_parses_as_a_version_not_a_one_element_range() {
    assert!(matches!(dep("1.2.3"), VersionSpec::Version(_)));
    assert!(matches!(dep("1.2.3-alpha.1"), VersionSpec::Version(_)));
    assert!(matches!(dep("=1.2.3"), VersionSpec::Range(_)));
    assert!(dep("1.2.3").matches(&Version::parse("1.2.3").unwrap()));
    assert!(!dep("1.2.3").matches(&Version::parse("1.2.4").unwrap()));
}

#[test]
fn a_dist_tag_stays_data_and_never_resolves_itself() {
    for tag in ["latest", "next", "canary", "beta", "experimental-1"] {
        assert_eq!(dep(tag), VersionSpec::Tag(tag.to_string()), "{tag}");
    }
    // Parse-only: a tag cannot answer a satisfaction question without a
    // packument, so it answers `false` rather than guessing.
    assert!(!dep("latest").matches(&Version::parse("1.0.0").unwrap()));
}

#[test]
fn a_missing_version_is_recorded_as_absent_rather_than_invented() {
    let bare = PackageSpec::parse("react").unwrap();
    assert_eq!(
        bare,
        PackageSpec::Npm { scope: None, name: "react".into(), requested: None }
    );
    assert_eq!(PackageSpec::from_dependency("react", "").unwrap(), bare);
    // `*` is a stated range and stays one — the distinction is what lets a
    // lock writer reproduce what the manifest said.
    assert!(matches!(dep("*"), VersionSpec::Range(_)));
}

// ---------------------------------------------------------------------------
// Spec forms
// ---------------------------------------------------------------------------

#[test]
fn a_scoped_name_keeps_its_scope_and_its_version() {
    let spec = PackageSpec::parse("@babel/core@^7.24.0").unwrap();
    let PackageSpec::Npm { scope, name, requested } = spec else {
        panic!("expected an npm spec");
    };
    assert_eq!(scope.as_deref(), Some("babel"));
    assert_eq!(name, "@babel/core");
    assert!(matches!(requested, Some(VersionSpec::Range(_))));
}

#[test]
fn an_npm_alias_records_both_the_install_name_and_the_real_package() {
    for input in ["foo@npm:bar@^1.2.3", "foo@NPM:bar@^1.2.3"] {
        let spec = PackageSpec::parse(input).unwrap();
        let PackageSpec::Alias { name, spec: target } = &spec else {
            panic!("{input}: expected an alias, got {spec:?}");
        };
        assert_eq!(name, "foo");
        assert_eq!(target.name(), Some("bar"));
        assert!(spec.is_alias());
        assert_eq!(spec.target().name(), Some("bar"));
    }

    // The manifest shape of the same thing.
    let entry = PackageSpec::from_dependency("foo", "npm:@scope/bar@^1").unwrap();
    assert_eq!(entry.name(), Some("foo"));
    assert_eq!(entry.target().name(), Some("@scope/bar"));
}

#[test]
fn a_bare_npm_prefix_is_not_an_alias_just_a_package() {
    let spec = PackageSpec::parse("npm:lodash@^4").unwrap();
    assert!(!spec.is_alias());
    assert_eq!(spec.name(), Some("lodash"));
}

#[test]
fn file_and_bare_paths_both_parse_to_a_directory() {
    for input in ["file:../pkg", "FILE:../pkg", "../pkg"] {
        assert!(
            matches!(PackageSpec::parse(input).unwrap(), PackageSpec::Dir { .. }),
            "{input}"
        );
    }
    for input in ["./pkg", "/abs/pkg", ".", ".."] {
        assert!(
            matches!(PackageSpec::parse(input).unwrap(), PackageSpec::Dir { .. }),
            "{input}"
        );
    }
    // A manifest entry keeps the key it installs under.
    let entry = PackageSpec::from_dependency("runtime", "file:../runtime").unwrap();
    assert_eq!(entry.name(), Some("runtime"));
    assert!(matches!(entry.target(), PackageSpec::Dir { .. }));
}

#[test]
fn git_shorthands_and_urls_parse_with_their_committish() {
    let plain = PackageSpec::parse("github:owner/repo").unwrap();
    assert_eq!(
        plain,
        PackageSpec::Git(rnpm::spec::GitSpec {
            source: GitSource::Hosted {
                host: GitHost::GitHub,
                owner: "owner".into(),
                repo: "repo".into()
            },
            committish: None,
            semver: None,
        })
    );

    // The prefix-less `owner/repo` shorthand is GitHub, per npm.
    let PackageSpec::Git(bare) = PackageSpec::parse("owner/repo#main").unwrap() else {
        panic!("expected a git spec");
    };
    assert_eq!(
        bare.source,
        GitSource::Hosted { host: GitHost::GitHub, owner: "owner".into(), repo: "repo".into() }
    );
    assert_eq!(bare.committish.as_deref(), Some("main"));

    for (input, host) in [
        ("gitlab:o/r", GitHost::GitLab),
        ("gist:o/r", GitHost::Gist),
        ("bitbucket:o/r", GitHost::Bitbucket),
    ] {
        let PackageSpec::Git(g) = PackageSpec::parse(input).unwrap() else {
            panic!("{input}");
        };
        assert!(matches!(g.source, GitSource::Hosted { host: h, .. } if h == host), "{input}");
    }

    // `.git` is stripped; a `#semver:` committish becomes a range.
    let PackageSpec::Git(sem) = PackageSpec::parse("github:o/r.git#semver:^1.2.0").unwrap() else {
        panic!("expected a git spec");
    };
    assert_eq!(
        sem.source,
        GitSource::Hosted { host: GitHost::GitHub, owner: "o".into(), repo: "r".into() }
    );
    assert_eq!(sem.committish, None);
    assert!(sem
        .semver
        .expect("a semver committish")
        .satisfies(&Version::parse("1.5.0").unwrap()));

    for url in [
        "git+https://host/o/r.git#abc123",
        "git+ssh://git@host/o/r.git",
        "git://host/o/r.git",
        "ssh://git@host:o/r.git",
    ] {
        let PackageSpec::Git(g) = PackageSpec::parse(url).unwrap() else {
            panic!("{url}");
        };
        assert!(matches!(g.source, GitSource::Url(_)), "{url}");
    }
}

#[test]
fn a_direct_tarball_url_parses_and_a_scoped_name_never_becomes_one() {
    for url in [
        "https://host/pkg-1.0.0.tgz",
        "http://host/path/pkg.tar.gz",
        "https://host/pkg.tgz?token=abc",
    ] {
        assert_eq!(
            PackageSpec::parse(url).unwrap(),
            PackageSpec::Tarball { url: url.to_string() },
            "{url}"
        );
    }
    // A `git+https://…` URL is git, not a tarball, even ending in `.git`.
    assert!(matches!(
        PackageSpec::parse("git+https://host/o/r.git").unwrap(),
        PackageSpec::Git(_)
    ));
}

#[test]
fn a_parsed_spec_renders_back_to_something_that_reparses() {
    for input in [
        "react@^18.0.0",
        "@babel/core@7.24.0",
        "lodash@latest",
        "foo@npm:bar@^1.2.3",
        "github:owner/repo#main",
        "github:owner/repo#semver:^1.2.0",
        "https://host/pkg-1.0.0.tgz",
    ] {
        let once = PackageSpec::parse(input).unwrap();
        let rendered = once.to_string();
        let twice = PackageSpec::parse(&rendered)
            .unwrap_or_else(|e| panic!("{input} rendered as {rendered:?}, which failed: {e}"));
        assert_eq!(once, twice, "{input} -> {rendered}");
    }
}

// ---------------------------------------------------------------------------
// Refusals — criterion 2
// ---------------------------------------------------------------------------

/// The protocols this resolver deliberately does not implement. Each must be
/// refused *naming its own scheme*, because the failure mode being prevented
/// is a `workspace:^` silently parsing as a package called `workspace` at
/// version `^`.
#[test]
fn an_unsupported_protocol_is_refused_by_name() {
    for (input, scheme) in [
        ("workspace:^", "workspace"),
        ("workspace:*", "workspace"),
        ("catalog:default", "catalog"),
        ("link:../sibling", "link"),
        ("patch:foo@1.0.0", "patch"),
        ("portal:./local", "portal"),
        ("jsr:@luca/flag", "jsr"),
        ("bun:sqlite", "bun"),
    ] {
        let err = PackageSpec::parse(input).unwrap_err();
        assert_eq!(
            err.kind,
            SpecErrorKind::UnsupportedProtocol(scheme.to_string()),
            "{input}"
        );
        let message = err.to_string();
        assert!(message.contains(scheme), "{input}: {message}");
        assert!(message.contains(input), "{input}: {message}");
    }
}

/// The same protocols reached through a manifest entry, which is how they
/// will actually arrive.
#[test]
fn an_unsupported_protocol_in_a_manifest_entry_is_refused_too() {
    for (value, scheme) in [
        ("workspace:^", "workspace"),
        ("catalog:", "catalog"),
        ("link:../x", "link"),
        ("jsr:@luca/flag", "jsr"),
    ] {
        let err = PackageSpec::from_dependency("dep", value).refusal(value);
        assert_eq!(
            err.kind,
            SpecErrorKind::UnsupportedProtocol(scheme.to_string()),
            "{value}"
        );
        // The message-names-the-form guarantee has to hold on THIS path too —
        // a manifest entry is how these will really arrive.
        let message = err.to_string();
        assert!(message.contains(scheme), "{value}: {message}");
        assert!(message.contains(value), "{value}: {message}");
    }
}

/// Garbage that *resembles* a supported form. Each of these could plausibly
/// be mis-parsed as a dist-tag, a name or a URL, and none of them may be.
#[test]
fn input_resembling_a_supported_form_is_refused_rather_than_mis_parsed() {
    // Looks like a caret range, is not one — must NOT become Tag("^1.2.3.4").
    // (`~>1.2.3` and `1.2.3 &&` are absent: node-semver accepts both. See
    // RANGES and `node_semver_leniency_is_recorded_not_papered_over`.)
    let mut survived = Vec::new();
    for bad_range in ["^1.2.3.4", ">=", "^^1.0.0", "1.2.3-", "1.2.3..4", "=="] {
        match PackageSpec::from_dependency("p", bad_range) {
            Ok(spec) => survived.push(format!("  {bad_range:?} parsed as {spec:?}")),
            Err(e) => assert!(
                matches!(e.kind, SpecErrorKind::InvalidRange { .. }),
                "{bad_range:?} produced {:?}",
                e.kind
            ),
        }
    }
    assert!(
        survived.is_empty(),
        "these should have been refused:\n{}",
        survived.join("\n")
    );

    // Looks like a tarball URL, names no tarball.
    let err = PackageSpec::parse("https://host/archive.zip").unwrap_err();
    assert!(
        matches!(err.kind, SpecErrorKind::NotATarballUrl(_)),
        "{err:?}"
    );
    assert!(err.to_string().contains(".tgz"), "{err}");

    // Looks like a git shorthand, has no repo.
    assert!(matches!(
        PackageSpec::parse("github:owner").unwrap_err().kind,
        SpecErrorKind::InvalidGitShorthand(_)
    ));

    // Looks like an alias, names nothing.
    assert!(matches!(
        PackageSpec::parse("foo@npm:").unwrap_err().kind,
        SpecErrorKind::EmptyAliasTarget
    ));

    // Looks like a path, is empty.
    assert!(matches!(
        PackageSpec::parse("file:").unwrap_err().kind,
        SpecErrorKind::EmptyPath
    ));

    // Trailing `@` with nothing after it.
    assert!(matches!(
        PackageSpec::parse("react@").unwrap_err().kind,
        SpecErrorKind::EmptyVersionSpec
    ));

    // Trailing `#` with no commit-ish.
    assert!(matches!(
        PackageSpec::parse("github:o/r#").unwrap_err().kind,
        SpecErrorKind::EmptyCommittish
    ));
}

#[test]
fn an_invalid_package_name_is_refused_by_name() {
    // `..` and `.` are absent on purpose: they are legal *directory* specs,
    // not malformed names.
    for bad in [
        "@scope", "@/name", "@scope/", "a/b/c/d", "with space", "pct%20", ".hidden",
    ] {
        let err = PackageSpec::parse(bad).refusal(bad);
        assert!(
            matches!(
                err.kind,
                SpecErrorKind::InvalidPackageName(_) | SpecErrorKind::InvalidGitShorthand(_)
            ),
            "{bad:?} produced {:?}",
            err.kind
        );
        assert!(err.to_string().contains(bad), "{bad:?}: {err}");
    }
    assert_eq!(PackageSpec::parse("").unwrap_err().kind, SpecErrorKind::Empty);
    assert_eq!(PackageSpec::parse("   ").unwrap_err().kind, SpecErrorKind::Empty);
}

/// The cache's filename rule and the grammar's name rule are the same rule —
/// R773-F2 folded the duplicate in `cache.rs` into `spec::validate_package_name`.
#[test]
fn the_cache_and_the_grammar_agree_on_what_a_legal_name_is() {
    for name in ["react", "@babel/core", "@jsr/luca__flag", "_legacy"] {
        assert!(rnpm::validate_package_name(name).is_ok(), "{name}");
        assert!(rnpm::cache_file_name(name).is_ok(), "{name}");
    }
    for name in ["../etc", "@scope", "with space", "pct%20"] {
        assert!(rnpm::validate_package_name(name).is_err(), "{name}");
        assert!(rnpm::cache_file_name(name).is_err(), "{name}");
    }
}

/// `node-semver` 2.2.0 is lenient in two places this grammar deliberately does
/// **not** tighten: it accepts RubyGems' `~>` as `~`, and it ignores trailing
/// garbage after an otherwise-valid comparator.
///
/// Tightening either one means writing a second range grammar in front of the
/// one W318 §8 pinned, and a second grammar is exactly how our range semantics
/// would start diverging from pnpm's on the same input — the failure R773-F7's
/// conformance corpus catches late and expensively. So the leniency is pinned
/// here as a fact instead: if a node-semver bump changes it, this test says so
/// by name rather than a resolve quietly changing shape.
#[test]
fn node_semver_leniency_is_recorded_not_papered_over() {
    assert!(satisfies("~>1.2.3", "1.2.9"));
    assert!(!satisfies("~>1.2.3", "1.3.0"));

    // `~` and `~>` both produce `>=1.2.3 <1.3.0-0` — note the `-0`, which
    // excludes prereleases of the next minor as well as its release. Asserted
    // because "reads it as plain `~`" is a claim about the exact bounds, and
    // the `-0` half of them is where an implementation would quietly differ.
    for tilde in ["~1.2.3", "~>1.2.3"] {
        assert!(!satisfies(tilde, "1.3.0-0"), "{tilde} vs 1.3.0-0");
        assert!(!satisfies(tilde, "1.3.0-alpha"), "{tilde} vs 1.3.0-alpha");
        assert!(satisfies(tilde, "1.2.3"), "{tilde} vs 1.2.3");
    }

    // Trailing junk after a valid comparator is dropped, not refused.
    for (lenient, equivalent) in [("1.2.3 &&", "1.2.3"), ("1.2.3 ||", "1.2.3")] {
        assert!(
            satisfies(lenient, equivalent),
            "{lenient:?} no longer parses like {equivalent:?}"
        );
    }
}

/// Makes the refusal assertions above read as one line each while still
/// failing loudly on the case they exist to catch: a spec that *parsed* when
/// it should not have.
trait Refusal {
    fn refusal(self, input: &str) -> SpecError;
}

impl Refusal for Result<PackageSpec, SpecError> {
    fn refusal(self, input: &str) -> SpecError {
        match self {
            Ok(spec) => panic!("{input:?} was mis-parsed as {spec:?} instead of being refused"),
            Err(e) => e,
        }
    }
}
