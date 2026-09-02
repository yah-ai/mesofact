//! Package specifiers: `react@^18`, `npm:lodash@4`, `file:../pkg`,
//! `github:owner/repo#semver:^1`, `https://host/pkg.tgz`.
//!
//! # Parse-only
//!
//! [`PackageSpec`] is a pure value. Nothing here fetches, and nothing here
//! *resolves*: a dist-tag stays [`VersionSpec::Tag`] rather than becoming a
//! version, because turning `latest` into `18.3.1` needs a packument and
//! belongs to R773-F3's walk. A `file:`, git or tarball spec is likewise
//! parsed and handed on — this module never touches a filesystem or a network.
//!
//! # No registry knowledge
//!
//! There is no `npm`/`jsr` branch in this file and there must not be one.
//! Which registries exist, what base URL each has and how each spells a
//! package name is the `jsget` facade's job (W318 §9). A protocol this
//! grammar does not know — `jsr:`, `workspace:`, `catalog:`, `link:`,
//! `patch:` — is refused generically by [`SpecErrorKind::UnsupportedProtocol`],
//! which names the protocol it found without this module having to know what
//! any of them mean.
//!
//! # Ranges
//!
//! Range parsing and satisfaction are `node-semver` 2.2.0's, not ours and not
//! the `semver` crate's — npm's grammar is not Cargo's (W318 §8). Notably its
//! prerelease rule is npm's: a prerelease version satisfies a comparator set
//! only when some bound with the *same* `major.minor.patch` is itself a
//! prerelease, so `^1.2.3` does not match `1.2.4-beta` but `^1.2.3-alpha`
//! does match `1.2.3-beta` (`node-semver-2.2.0/src/range.rs:90-122`).
//!
//! # Attribution
//!
//! The grammar below was read off `oro-package-spec` 0.3.34 (Apache-2.0,
//! orogene) — `src/parsers/{package,npm,alias,path,git}.rs` and
//! `src/gitinfo.rs` — and the `///` grammar lines on each function here are
//! that crate's, quoted. The *implementation* is not a port: oro is built on
//! `nom` combinators and this is hand-written, because the grammar is small
//! and a reader chasing a spec bug should not have to read `nom` first. Two
//! deliberate divergences, both noted at their sites: direct tarball URLs
//! (which oro's grammar does not cover) are supported, and oro's separate
//! `GitInfo::Ssh` variant is folded into [`GitSource::Url`]. See `NOTICE`.

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

pub use node_semver::{Range, Version};

/// Which version of a package is wanted. Never resolved here — see the module
/// doc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionSpec {
    /// An exact version: `1.2.3`.
    Version(Version),
    /// A range: `^1.2.3`, `>=1 <2`, `1.x`, `1.2.3 - 2.0.0`, `a || b`.
    Range(Range),
    /// A dist-tag: `latest`, `next`. Data, not a lookup.
    Tag(String),
}

impl VersionSpec {
    /// Whether `version` is acceptable. An exact spec compares equal; a tag
    /// cannot answer without a packument, so it answers `false` — callers that
    /// hold one resolve the tag through [`crate::Packument::dist_tag`] first.
    pub fn matches(&self, version: &Version) -> bool {
        match self {
            VersionSpec::Version(v) => v == version,
            VersionSpec::Range(r) => r.satisfies(version),
            VersionSpec::Tag(_) => false,
        }
    }
}

/// Which forge a `github:owner/repo` shorthand names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitHost {
    GitHub,
    Gist,
    GitLab,
    Bitbucket,
}

impl GitHost {
    pub fn as_str(&self) -> &'static str {
        match self {
            GitHost::GitHub => "github",
            GitHost::Gist => "gist",
            GitHost::GitLab => "gitlab",
            GitHost::Bitbucket => "bitbucket",
        }
    }
}

impl fmt::Display for GitHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where a git dependency's repository is.
///
/// Diverges from `oro-package-spec`, which splits `Url` and `Ssh`: nothing in
/// this crate fetches, so the two differ only in a string we would hand
/// onward verbatim either way. Fold them, and let whatever grows a git fetcher
/// re-split if it ever needs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitSource {
    Hosted { host: GitHost, owner: String, repo: String },
    /// A full URL, verbatim and with any `git+` prefix retained:
    /// `git+ssh://git@host/o/r`, `git://host/o/r`.
    Url(String),
}

