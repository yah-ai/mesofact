//! Declared-vs-enforced route policy (R749-T1) — the general form of the rule
//! [`crate::manifest`] fields keep re-learning one field at a time.
//!
//! **The rule.** A route policy that is declared in `mesofact.routes.ts` and
//! not enforced by the tier actually serving it is a hard error, named at
//! startup. Never a warning, never a skip, never fail-open.
//!
//! **Why a mechanism rather than another one-off check.** Express middleware is
//! a function you can watch run; a declared policy that nothing wired looks
//! byte-identical to one that is enforced, from outside the process and from
//! inside the manifest alike. That class has now bitten four times — `requires:
//! ["user"]` served fail-open by `mesofact serve` (R556-B13), `route_headers`
//! dropped on a `worker` → `passway` front-door flip (R749-F3),
//! `cache_policy` inert in the serve tier and `concurrency` unread by it
//! (R746-S4's audit, W225 §2c). Each was found by someone reading the code, not
//! by the system. Four is enough to stop fixing instances.
//!
//! **The shape is advertise-and-check**, reused from the bundle/runtime
//! contract (R746-F6, W272): a tier states the policy set it implements
//! ([`PolicySupport`]), and startup diffs that set against what the manifest
//! actually declares ([`check_manifest`]). A tier that gains an enforcement
//! point edits one line here; a tier that never does refuses to serve the
//! routes it would lie about.
//!
//! **Unknown fields refuse too**, and that is the half that makes the class
//! impossible rather than merely closed today. A hand-written serde slice can
//! only miss a policy field added after it — silently, by construction, which
//! is the exact defect. So the check walks the manifest's *raw JSON* keys and
//! refuses any route key it cannot classify, instead of deserializing into a
//! struct that would discard it. An old binary handed a manifest from a newer
//! build stops, naming the field.
//!
//! **The escape hatch is an operator assertion, not a downgrade.** A policy may
//! be [`PolicySupport::delegate`]d to something in front of the process — an
//! authenticating edge, a CDN. That is the generalization of R556-B13's
//! `--trust-edge-auth`: it stays a positive claim someone made, recorded in the
//! process's own startup log, and it can be wrong — but it cannot be *silent*,
//! which is the property this module is defending.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// A route field that changes **serving** behaviour, and can therefore fail
/// open when the serving tier does not implement it.
///
/// Deliberately not "every field on [`crate::manifest::Route`]" — build-time
/// and publish-time fields (`prerender`, `data_inputs`, `placement`,
/// `source_reads`, `hydration`) are consumed before a request exists, so a
/// server that ignores them cannot serve a route that quietly lacks a policy
/// it declared. See [`STRUCTURAL_FIELDS`] for that half of the partition;
/// every key on `Route` belongs to exactly one of the two, pinned by
/// `every_route_field_is_classified`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RoutePolicy {
    /// `requires: ["user" | "project" | "region"]` — the auth gate. The one
    /// whose fail-open is a confidentiality breach rather than a performance
    /// regression, which is why R556-B13 was filed at severity high.
    Requires,
    /// `cache_policy: { ttl, swr, negative_ttl, vary }`.
    CachePolicy,
    /// `concurrency` — per-route in-flight cap.
    Concurrency,
    /// `resilience: { retry, timeout_ms }` (W181).
    Resilience,
}

impl RoutePolicy {
    /// Every policy, in declaration order. Adding a variant without adding it
    /// here fails to compile (the exhaustive matches on `RoutePolicy` below
    /// won't cover it).
    pub const ALL: [RoutePolicy; 4] = [
        RoutePolicy::Requires,
        RoutePolicy::CachePolicy,
        RoutePolicy::Concurrency,
        RoutePolicy::Resilience,
    ];

