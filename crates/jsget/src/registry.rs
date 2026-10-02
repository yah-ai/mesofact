//! Which registries exist, and how each one spells a package name.
//!
//! That is the whole of registry plurality as W318 §9 found it, and it is
//! deliberately small. JSR needs **no second resolver**:
//! `https://npm.jsr.io/@jsr/<scope>__<name>` serves a complete npm packument —
//! `dist-tags`, a `versions` map, and per version a `dist.tarball` and a
//! `sha512-` `integrity` (verified live against `@jsr/luca__flag`, W318 §9). So
//! a JSR package differs from an npm package by a base URL and a name, and
//! nothing else in this crate or in `rnpm` branches on which one it is.
//!
//! **Tarball layout is not here on purpose.** A packument's `dist.tarball` is
//! an absolute URL and is authoritative, so no code path constructs one — which
//! is why JSR's rather different tarball paths (`/~/11/@jsr/luca__flag/…`) cost
//! zero lines. The only place in this workspace that builds a tarball URL is
//! `mesofact-build`'s `install.rs`, which does it from a *lockfile* entry that
//! carries no URL.

use anyhow::{bail, Result};
use rnpm::RegistryEndpoint;

pub const NPM_REGISTRY_URL: &str = "https://registry.npmjs.org";
/// JSR's npm-compatibility endpoint — *not* `https://jsr.io`, whose native
/// `meta.json` API is thinner and carries no dependency information.
pub const JSR_NPM_REGISTRY_URL: &str = "https://npm.jsr.io";
/// JSR packages are served under one npm scope, with the JSR scope folded into
/// the unscoped part.
pub const JSR_SCOPE: &str = "@jsr/";

/// Which ecosystem a registry belongs to. This exists to select a *name
/// mangling*, and for no other reason — see [`Registry::registry_name`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ecosystem {
    /// Names are used verbatim.
    Npm,
    /// `@scope/name` → `@jsr/scope__name`.
    Jsr,
}

/// One registry: an ecosystem's naming rules bound to an endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry {
    ecosystem: Ecosystem,
    endpoint: RegistryEndpoint,
}

impl Registry {
    pub fn npm() -> Self {
        Self {
            ecosystem: Ecosystem::Npm,
            endpoint: RegistryEndpoint::new("npm", NPM_REGISTRY_URL),
        }
    }

    /// JSR, through its npm-compatibility endpoint.
    pub fn jsr() -> Self {
        Self {
            ecosystem: Ecosystem::Jsr,
            endpoint: RegistryEndpoint::new("jsr", JSR_NPM_REGISTRY_URL),
        }
    }

    /// A private or mirrored npm-protocol registry. `id` namespaces the cache,
    /// so it must not collide with `npm` or `jsr` unless it really is a mirror
    /// of one.
    pub fn npm_compatible(id: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            ecosystem: Ecosystem::Npm,
            endpoint: RegistryEndpoint::new(id, base_url),
        }
    }

    /// Point this registry at a different base URL — a mirror, a proxy, or a
    /// test server. The mangling is unaffected, which is the point.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.endpoint.base_url = base_url.into();
        self
    }

    /// Verbatim `Authorization` header value. Nothing here reads a `.npmrc`
    /// or an environment variable; a caller that wants token discovery owns
    /// that policy.
    pub fn with_authorization(mut self, value: impl Into<String>) -> Self {
        self.endpoint.authorization = Some(value.into());
        self
    }

    pub fn ecosystem(&self) -> Ecosystem {
        self.ecosystem
    }

    pub fn endpoint(&self) -> &RegistryEndpoint {
        &self.endpoint
    }

    pub fn id(&self) -> &str {
        &self.endpoint.id
    }

    /// How this registry spells `name`.
    ///
    /// The npm case is the identity. The JSR case folds the scope:
    /// `@luca/flag` → `@jsr/luca__flag`. Already-mangled names pass through, so
    /// a lockfile that records the registry spelling (which is what ends up on
    /// disk under `node_modules/@jsr/luca__flag`) can be fed straight back in.
    pub fn registry_name(&self, name: &str) -> Result<String> {
        match self.ecosystem {
            Ecosystem::Npm => Ok(name.to_string()),
            Ecosystem::Jsr => {
                if name.starts_with(JSR_SCOPE) {
                    return Ok(name.to_string());
                }
                let Some(rest) = name.strip_prefix('@') else {
                    bail!("jsr package names are scoped: expected @scope/name, got {name:?}");
                };
                let mut parts = rest.splitn(2, '/');
                let scope = parts.next().unwrap_or_default();
                let Some(unscoped) = parts.next() else {
                    bail!("jsr package names are scoped: expected @scope/name, got {name:?}");
                };
                if scope.is_empty() || unscoped.is_empty() || unscoped.contains('/') {
                    bail!("jsr package names are scoped: expected @scope/name, got {name:?}");
                }
                Ok(format!("{JSR_SCOPE}{scope}__{unscoped}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn npm_spells_a_name_exactly_as_it_was_given() {
        let npm = Registry::npm();
        for name in ["react", "@babel/core", "@jsr/luca__flag"] {
            assert_eq!(npm.registry_name(name).unwrap(), name);
        }
    }

    #[test]
    fn jsr_folds_the_scope_into_one_npm_scope() {
        let jsr = Registry::jsr();
        assert_eq!(jsr.registry_name("@luca/flag").unwrap(), "@jsr/luca__flag");
        assert_eq!(jsr.registry_name("@std/path").unwrap(), "@jsr/std__path");
    }

    #[test]
    fn an_already_mangled_jsr_name_passes_through_unchanged() {
        assert_eq!(
            Registry::jsr().registry_name("@jsr/luca__flag").unwrap(),
            "@jsr/luca__flag"
        );
    }

    #[test]
    fn an_unscoped_name_on_jsr_is_refused_with_the_form_it_wanted() {
        for bad in ["flag", "@flag", "@/flag", "@luca/", "@a/b/c"] {
            let err = Registry::jsr().registry_name(bad).unwrap_err().to_string();
            assert!(err.contains("@scope/name"), "{bad:?}: {err}");
            assert!(err.contains(bad), "{bad:?}: {err}");
        }
    }

    #[test]
    fn the_two_registries_differ_only_by_base_url_and_id() {
        let npm = Registry::npm();
        let jsr = Registry::jsr();
        assert_eq!(npm.endpoint().base_url, "https://registry.npmjs.org");
        assert_eq!(jsr.endpoint().base_url, "https://npm.jsr.io");
        assert_eq!(npm.id(), "npm");
        assert_eq!(jsr.id(), "jsr");
        assert_eq!(npm.endpoint().authorization, None);
        assert_eq!(jsr.endpoint().authorization, None);
    }

    #[test]
    fn a_mirror_keeps_the_mangling_of_the_registry_it_mirrors() {
        let mirror = Registry::jsr().with_base_url("https://mirror.test/jsr");
        assert_eq!(mirror.registry_name("@luca/flag").unwrap(), "@jsr/luca__flag");
        assert_eq!(
            mirror.endpoint().packument_url("@jsr/luca__flag"),
            "https://mirror.test/jsr/@jsr/luca__flag"
        );
    }
}