/// A git dependency: where, at which commit-ish, optionally narrowed by a
/// `#semver:` range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSpec {
    pub source: GitSource,
    /// The `#`-suffix: a branch, tag or sha. `None` means the default branch.
    pub committish: Option<String>,
    /// `#semver:<range>`, which selects a *tag* in the repository rather than
    /// a commit-ish. Mutually exclusive with `committish` by construction.
    pub semver: Option<Range>,
}

/// A parsed package specifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageSpec {
    /// `[@scope/]name[@version-spec]`.
    Npm {
        scope: Option<String>,
        /// The full name including the scope: `@babel/core`.
        name: String,
        /// `None` where the spec named no version at all, which npm reads as
        /// `*`. Kept distinct from `Some(Range(*))` so a lock writer can
        /// reproduce what the manifest actually said.
        requested: Option<VersionSpec>,
    },
    /// Installed under `name`, but actually `spec` — `foo@npm:bar@^1`, and
    /// every non-npm form reached through a manifest entry (`"foo":
    /// "file:../foo"` installs a directory under the name `foo`).
    Alias { name: String, spec: Box<PackageSpec> },
    /// `file:../pkg`, `./pkg`, `/abs/pkg`.
    Dir { path: PathBuf },
    Git(GitSpec),
    /// A direct tarball URL. `oro-package-spec`'s grammar has no such form;
    /// npm accepts one and R773-F1's ticket asks for it, so it is here.
    Tarball { url: String },
}

impl PackageSpec {
    /// The name a package installs under, when the spec names one. `None` for
    /// a bare path/git/tarball spec, whose install name comes from the
    /// manifest key or the fetched `package.json`.
    pub fn name(&self) -> Option<&str> {
        match self {
            PackageSpec::Npm { name, .. } | PackageSpec::Alias { name, .. } => Some(name),
            PackageSpec::Dir { .. } | PackageSpec::Git(_) | PackageSpec::Tarball { .. } => None,
        }
    }

    pub fn is_alias(&self) -> bool {
        matches!(self, PackageSpec::Alias { .. })
    }

    /// Follow `Alias` links to the spec that actually says where to get the
    /// package.
    pub fn target(&self) -> &PackageSpec {
        match self {
            PackageSpec::Alias { spec, .. } => spec.target(),
            other => other,
        }
    }

    /// `[ "npm:" ] npm-spec | [ "file:" ] path | git-spec | tarball-url | alias`
    ///
    /// The CLI-shaped entry point: one string carrying both a name and a
    /// source. A manifest entry, where the two arrive separately, is
    /// [`Self::from_dependency`].
    pub fn parse(input: &str) -> Result<Self, SpecError> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(SpecError::new(input, SpecErrorKind::Empty));
        }

        // A source-shaped spec carries no name of its own, so it is recognised
        // before anything tries to read one out of it.
        if let Some(spec) = parse_source(trimmed)? {
            return Ok(spec);
        }

        let (name, rest) = split_name(trimmed)?;
        validate_package_name(name).map_err(|kind| SpecError::new(input, kind))?;

        match rest {
            // `foo@npm:bar@^1`, `foo@file:../bar`, `foo@github:o/r`: the part
            // after the `@` is itself a source, so this is an alias.
            Some(rest) => match parse_source(rest)? {
                Some(spec) => Ok(PackageSpec::Alias {
                    name: name.to_string(),
                    spec: Box::new(spec),
                }),
                None => Ok(npm_spec(name, Some(parse_version_spec(rest)?))),
            },
            None => Ok(npm_spec(name, None)),
        }
    }

    /// A `package.json` dependency entry, where the name is the map key and
    /// `spec` is its value: `("react", "^18.0.0")`, `("foo", "npm:bar@^1")`,
    /// `("x", "file:../x")`.
    ///
    /// This is the shape R773-F3's walk actually consumes, since a packument's
    /// `dependencies` map is exactly this pair. A value that is a *source*
    /// rather than a version yields an [`PackageSpec::Alias`] under the map
    /// key, because that is what the entry means: install this thing, under
    /// this name.
    pub fn from_dependency(name: &str, spec: &str) -> Result<Self, SpecError> {
        validate_package_name(name).map_err(|kind| SpecError::new(name, kind))?;
        let value = spec.trim();

        if let Some(source) = parse_source(value)? {
            return Ok(PackageSpec::Alias {
                name: name.to_string(),
                spec: Box::new(source),
            });
        }
        if value.is_empty() {
            return Ok(npm_spec(name, None));
        }
        Ok(npm_spec(name, Some(parse_version_spec(value)?)))
    }
}