    /// The manifest / `defineRoutes` field name. This is what an author reads
    /// in their own routes file, so it is what a refusal message must name.
    pub const fn field(self) -> &'static str {
        match self {
            RoutePolicy::Requires => "requires",
            RoutePolicy::CachePolicy => "cache_policy",
            RoutePolicy::Concurrency => "concurrency",
            RoutePolicy::Resilience => "resilience",
        }
    }

    /// What an author expects the field to *do*, one clause. Rendered into the
    /// refusal so the message says what is being lost, not just which key.
    pub const fn effect(self) -> &'static str {
        match self {
            RoutePolicy::Requires => "gate the route behind a resolved session",
            RoutePolicy::CachePolicy => "cache the response for the declared ttl/swr",
            RoutePolicy::Concurrency => "cap in-flight requests for this route",
            RoutePolicy::Resilience => "retry and time-bound the render",
        }
    }

    /// Parse a field name, for `--policy-delegated` / `MESOFACT_POLICY_DELEGATED`.
    pub fn parse(field: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.field() == field)
    }

    /// For a policy carried as a JSON object, the sub-keys this build actually
    /// implements. Empty = the policy is a scalar/array with no inner shape.
    ///
    /// Checked as strictly as the top level, because a policy's *sub*-field is
    /// the same defect one level down and hides better: `serde` on
    /// [`crate::manifest::CachePolicy`] silently drops an unknown key, so a
    /// `cache_policy.private` added by a newer build would deserialize into a
    /// policy that looks complete and quietly is not.
    ///
    /// `resilience.queue` is deliberately **absent** — W181 reserves the slot
    /// for v2 and `defineRoutes` rejects it today, so the type exists only so
    /// v1 binaries keep deserializing v2 manifests. Deserializing one is not
    /// the same as honouring it; a manifest that carries a queue policy must
    /// not be served as though the queueing happens.
    pub const fn subfields(self) -> &'static [&'static str] {
        match self {
            RoutePolicy::CachePolicy => &["ttl", "swr", "negative_ttl", "vary"],
            RoutePolicy::Resilience => &["retry", "timeout_ms"],
            RoutePolicy::Requires | RoutePolicy::Concurrency => &[],
        }
    }

    /// Sub-keys that exist on the manifest type and are deliberately not
    /// implemented. Listing them is what turns "we left it out" into a
    /// decision the completeness gate can check, and it keeps
    /// [`subfields`](Self::subfields)'s omissions from reading as oversights.
    pub const fn reserved_subfields(self) -> &'static [&'static str] {
        match self {
            // W181 § "v1 scope" — type slot only, rejected at `defineRoutes`.
            RoutePolicy::Resilience => &["queue"],
            _ => &[],
        }
    }
}

impl fmt::Display for RoutePolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.field())
    }
}

/// Route keys that carry no serving policy — consumed by the build, the
/// publisher, or the router's own addressing, all of which happen before a
/// request exists.
///
/// This list is the *other half* of the partition [`RoutePolicy`] opens, and
/// it is exhaustive on purpose: a key in neither list is an unknown policy and
/// refuses ([`Violation::UnknownField`]). Growing this list is the deliberate
/// act of saying "this new field cannot fail open".
pub const STRUCTURAL_FIELDS: [&str; 8] = [
    // Routing identity — a server that ignored these would not route at all.
    "route",
    "mode",
    "render_entrypoint",
    // Build-time inference and build-time inputs.
    "source_reads",
    "data_inputs",
    // Build/publish-time: which instances exist, which bundle, which script.
    "prerender",
    "placement",
    "hydration",
];

/// The policy set one serving tier implements, plus the set an operator has
/// asserted is enforced in front of it.
///
/// Constructed at startup by whichever binary is about to serve, so the claim
/// lives next to the code that makes it true rather than in a doc.
#[derive(Debug, Clone)]
pub struct PolicySupport {
    tier: String,
    enforced: BTreeSet<RoutePolicy>,
    delegated: BTreeSet<RoutePolicy>,
}

