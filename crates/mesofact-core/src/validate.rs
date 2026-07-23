//! Manifest validator — semantic rules over a parsed [`Manifest`]. Mirrors
//! `packages/mesofact-runtime/src/validate.ts`. Used by the proxy on boot
//! (refuse a malformed manifest, keep the old one live) and by the build to
//! reject forbidden shapes before HTML hits R2.
//!
//! Structural validation comes "for free" from serde — call [`validate`] only
//! after `serde_json::from_str::<Manifest>(...)` succeeds.
//!
//! Rules enforced:
//! - **R1** (`Mode1ScopedSource`) — a `Mode::Static` route whose
//!   `source_reads` names any non-`global` source is rejected.
//! - **R2** (`Mode1RequiresUser`) — a `Mode::Static` route whose `requires`
//!   contains [`Requires::User`] is rejected.
//!
//! See `.yah/docs/architecture/mesofact.md` §"Render axis × source axis".

use crate::manifest::{Manifest, Requires, RouteMode, MANIFEST_VERSION};
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceScope {
    Global,
    Project,
    User,
}

impl SourceScope {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceScope::Global => "global",
            SourceScope::Project => "project",
            SourceScope::User => "user",
        }
    }
}

pub type SourceCatalog = BTreeMap<String, SourceScope>;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ValidationErrorKind {
    #[error("unsupported manifest version '{got}' (expected '{}')", MANIFEST_VERSION)]
    UnsupportedVersion { got: String },
    #[error("source '{name}' not declared in catalog")]
    UnknownSource { name: String },
    #[error("Mode 1 cannot read from non-'global' source '{name}' (scope='{scope}')")]
    Mode1ScopedSource { name: String, scope: &'static str },
    #[error("Mode 1 cannot require 'user' (the build can't enumerate users)")]
    Mode1RequiresUser,
}

impl ValidationErrorKind {
    /// Snake-case label matching the TS validator's `ValidationErrorKind`.
    /// Stable identifier used by the shared fixture suite.
    pub fn label(&self) -> &'static str {
        match self {
            ValidationErrorKind::UnsupportedVersion { .. } => "unsupported_version",
            ValidationErrorKind::UnknownSource { .. } => "unknown_source",
            ValidationErrorKind::Mode1ScopedSource { .. } => "mode1_scoped_source",
            ValidationErrorKind::Mode1RequiresUser => "mode1_requires_user",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{path}: {kind}")]
pub struct ValidationError {
    pub path: String,
    pub kind: ValidationErrorKind,
}

pub fn validate(manifest: &Manifest, catalog: &SourceCatalog) -> Result<(), Vec<ValidationError>> {
    let mut errors = Vec::new();

    if manifest.version != MANIFEST_VERSION {
        errors.push(ValidationError {
            path: "version".into(),
            kind: ValidationErrorKind::UnsupportedVersion {
                got: manifest.version.clone(),
            },
        });
    }

    for (idx, route) in manifest.routes.iter().enumerate() {
        if !matches!(route.mode, RouteMode::Static) {
            continue;
        }

        if let Some(reads) = &route.source_reads {
            for name in reads {
                let base = format!("routes[{idx}].source_reads");
                match catalog.get(name) {
                    None => errors.push(ValidationError {
                        path: base,
                        kind: ValidationErrorKind::UnknownSource { name: name.clone() },
                    }),
                    Some(SourceScope::Global) => {}
                    Some(other) => errors.push(ValidationError {
                        path: base,
                        kind: ValidationErrorKind::Mode1ScopedSource {
                            name: name.clone(),
                            scope: other.as_str(),
                        },
                    }),
                }
            }
        }

        if let Some(requires) = &route.requires {
            if requires.iter().any(|r| matches!(r, Requires::User)) {
                errors.push(ValidationError {
                    path: format!("routes[{idx}].requires"),
                    kind: ValidationErrorKind::Mode1RequiresUser,
                });
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}