/// Build an `Npm` variant, splitting the scope back out of a validated name.
fn npm_spec(name: &str, requested: Option<VersionSpec>) -> PackageSpec {
    let scope = name
        .strip_prefix('@')
        .and_then(|rest| rest.split('/').next())
        .map(str::to_string);
    PackageSpec::Npm { scope, name: name.to_string(), requested }
}

/// The forms that carry a source rather than a version: `npm:`, `file:`, a
/// path, git, a tarball URL. `Ok(None)` means "not one of these", which is the
/// caller's cue to read a name out of the input instead.
///
/// An unknown `<scheme>:` prefix is refused *here*, so `workspace:^`,
/// `catalog:`, `link:../x` and `jsr:@a/b` all fail by name rather than being
/// mistaken for a package called `workspace` at version `^`.
fn parse_source(input: &str) -> Result<Option<PackageSpec>, SpecError> {
    if let Some(rest) = strip_prefix_ci(input, "npm:") {
        if rest.trim().is_empty() {
            return Err(SpecError::new(input, SpecErrorKind::EmptyAliasTarget));
        }
        // `npm:` may only introduce an npm spec, never another protocol.
        let (name, version) = split_name(rest)?;
        validate_package_name(name).map_err(|kind| SpecError::new(input, kind))?;
        let requested = version.map(parse_version_spec).transpose()?;
        return Ok(Some(npm_spec(name, requested)));
    }
    if let Some(rest) = strip_prefix_ci(input, "file:") {
        return Ok(Some(dir_spec(input, rest)?));
    }
    // Paths are tested BEFORE git, because the bare `owner/repo` GitHub
    // shorthand would otherwise claim `../pkg` (owner `..`, repo `pkg`). This
    // is oro's own `alt` ordering in `parsers/package.rs`: path, then git.
    if looks_like_path(input) {
        return Ok(Some(dir_spec(input, input)?));
    }
    if let Some(git) = parse_git(input)? {
        return Ok(Some(PackageSpec::Git(git)));
    }
    if let Some(url) = parse_tarball(input)? {
        return Ok(Some(PackageSpec::Tarball { url }));
    }
    if let Some(scheme) = unknown_protocol(input) {
        return Err(SpecError::new(
            input,
            SpecErrorKind::UnsupportedProtocol(scheme.to_string()),
        ));
    }
    Ok(None)
}

fn dir_spec(input: &str, path: &str) -> Result<PackageSpec, SpecError> {
    if path.trim().is_empty() {
        return Err(SpecError::new(input, SpecErrorKind::EmptyPath));
    }
    Ok(PackageSpec::Dir { path: PathBuf::from(path) })
}

/// `relative-path := [ '.' ] '.' [path-sep] .*` /
/// `absolute-path := [ alpha ':' ] path-sep+ .*` (oro `parsers/path.rs`)
///
/// A leading `~` is NOT a path here, however much it looks like a home
/// directory: `~1.2.3` is npm's tilde *range*, and it has to win. npm expands
/// no home directories in a spec and neither does oro's grammar — an
/// unprefixed path must open with `.`, `/` or a drive letter.
fn looks_like_path(input: &str) -> bool {
    input.starts_with("./")
        || input.starts_with("../")
        || input == "."
        || input == ".."
        || input.starts_with('/')
        || input.starts_with(".\\")
        || input.starts_with("..\\")
        // A one-letter scheme is a Windows drive letter, not a protocol.
        || windows_drive(input)
}

fn windows_drive(input: &str) -> bool {
    let mut chars = input.chars();
    matches!(
        (chars.next(), chars.next(), chars.next()),
        (Some(c), Some(':'), Some('/' | '\\')) if c.is_ascii_alphabetic()
    )
}