impl PolicySupport {
    /// A tier that enforces nothing. `tier` names the binary/subcommand as an
    /// operator would type it (`mesofact serve`), since that is the thing they
    /// have to change.
    pub fn new(tier: impl Into<String>) -> Self {
        Self {
            tier: tier.into(),
            enforced: BTreeSet::new(),
            delegated: BTreeSet::new(),
        }
    }

    /// Advertise a policy this tier implements itself.
    #[must_use]
    pub fn enforces(mut self, policy: RoutePolicy) -> Self {
        self.enforced.insert(policy);
        self
    }

    /// Record an operator's assertion that something in front of this process
    /// enforces `policy` (an authenticating edge, a CDN). Distinct from
    /// [`enforces`](Self::enforces) so the startup log can say which of the two
    /// is carrying a route — "we do this" and "someone says they do this" are
    /// not the same claim and should not print the same.
    #[must_use]
    pub fn delegate(mut self, policy: RoutePolicy) -> Self {
        self.delegated.insert(policy);
        self
    }

    pub fn tier(&self) -> &str {
        &self.tier
    }

    pub fn covers(&self, policy: RoutePolicy) -> bool {
        self.enforced.contains(&policy) || self.delegated.contains(&policy)
    }

    pub fn is_delegated(&self, policy: RoutePolicy) -> bool {
        self.delegated.contains(&policy)
    }

    /// Policies this tier implements, for the startup log.
    pub fn enforced(&self) -> impl Iterator<Item = RoutePolicy> + '_ {
        self.enforced.iter().copied()
    }

    /// Policies covered only by an operator assertion, for the startup log.
    pub fn delegated(&self) -> impl Iterator<Item = RoutePolicy> + '_ {
        self.delegated.iter().copied()
    }
}

/// One route declaring something this tier will not do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    /// A known policy, declared, and neither enforced nor delegated.
    Unenforced { route: String, policy: RoutePolicy },
    /// A route key (or policy sub-key) nothing in this binary implements —
    /// either a manifest from a newer build, or a slot reserved for a version
    /// this one is not. Refusing is the whole point: a field we cannot even
    /// name is the maximally-silent case of the defect, and it is the half that
    /// makes the class impossible rather than merely closed today.
    UnknownField { route: String, field: String },
}

impl Violation {
    fn route(&self) -> &str {
        match self {
            Violation::Unenforced { route, .. } | Violation::UnknownField { route, .. } => route,
        }
    }

    fn line(&self) -> String {
        match self {
            Violation::Unenforced { route, policy } => format!(
                "  {route} declares `{}` — nothing here will {}",
                policy.field(),
                policy.effect(),
            ),
            Violation::UnknownField { route, field } => format!(
                "  {route} declares `{field}`, which nothing in this binary implements — the \
                 manifest is either newer than this build or uses a slot reserved for one"
            ),
        }
    }
}

/// Every violation found, rendered as the refusal an operator reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyRefusal {
    pub tier: String,
    pub violations: Vec<Violation>,
}