/// `git-spec := git-shorthand | git-url` — oro's `parsers/git.rs`, minus its
/// separate SCP branch (see [`GitSource`]).
fn parse_git(input: &str) -> Result<Option<GitSpec>, SpecError> {
    const HOSTS: [(&str, GitHost); 4] = [
        ("github:", GitHost::GitHub),
        ("gist:", GitHost::Gist),
        ("gitlab:", GitHost::GitLab),
        ("bitbucket:", GitHost::Bitbucket),
    ];

    for (prefix, host) in HOSTS {
        if let Some(rest) = strip_prefix_ci(input, prefix) {
            let (path, committish, semver) = split_committish(input, rest)?;
            let (owner, repo) = split_owner_repo(input, path)?;
            return Ok(Some(GitSpec {
                source: GitSource::Hosted { host, owner, repo },
                committish,
                semver,
            }));
        }
    }

    let is_git_url = strip_prefix_ci(input, "git+").is_some()
        || strip_prefix_ci(input, "git://").is_some()
        || strip_prefix_ci(input, "ssh://").is_some();
    if is_git_url {
        let (url, committish, semver) = split_committish(input, input)?;
        if url.trim().is_empty() {
            return Err(SpecError::new(input, SpecErrorKind::EmptyGitUrl));
        }
        return Ok(Some(GitSpec {
            source: GitSource::Url(url.to_string()),
            committish,
            semver,
        }));
    }

    // `owner/repo[#committish]` with no protocol at all is npm's GitHub
    // shorthand. `@scope/name` is not — a leading `@` makes it an npm name,
    // which is why this test runs after the npm forms have had their chance
    // at anything scoped.
    if !input.starts_with('@') && input.contains('/') && !input.contains(':') {
        let (path, committish, semver) = split_committish(input, input)?;
        // A version spec can contain neither `/` nor a bare word pair, so
        // anything reaching here with exactly one `/` and no `@` version part
        // is the shorthand.
        if !path.contains('@') {
            let (owner, repo) = split_owner_repo(input, path)?;
            return Ok(Some(GitSpec {
                source: GitSource::Hosted { host: GitHost::GitHub, owner, repo },
                committish,
                semver,
            }));
        }
    }

    Ok(None)
}

/// Split a trailing `#committish` or `#semver:<range>` off `value`.
fn split_committish<'a>(
    input: &str,
    value: &'a str,
) -> Result<(&'a str, Option<String>, Option<Range>), SpecError> {
    let Some((head, tail)) = value.split_once('#') else {
        return Ok((value, None, None));
    };
    if let Some(range) = strip_prefix_ci(tail, "semver:") {
        let parsed = Range::parse(range).map_err(|e| {
            SpecError::new(
                input,
                SpecErrorKind::InvalidRange { spec: range.to_string(), reason: e.to_string() },
            )
        })?;
        return Ok((head, None, Some(parsed)));
    }
    if tail.is_empty() {
        return Err(SpecError::new(input, SpecErrorKind::EmptyCommittish));
    }
    Ok((head, Some(tail.to_string()), None))
}

fn split_owner_repo(input: &str, path: &str) -> Result<(String, String), SpecError> {
    let Some((owner, repo)) = path.split_once('/') else {
        return Err(SpecError::new(
            input,
            SpecErrorKind::InvalidGitShorthand(path.to_string()),
        ));
    };
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return Err(SpecError::new(
            input,
            SpecErrorKind::InvalidGitShorthand(path.to_string()),
        ));
    }
    Ok((owner.to_string(), repo.to_string()))
}

/// A bare `http(s)://` URL naming a tarball.
///
/// The extension is *required*. npm will take any http(s) URL here, but this
/// grammar's contract is that an unsupported form is refused rather than
/// mis-parsed, and a URL that is not a tarball is not something this resolver
/// can do anything with.
fn parse_tarball(input: &str) -> Result<Option<String>, SpecError> {
    let is_http =
        strip_prefix_ci(input, "http://").is_some() || strip_prefix_ci(input, "https://").is_some();
    if !is_http {
        return Ok(None);
    }
    let path = input.split(['?', '#']).next().unwrap_or(input);
    let lower = path.to_ascii_lowercase();
    if [".tgz", ".tar.gz", ".tar"].iter().any(|ext| lower.ends_with(ext)) {
        return Ok(Some(input.to_string()));
    }
    Err(SpecError::new(
        input,
        SpecErrorKind::NotATarballUrl(input.to_string()),
    ))
}

/// The scheme of a `<scheme>:` prefix this grammar does not know, if the input
/// has one. Schemes are at least two characters, so a Windows drive letter is
/// never mistaken for one.
fn unknown_protocol(input: &str) -> Option<&str> {
    let (scheme, _) = input.split_once(':')?;
    if scheme.len() < 2 {
        return None;
    }
    let mut chars = scheme.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_alphabetic());
    let rest_ok = chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    (first_ok && rest_ok).then_some(scheme)
}

/// `npm-spec := [ '@' not('/')+ '/' ] not('@/')+ [ '@' version-req ]`
/// (oro `parsers/npm.rs`) — split into the name and the part after its `@`.
fn split_name(input: &str) -> Result<(&str, Option<&str>), SpecError> {
    // A leading `@` opens a scope, so the version separator is searched for
    // only after the scope's `/`.
    let search_from = if input.starts_with('@') {
        match input.find('/') {
            Some(slash) => slash + 1,
            None => {
                return Err(SpecError::new(
                    input,
                    SpecErrorKind::InvalidPackageName(input.to_string()),
                ))
            }
        }
    } else {
        0
    };

    match input[search_from..].find('@') {
        Some(at) => {
            let at = search_from + at;
            let rest = &input[at + 1..];
            if rest.is_empty() {
                return Err(SpecError::new(input, SpecErrorKind::EmptyVersionSpec));
            }
            Ok((&input[..at], Some(rest)))
        }
        None => Ok((input, None)),
    }
}

/// `version-req := semver-version | semver-range | dist-tag` (oro
/// `parsers/npm.rs`), in that order — an exact version is recognised as such
/// rather than as a one-version range.
fn parse_version_spec(input: &str) -> Result<VersionSpec, SpecError> {
    let value = input.trim();
    if value.is_empty() {
        return Err(SpecError::new(input, SpecErrorKind::EmptyVersionSpec));
    }
    if let Ok(version) = Version::parse(value) {
        return Ok(VersionSpec::Version(version));
    }
    let range_error = match Range::parse(value) {
        Ok(range) => return Ok(VersionSpec::Range(range)),
        Err(e) => e.to_string(),
    };
    // Only a plausible *tag* falls through. Something that opened like a range
    // and failed to parse must not become `Tag("^1.2.3.4")` — that is the
    // silent mis-parse this grammar exists to refuse.
    if looks_like_tag(value) {
        return Ok(VersionSpec::Tag(value.to_string()));
    }
    Err(SpecError::new(
        input,
        SpecErrorKind::InvalidRange { spec: value.to_string(), reason: range_error },
    ))
}

fn looks_like_tag(value: &str) -> bool {
    let first = value.chars().next().unwrap_or_default();
    if !(first.is_ascii_alphabetic() || first == '_' || first == '-') {
        return false;
    }
    // `v1.2.3` and `x`/`X` ranges open like a version, not a tag.
    if matches!(first, 'v' | 'V') && value[1..].starts_with(|c: char| c.is_ascii_digit()) {
        return false;
    }
    if matches!(value, "x" | "X" | "*") {
        return false;
    }
    value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn strip_prefix_ci<'a>(input: &'a str, prefix: &str) -> Option<&'a str> {
    input
        .get(..prefix.len())
        .filter(|head| head.eq_ignore_ascii_case(prefix))
        .map(|_| &input[prefix.len()..])
}

/// Validate an npm package name.
///
/// Shared with [`crate::cache`], which turns a validated name into exactly one
/// path component — so this is also the check that stops a manifest-supplied
/// name from escaping a cache root. At most 214 bytes, no control or
/// whitespace characters, no leading `.` on a component, and `/` only as the
/// single separator after a leading `@` scope. `%` and `\` are refused
/// outright: the first would make the cache's `%2f` mangling ambiguous, the
/// second is a path separator on a platform we do not support but might.
///
/// A leading `_` is deliberately allowed — npm refuses it for new publishes,
/// but names predating that rule are still installable, and refusing one would
/// fail a resolve rather than close a hole.
pub fn validate_package_name(name: &str) -> Result<(), SpecErrorKind> {
    let invalid = || SpecErrorKind::InvalidPackageName(name.to_string());

    if name.is_empty() || name.len() > 214 {
        return Err(invalid());
    }
    for c in name.chars() {
        if c.is_control()
            || c.is_whitespace()
            || matches!(c, '%' | '\\' | ':' | '?' | '#' | '*' | '"' | '<' | '>' | '|')
        {
            return Err(invalid());
        }
    }

    let components: Vec<&str> = if let Some(rest) = name.strip_prefix('@') {
        let mut parts = rest.splitn(2, '/');
        let scope = parts.next().unwrap_or_default();
        let Some(unscoped) = parts.next() else {
            return Err(invalid());
        };
        vec![scope, unscoped]
    } else {
        vec![name]
    };

    for part in components {
        if part.is_empty() || part.contains('/') || part.starts_with('.') {
            return Err(invalid());
        }
    }
    Ok(())
}