impl fmt::Display for PolicyRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let n = self.violations.len();
        write!(
            f,
            "refusing to start: {n} route policy declaration(s) that `{}` does not enforce.\n",
            self.tier,
        )?;
        for v in &self.violations {
            writeln!(f, "{}", v.line())?;
        }
        let unknown = self
            .violations
            .iter()
            .any(|v| matches!(v, Violation::UnknownField { .. }));
        let known: BTreeSet<&'static str> = self
            .violations
            .iter()
            .filter_map(|v| match v {
                Violation::Unenforced { policy, .. } => Some(policy.field()),
                Violation::UnknownField { .. } => None,
            })
            .collect();
        write!(
            f,
            "A policy declared here and enforced nowhere is worse than no policy: the route \
             serves 200 and looks correct. Either (a) drop the declaration if it was never \
             meant to bind, (b) serve these routes on a tier that implements it, or (c) if \
             something in front of this process really does enforce it, say so with \
             `--policy-delegated <field>` / `MESOFACT_POLICY_DELEGATED=<field,…>`",
        )?;
        if !known.is_empty() {
            write!(
                f,
                " (here: `{}`)",
                known.into_iter().collect::<Vec<_>>().join(",")
            )?;
        }
        f.write_str(".")?;
        if unknown {
            write!(
                f,
                " The unknown field(s) are not delegatable — upgrade this binary to one that \
                 knows them, or rebuild the workload with a matching toolchain."
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for PolicyRefusal {}

/// Check a built `manifest.json` against what `support` claims to enforce.
///
/// Takes the **raw bytes**, not a [`crate::manifest::Manifest`], and that is
/// load-bearing: deserializing first would discard exactly the unknown keys
/// this is looking for. Only the route objects' key sets are inspected, so a
/// manifest this binary cannot fully model is still checkable.
///
/// A manifest that does not parse as JSON is an error, not a pass — reading
/// "no policies declared" out of a file we failed to understand is the
/// fail-open shape this module exists to prevent.
pub fn check_manifest(raw: &[u8], support: &PolicySupport) -> Result<(), PolicyCheckError> {
    let doc: serde_json::Value =
        serde_json::from_slice(raw).map_err(|e| PolicyCheckError::Unreadable(e.to_string()))?;
    let routes = match doc.get("routes") {
        Some(serde_json::Value::Array(routes)) => routes.as_slice(),
        // `routes` present but not an array is a manifest shape we do not recognize.
        Some(_) => return Err(PolicyCheckError::Unreadable("`routes` is not an array".into())),
        None => &[],
    };
    let mut violations = Vec::new();
    for route in routes {
        let Some(obj) = route.as_object() else {
            return Err(PolicyCheckError::Unreadable(
                "a manifest route entry is not an object".into(),
            ));
        };
        let name = obj
            .get("route")
            .and_then(|v| v.as_str())
            .unwrap_or("<unnamed route>")
            .to_string();
        for (key, value) in obj {
            if STRUCTURAL_FIELDS.contains(&key.as_str()) {
                continue;
            }
            match RoutePolicy::parse(key) {
                Some(policy) => {
                    if is_declared(policy, value) && !support.covers(policy) {
                        violations.push(Violation::Unenforced {
                            route: name.clone(),
                            policy,
                        });
                    }
                    // Sub-keys get the same treatment, and are not delegatable
                    // for the same reason the top-level unknowns are not: an
                    // operator cannot assert an edge enforces a directive
                    // neither of them can name.
                    let known = policy.subfields();
                    if !known.is_empty() {
                        if let Some(obj) = value.as_object() {
                            for sub in obj.keys() {
                                if !known.contains(&sub.as_str()) {
                                    violations.push(Violation::UnknownField {
                                        route: name.clone(),
                                        field: format!("{}.{sub}", policy.field()),
                                    });
                                }
                            }
                        }
                    }
                }
                None => violations.push(Violation::UnknownField {
                    route: name.clone(),
                    field: key.clone(),
                }),
            }
        }
    }
    if violations.is_empty() {
        return Ok(());
    }
    // Stable order: by route, then by the message itself, so a refusal reads
    // the same on every node and diffs cleanly in a deploy log.
    violations.sort_by(|a, b| a.route().cmp(b.route()).then_with(|| a.line().cmp(&b.line())));
    Err(PolicyCheckError::Refused(PolicyRefusal {
        tier: support.tier.clone(),
        violations,
    }))
}

/// Why a policy check could not conclude "this is safe to serve".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyCheckError {
    /// The manifest is present and we could not read it. Not a pass.
    Unreadable(String),
    /// The manifest is fine and declares policy this tier will not honour.
    Refused(PolicyRefusal),
}

impl fmt::Display for PolicyCheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PolicyCheckError::Unreadable(why) => write!(
                f,
                "refusing to start: cannot read the route manifest to check for declared \
                 policy this binary does not enforce — {why}"
            ),
            PolicyCheckError::Refused(r) => r.fmt(f),
        }
    }
}

impl std::error::Error for PolicyCheckError {}

/// Is this field's value an actual declaration, or the inert default every
/// route carries?
///
/// The distinction matters because `cache_policy` is **required** in
/// `defineRoutes` — every route in every workload has one. Treating the
/// no-op form (`{ ttl: 0 }`, which is what an author writes to mean "never
/// cache this") as a declaration would refuse every workload in existence and
/// teach operators to reach straight for the escape hatch, which costs the
/// mechanism its entire value.
fn is_declared(policy: RoutePolicy, value: &serde_json::Value) -> bool {
    if value.is_null() {
        return false;
    }
    match policy {
        RoutePolicy::Requires => value.as_array().is_some_and(|a| !a.is_empty()),
        RoutePolicy::CachePolicy => {
            let Some(obj) = value.as_object() else {
                // A `cache_policy` that is not an object is malformed, and a
                // malformed policy is emphatically not an absent one.
                return true;
            };
            // `ttl: 0` with nothing else is "do not cache" — a statement the
            // serving tier honours by doing nothing.
            obj.get("ttl").and_then(|v| v.as_u64()).unwrap_or(0) > 0
                || obj.contains_key("swr")
                || obj.contains_key("negative_ttl")
                || obj
                    .get("vary")
                    .is_some_and(|v| v.as_array().is_some_and(|a| !a.is_empty()))
        }
        RoutePolicy::Concurrency => true,
        RoutePolicy::Resilience => value
            .as_object()
            .is_some_and(|o| o.values().any(|v| !v.is_null())),
    }
}