/// Why a spec could not be parsed. Every variant names the offending form —
/// the ticket's second criterion is that an unsupported form is refused with a
/// message naming it, never silently mis-parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecErrorKind {
    Empty,
    /// A `<scheme>:` this grammar does not implement — `workspace:`,
    /// `catalog:`, `link:`, `patch:`, `jsr:`.
    UnsupportedProtocol(String),
    InvalidPackageName(String),
    /// Opened like a range and is not one. Carries `node-semver`'s own reason.
    InvalidRange { spec: String, reason: String },
    EmptyVersionSpec,
    EmptyAliasTarget,
    EmptyPath,
    EmptyGitUrl,
    EmptyCommittish,
    InvalidGitShorthand(String),
    /// An `http(s)://` URL that does not name a tarball.
    NotATarballUrl(String),
}

impl fmt::Display for SpecErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpecErrorKind::Empty => write!(f, "the spec is empty"),
            SpecErrorKind::UnsupportedProtocol(scheme) => write!(
                f,
                "the `{scheme}:` protocol is not supported; this resolver understands \
                 `npm:`, `file:`, `git+`/`git:`/`ssh:`, `github:`/`gitlab:`/`gist:`/`bitbucket:` \
                 and direct http(s) tarball URLs"
            ),
            SpecErrorKind::InvalidPackageName(name) => {
                write!(f, "`{name}` is not a valid npm package name")
            }
            SpecErrorKind::InvalidRange { spec, reason } => {
                write!(f, "`{spec}` is not a valid npm version range: {reason}")
            }
            SpecErrorKind::EmptyVersionSpec => write!(f, "the `@` is not followed by a version"),
            SpecErrorKind::EmptyAliasTarget => write!(f, "`npm:` is not followed by a package"),
            SpecErrorKind::EmptyPath => write!(f, "`file:` is not followed by a path"),
            SpecErrorKind::EmptyGitUrl => write!(f, "the git URL is empty"),
            SpecErrorKind::EmptyCommittish => write!(f, "the `#` is not followed by a commit-ish"),
            SpecErrorKind::InvalidGitShorthand(path) => write!(
                f,
                "`{path}` is not a `owner/repo` git shorthand"
            ),
            SpecErrorKind::NotATarballUrl(url) => write!(
                f,
                "`{url}` is an http(s) URL but does not name a tarball \
                 (expected a `.tgz`, `.tar.gz` or `.tar` path)"
            ),
        }
    }
}

/// A refused spec, carrying the input it refused so an error can be reported
/// against what the manifest actually said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecError {
    pub input: String,
    pub kind: SpecErrorKind,
}

impl SpecError {
    fn new(input: &str, kind: SpecErrorKind) -> Self {
        Self { input: input.to_string(), kind }
    }
}

impl fmt::Display for SpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot parse package spec `{}`: {}", self.input, self.kind)
    }
}

impl std::error::Error for SpecError {}

impl FromStr for PackageSpec {
    type Err = SpecError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        PackageSpec::parse(s)
    }
}

impl fmt::Display for VersionSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VersionSpec::Version(v) => write!(f, "{v}"),
            VersionSpec::Range(r) => write!(f, "{r}"),
            VersionSpec::Tag(t) => write!(f, "{t}"),
        }
    }
}

impl fmt::Display for PackageSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PackageSpec::Npm { name, requested, .. } => match requested {
                Some(req) => write!(f, "{name}@{req}"),
                None => write!(f, "{name}"),
            },
            PackageSpec::Alias { name, spec } => {
                write!(f, "{name}@")?;
                if matches!(**spec, PackageSpec::Npm { .. }) {
                    write!(f, "npm:")?;
                }
                write!(f, "{spec}")
            }
            PackageSpec::Dir { path } => write!(f, "file:{}", path.display()),
            PackageSpec::Tarball { url } => write!(f, "{url}"),
            PackageSpec::Git(git) => {
                match &git.source {
                    GitSource::Hosted { host, owner, repo } => write!(f, "{host}:{owner}/{repo}")?,
                    GitSource::Url(url) => write!(f, "{url}")?,
                }
                if let Some(range) = &git.semver {
                    write!(f, "#semver:{range}")?;
                } else if let Some(committish) = &git.committish {
                    write!(f, "#{committish}")?;
                }
                Ok(())
            }
        }
    }
}