/// Group a refusal's violations by policy, for a caller that wants to log the
/// summary rather than the whole list.
pub fn by_policy(refusal: &PolicyRefusal) -> BTreeMap<&'static str, Vec<&str>> {
    let mut out: BTreeMap<&'static str, Vec<&str>> = BTreeMap::new();
    for v in &refusal.violations {
        let key = match v {
            Violation::Unenforced { policy, .. } => policy.field(),
            Violation::UnknownField { .. } => "<unknown>",
        };
        out.entry(key).or_default().push(v.route());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{
        CachePolicy, Hydration, Prerender, Requires, ResiliencePolicy, ResolvedPlacement,
        RetryPolicy, Route, RouteMode,
    };

    fn serve_tier() -> PolicySupport {
        PolicySupport::new("mesofact serve").enforces(RoutePolicy::Resilience)
    }

    fn manifest(routes: &str) -> Vec<u8> {
        format!(r#"{{"version":"1","build_id":"b","routes":[{routes}]}}"#).into_bytes()
    }

    const PLAIN: &str = r#"{"route":"/","mode":"static","render_entrypoint":"e.js","cache_policy":{"ttl":0}}"#;

    /// THE test this ticket exists for, written as the absence of a success
    /// path: a declared-and-unenforced policy must not produce `Ok`. Phrased
    /// this way — rather than asserting on the error string — because the
    /// failure mode is invisible by construction, so the thing worth pinning
    /// is that no input reaches the serving side at all.
    #[test]
    fn a_declared_unenforced_policy_has_no_success_path() {
        let tier = serve_tier();
        let declarations = [
            r#""requires":["user"]"#,
            r#""cache_policy":{"ttl":3600}"#,
            r#""cache_policy":{"ttl":0,"swr":60}"#,
            r#""cache_policy":{"ttl":0,"vary":["accept-language"]}"#,
            r#""concurrency":4"#,
            r#""future_policy":{"limit":2}"#,
        ];
        for decl in declarations {
            let raw = manifest(&format!(
                r#"{{"route":"/p","mode":"ssr","render_entrypoint":"e.js","cache_policy":{{"ttl":0}},{decl}}}"#
            ));
            match check_manifest(&raw, &tier) {
                Err(PolicyCheckError::Refused(_)) => {}
                other => panic!(
                    "declaring {decl} on a tier that does not enforce it returned {other:?}; \
                     that is the silent no-op R749-T1 forbids ({} policies known)",
                    RoutePolicy::ALL.len(),
                ),
            }
        }
    }

    #[test]
    fn the_refusal_names_the_route_and_the_field() {
        let raw = manifest(
            r#"{"route":"/private","mode":"ssr","render_entrypoint":"e.js","cache_policy":{"ttl":0},"requires":["user"]}"#,
        );
        let err = check_manifest(&raw, &serve_tier()).unwrap_err().to_string();
        assert!(err.contains("/private"), "{err}");
        assert!(err.contains("requires"), "{err}");
        assert!(err.contains("--policy-delegated"), "{err}");
    }

    #[test]
    fn the_inert_cache_policy_every_route_carries_is_not_a_declaration() {
        assert!(check_manifest(&manifest(PLAIN), &serve_tier()).is_ok());
    }

    #[test]
    fn an_enforced_policy_passes_and_a_delegated_one_does_too() {
        let raw = manifest(
            r#"{"route":"/a","mode":"ssr","render_entrypoint":"e.js","cache_policy":{"ttl":0},"resilience":{"timeout_ms":5000},"requires":["user"]}"#,
        );
        assert!(check_manifest(&raw, &serve_tier()).is_err());
        let trusting = serve_tier().delegate(RoutePolicy::Requires);
        assert!(check_manifest(&raw, &trusting).is_ok());
        assert!(trusting.is_delegated(RoutePolicy::Requires));
        assert!(!trusting.is_delegated(RoutePolicy::Resilience));
    }

    /// An empty `resilience: {}` block survives a round-trip through
    /// `skip_serializing_if` as `{}`; nothing is being asked for, so nothing
    /// is unenforced.
    #[test]
    fn an_empty_policy_block_is_not_a_declaration() {
        let raw = manifest(
            r#"{"route":"/a","mode":"ssr","render_entrypoint":"e.js","cache_policy":{"ttl":0},"resilience":{},"requires":[]}"#,
        );
        assert!(check_manifest(&raw, &PolicySupport::new("bare")).is_ok());
    }

    #[test]
    fn an_unparseable_manifest_is_not_a_pass() {
        assert!(matches!(
            check_manifest(b"{ not json", &serve_tier()),
            Err(PolicyCheckError::Unreadable(_))
        ));
        assert!(matches!(
            check_manifest(br#"{"routes":"nope"}"#, &serve_tier()),
            Err(PolicyCheckError::Unreadable(_))
        ));
    }

    #[test]
    fn an_unknown_field_is_refused_and_not_delegatable() {
        let raw = manifest(
            r#"{"route":"/x","mode":"ssr","render_entrypoint":"e.js","cache_policy":{"ttl":0},"rate_limit":{"rps":10}}"#,
        );
        let mut permissive = serve_tier();
        for p in RoutePolicy::ALL {
            permissive = permissive.delegate(p);
        }
        let err = check_manifest(&raw, &permissive).unwrap_err().to_string();
        assert!(err.contains("rate_limit"), "{err}");
        assert!(err.contains("not delegatable"), "{err}");
    }

    /// The completeness gate. Every serde key on [`Route`] must be classified
    /// as either a [`RoutePolicy`] or a [`STRUCTURAL_FIELDS`] entry — so adding
    /// a field to the manifest without deciding "can this fail open?" fails
    /// here rather than shipping as the next R556-B13.
    ///
    /// Built from a maximally-populated `Route` rather than a hand-listed set,
    /// because a hand-listed set is exactly the artifact that goes stale.
    #[test]
    fn every_route_field_is_classified() {
        let full = Route {
            route: "/x/:id".into(),
            mode: RouteMode::Ssr,
            render_entrypoint: "dist/server/x.js".into(),
            requires: Some(vec![Requires::User]),
            source_reads: Some(vec!["s".into()]),
            data_inputs: Some(vec!["d.json".into()]),
            cache_policy: CachePolicy {
                ttl: 1,
                swr: Some(1),
                negative_ttl: Some(1),
                vary: Some(vec!["accept".into()]),
            },
            concurrency: Some(1),
            hydration: Some(Hydration {
                script: "s.js".into(),
                code_split: vec![],
            }),
            prerender: Some(Prerender::Deferred { deferred: true }),
            placement: Some(ResolvedPlacement::Host),
            resilience: Some(ResiliencePolicy {
                retry: Some(RetryPolicy {
                    attempts: 2,
                    backoff_ms: vec![10],
                    retry_on: None,
                    budget_ms: None,
                }),
                queue: None,
                timeout_ms: Some(1),
            }),
        };
        let json = serde_json::to_value(&full).unwrap();
        let unclassified: Vec<&String> = json
            .as_object()
            .unwrap()
            .keys()
            .filter(|k| {
                !STRUCTURAL_FIELDS.contains(&k.as_str()) && RoutePolicy::parse(k).is_none()
            })
            .collect();
        assert!(
            unclassified.is_empty(),
            "manifest Route gained field(s) {unclassified:?} that are neither a RoutePolicy nor \
             STRUCTURAL_FIELDS. Decide which: if a serving tier ignoring it would silently drop \
             behaviour an author asked for, it is a RoutePolicy and every tier must advertise \
             it; otherwise add it to STRUCTURAL_FIELDS with a reason.",
        );
        // And the reverse: a policy nothing on `Route` carries would refuse a
        // manifest nobody can produce.
        for policy in RoutePolicy::ALL {
            let value = json
                .get(policy.field())
                .unwrap_or_else(|| panic!(
                    "RoutePolicy::{policy:?} names `{}`, which is not a field on manifest::Route",
                    policy.field(),
                ));
            // Same gate one level down: every sub-key must be implemented or
            // explicitly reserved, so a new `cache_policy` directive cannot
            // land as a field serde quietly drops.
            let Some(obj) = value.as_object() else { continue };
            let unclassified: Vec<&String> = obj
                .keys()
                .filter(|k| {
                    !policy.subfields().contains(&k.as_str())
                        && !policy.reserved_subfields().contains(&k.as_str())
                })
                .collect();
            assert!(
                unclassified.is_empty(),
                "`{}` gained sub-field(s) {unclassified:?}: add them to RoutePolicy::subfields \
                 once a tier implements them, or to reserved_subfields with the doc that \
                 reserves the slot",
                policy.field(),
            );
        }
    }

    /// `resilience.queue` is a v2 slot `defineRoutes` rejects — so a manifest
    /// carrying one did not come from `defineRoutes`, and deserializing it is
    /// not the same as queueing anything.
    #[test]
    fn a_reserved_subfield_refuses_even_where_its_parent_policy_is_enforced() {
        let raw = manifest(
            r#"{"route":"/q","mode":"ssr","render_entrypoint":"e.js","cache_policy":{"ttl":0},"resilience":{"timeout_ms":100,"queue":{"queue":"q","ack":"on_enqueue"}}}"#,
        );
        let err = check_manifest(&raw, &serve_tier()).unwrap_err().to_string();
        assert!(err.contains("resilience.queue"), "{err}");
    }

    #[test]
    fn an_unknown_cache_directive_refuses_instead_of_being_dropped_by_serde() {
        let raw = manifest(
            r#"{"route":"/c","mode":"static","render_entrypoint":"e.js","cache_policy":{"ttl":60,"shared_max_age":30}}"#,
        );
        let tier = serve_tier().enforces(RoutePolicy::CachePolicy);
        let err = check_manifest(&raw, &tier).unwrap_err().to_string();
        assert!(err.contains("cache_policy.shared_max_age"), "{err}");
    }
}
