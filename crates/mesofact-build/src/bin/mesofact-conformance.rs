//! **The R773-F7 differential conformance corpus, both tiers, as one CLI.**
//!
//! - `check` is **Tier A, the gate**: hermetic replay of the checked-in corpus.
//!   No network, no `bun`, no `pnpm`, no clock. This is what
//!   `.yah/qed/mesofact-npm-conformance.toml` runs, via
//!   `oss/mesofact/scripts/check-npm-conformance.sh`, and it must pass on a
//!   machine with no JS toolchain installed at all. Its whole implementation is
//!   [`mesofact_build::conformance`].
//! - `record` is **Tier B, the recorder**: it runs the real `bun install` and
//!   `pnpm install`, captures their lockfiles and every packument the resolve
//!   touched, prunes those packuments to a bounded window, and writes a new
//!   case. It needs network and both package managers, it is run by a human
//!   adding or refreshing a case, and **it is never on the gate's path** — the
//!   QED step invokes `check` and nothing else.
//!
//! The separation is the point. A differential harness that needs the network,
//! plus `bun`, plus `pnpm`, to produce a verdict is a pipeline that goes red
//! when the network blinks, and a gate that cries wolf gets ignored. So `record`
//! additionally refuses to run unless `MESOFACT_CONFORMANCE_RECORD=1` is set:
//! nothing can reach the network here by accident, including a future QED step
//! that copies the wrong argv.
//!
//! `record` ends by replaying the case it just wrote through `check`. A recorder
//! that can mint a case the gate rejects — and say nothing — would seed the
//! corpus with red, so a divergence at record time is reported in full and exits
//! non-zero, with the two honest ways forward named: fix the resolver, or write
//! the disagreement down as a `[[known_divergence]]` with a reason.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use mesofact_build::conformance::{self, CaseSpec, CompareSpec, PACKUMENT_DIR, REGISTRY_ID};
use rnpm::transport::HttpTransport;
use rnpm::{
    CachePolicy, CachedPackument, PackumentCache, RegistryClient, RegistrySource, Resolver,
    RootManifest,
};

/// The gate is only as good as the neighbourhood it keeps around each selected
/// version — see `conformance::prune_packument`. Forty releases of the anchor
/// major is far more than any range needs to be non-trivial and still turns
/// `react`'s 2.9 MB packument into a few tens of KB.
const DEFAULT_VERSION_BUDGET: usize = 40;

/// The env var that arms `record`. Not a flag: a flag is one copy-paste away
/// from a pipeline, an env var has to be set deliberately.
const ARM_VAR: &str = "MESOFACT_CONFORMANCE_RECORD";

#[derive(Parser)]
#[command(name = "mesofact-conformance", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Verb,
}

#[derive(Subcommand)]
enum Verb {
    /// Tier A — replay the checked-in corpus offline and report divergences.
    Check {
        /// Corpus root (default: this crate's `tests/corpus`).
        #[arg(long)]
        corpus: Option<PathBuf>,
        /// Replay only this case.
        #[arg(long)]
        case: Option<String>,
    },
    /// Tier B — record a new case from a real `bun install` + `pnpm install`.
    Record {
        /// Case directory name, e.g. `nested-version-conflict`.
        #[arg(long)]
        name: String,
        /// The `package.json` to record. Copied into the case verbatim.
        #[arg(long)]
        manifest: PathBuf,
        /// What divergence this case is for. Recorded into `case.toml`; the
        /// corpus is authored for divergence, not for coverage.
        #[arg(long)]
        description: String,
        #[arg(long)]
        corpus: Option<PathBuf>,
        /// Per-package version budget for the packument prune.
        #[arg(long, default_value_t = DEFAULT_VERSION_BUDGET)]
        budget: usize,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Verb::Check { corpus, case } => check(corpus, case),
        Verb::Record { name, manifest, description, corpus, budget } => {
            record(RecordArgs { name, manifest, description, corpus, budget })
        }
    }
}

/// The corpus root: `--corpus` if given, else this crate's `tests/corpus`
/// resolved off `CARGO_MANIFEST_DIR`, which is baked in at compile time and so
/// works from any working directory — including the camp root, which is where
/// the QED daemon runs a step from.
fn corpus_root(explicit: Option<PathBuf>) -> PathBuf {
    explicit.unwrap_or_else(|| {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(conformance::CORPUS_DIR)
    })
}

// ── Tier A ──────────────────────────────────────────────────────────────────

fn check(corpus: Option<PathBuf>, case: Option<String>) -> Result<()> {
    let root = corpus_root(corpus);
    let report = match case {
        Some(name) => {
            let dir = root.join(&name);
            if !dir.join("case.toml").is_file() {
                bail!("no conformance case named {name:?} under {}", root.display());
            }
            conformance::CorpusReport {
                root: root.clone(),
                cases: vec![conformance::check_case(&dir)?],
            }
        }
        None => conformance::check_corpus(&root)?,
    };

    print!("{}", report.render());
    if report.passed() {
        Ok(())
    } else {
        std::process::exit(1);
    }
}

// ── Tier B ──────────────────────────────────────────────────────────────────

struct RecordArgs {
    name: String,
    manifest: PathBuf,
    description: String,
    corpus: Option<PathBuf>,
    budget: usize,
}

fn record(args: RecordArgs) -> Result<()> {
    if std::env::var(ARM_VAR).as_deref() != Ok("1") {
        bail!(
            "`record` reaches the network and shells out to bun and pnpm, so it is opt-in: \
             set {ARM_VAR}=1 to arm it. The gate (`check`) never needs this."
        );
    }
    require_tool("bun")?;
    require_tool("pnpm")?;

    let manifest_json = std::fs::read_to_string(&args.manifest)
        .with_context(|| format!("reading {}", args.manifest.display()))?;
    refuse_non_registry_deps(&manifest_json)?;

    let case_dir = corpus_root(args.corpus).join(&args.name);
    std::fs::create_dir_all(&case_dir)
        .with_context(|| format!("creating the case directory {}", case_dir.display()))?;
    std::fs::write(case_dir.join("package.json"), &manifest_json)?;

    // ── the two oracles ─────────────────────────────────────────────────────
    // `--lockfile-only` on both: the corpus compares *lockfiles*, so
    // materializing node_modules would be minutes of tarball download for
    // bytes nothing reads.
    let scratch = tempfile::tempdir().context("creating the recorder scratch directory")?;

    let bun_dir = scratch.path().join("bun");
    std::fs::create_dir_all(&bun_dir)?;
    std::fs::write(bun_dir.join("package.json"), &manifest_json)?;
    run(&bun_dir, "bun", &["install", "--lockfile-only", "--ignore-scripts"])?;
    copy_into(&bun_dir.join("bun.lock"), &case_dir.join("bun.lock"))?;

    let pnpm_dir = scratch.path().join("pnpm");
    std::fs::create_dir_all(&pnpm_dir)?;
    std::fs::write(pnpm_dir.join("package.json"), &manifest_json)?;
    // A throwaway cache directory, and it is load-bearing. pnpm serves
    // packument metadata out of `~/Library/Caches/pnpm` past the point where
    // the registry has moved on, and a stale one makes it lock a version bun
    // and the resolver both correctly reject — recorded, that reads as a
    // resolver divergence and is really a fact about this laptop. Observed
    // 2026-09-03 on `postcss` (pnpm 8.5.26, registry latest 8.5.28) and on
    // `electron-to-chromium`. All three oracles must see the same registry
    // instant or the corpus records noise.
    let pnpm_cache = scratch.path().join("pnpm-cache");
    run(
        &pnpm_dir,
        "pnpm",
        &[
            "install",
            "--lockfile-only",
            "--ignore-scripts",
            // `--config.<camelCaseKey>` is the form pnpm 11 honours; the
            // `npm_config_*` env vars are NOT picked up for these keys.
            &format!("--config.cacheDir={}", pnpm_cache.display()),
            // THE load-bearing flag. pnpm 11 ships a supply-chain policy that
            // refuses versions published too recently, so by DEFAULT it locks
            // an older version than bun and npm do for any package that
            // publishes often. Left on, it makes pnpm a systematically biased
            // oracle and every such package reads as a resolver divergence:
            // observed 2026-09-03 on postcss (pnpm 8.5.26 vs 8.5.28),
            // electron-to-chromium and baseline-browser-mapping, all three
            // wrong in the same direction and none of them a resolution
            // question at all. Verified by flipping this one flag.
            "--config.minimumReleaseAge=0",
        ],
    )?;
    copy_into(
        &pnpm_dir.join("pnpm-lock.yaml"),
        &case_dir.join("pnpm-lock.yaml"),
    )?;

    // ── the resolver, online, into a scratch cache ──────────────────────────
    // Not straight into the case: the packuments have to be pruned before they
    // are checked in, and the prune needs the finished tree to know what to
    // keep.
    let live_cache = scratch.path().join("packuments");
    let client = RegistryClient::new(&live_cache, HttpTransport::new()?)
        .with_policy(CachePolicy::MaxAge(std::time::Duration::from_secs(3600)));
    let endpoint = conformance::registry_endpoint();
    let source = RegistrySource::new(&client, &endpoint);

    let manifest = RootManifest::from_package_json(&manifest_json)?;
    let mut tree = Resolver::new(&source).resolve(&manifest)?;
    rnpm::resolve_peers(&mut tree, &source)?;

    // ── what every version-keeping rule anchors on ──────────────────────────
    // All THREE tools' selections, not just the resolver's: pruning to what the
    // resolver picked would delete the very version a divergence is about, and
    // the case would go green by having thrown away the evidence.
    let mut keep: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (_, id) in conformance::tree_by_install_path(&tree) {
        keep.entry(id.name).or_default().insert(id.version);
    }
    merge_lock_versions(&mut keep, &case_dir.join("bun.lock"))?;
    merge_lock_versions(&mut keep, &case_dir.join("pnpm-lock.yaml"))?;

    // ── prune and check in ──────────────────────────────────────────────────
    let case_cache = case_dir.join(PACKUMENT_DIR);
    if case_cache.exists() {
        std::fs::remove_dir_all(&case_cache).with_context(|| {
            format!("clearing the previous packuments at {}", case_cache.display())
        })?;
    }
    let out = PackumentCache::new(&case_cache);
    let mut recorded = 0usize;
    let mut before = 0usize;
    let mut after = 0usize;

    for name in cached_package_names(&live_cache.join(REGISTRY_ID))? {
        let Some(entry) = client.cache().load(REGISTRY_ID, &name)? else {
            continue;
        };
        let mut packument = entry.packument;
        before += packument.versions.len();
        let empty = BTreeSet::new();
        conformance::prune_packument(
            &mut packument,
            keep.get(&name).unwrap_or(&empty),
            args.budget,
        );
        after += packument.versions.len();
        // `etag` and `fetched_at` are dropped to zero deliberately. Neither is
        // read under `CachePolicy::Offline` — a hit is served without
        // consulting `age()` and without a conditional request — and leaving
        // real values in would make every re-record a diff in every fixture for
        // reasons unrelated to resolution.
        out.store(
            REGISTRY_ID,
            &name,
            &CachedPackument { etag: None, fetched_at: 0, packument },
        )?;
        recorded += 1;
    }

    // ── the case file ───────────────────────────────────────────────────────
    let spec = CaseSpec {
        name: args.name.clone(),
        description: args.description,
        recorded_at: today(),
        compare: CompareSpec::default(),
        known_divergence: Vec::new(),
    };
    std::fs::write(case_dir.join("case.toml"), toml::to_string_pretty(&spec)?)?;

    println!(
        "recorded {name}: {recorded} packuments, {before} versions pruned to {after}",
        name = args.name
    );

    // ── replay it through the gate, immediately ─────────────────────────────
    let report = conformance::check_case(&case_dir)?;
    for warning in &report.resolve_warnings {
        println!("  note — {warning}");
    }
    if report.passed() {
        println!(
            "  ok   — replays clean ({} packages, bun {:?}, pnpm {:?} names)",
            report.resolver_packages, report.bun_packages, report.pnpm_names
        );
        return Ok(());
    }

    println!("  FAIL — the case as recorded does not agree with its oracles:");
    for divergence in &report.divergences {
        println!("         {}", divergence.describe());
    }

    // A waiver has to pin the oracle, the key, the kind AND both versions
    // (conformance::KnownDivergence explains why the loose form was a hole), so
    // hand-writing one from the prose above is four chances to typo something
    // that silently waives more than intended. The recorder is holding the
    // exact values, so it emits them ready to paste.
    println!();
    println!("  If a divergence is legitimate, paste into {}/case.toml:", args.name);
    for divergence in &report.divergences {
        let Some(kind) = divergence.kind() else { continue };
        println!();
        println!("      [[known_divergence]]");
        println!("      oracle = {:?}", divergence.oracle().as_str());
        println!("      key = {:?}", divergence.key());
        println!("      kind = {:?}", format!("{kind:?}").to_lowercase());
        if let Some(expected) = divergence.expected() {
            println!("      expected = {expected:?}");
        }
        if let Some(found) = divergence.found() {
            println!("      found = {found:?}");
        }
        println!("      why = \"REPLACE ME: why this disagreement is acceptable\"");
    }
    println!();

    bail!(
        "recorded {name} but it diverges. That is a real finding, not a recorder bug: either \
         fix the resolver, or write the disagreement into {name}/case.toml as a \
         [[known_divergence]] with the reason it is acceptable.",
        name = args.name
    );
}

/// bun and pnpm both fail in ways that are much easier to read up front than
/// halfway through a recording.
fn require_tool(tool: &str) -> Result<()> {
    let found = Command::new(tool)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if found {
        Ok(())
    } else {
        bail!("`record` needs {tool} on PATH and it is not there (or `{tool} --version` failed)")
    }
}

fn run(dir: &Path, program: &str, args: &[&str]) -> Result<()> {
    let output = Command::new(program)
        .args(args)
        .current_dir(dir)
        .output()
        .with_context(|| format!("running {program} {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "{program} {} failed ({}):\n{}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn copy_into(from: &Path, to: &Path) -> Result<()> {
    std::fs::copy(from, to)
        .with_context(|| format!("copying {} to {}", from.display(), to.display()))?;
    Ok(())
}

/// A `file:`, git or tarball dependency has no version and therefore nothing to
/// diverge about — and its target would not exist in the recorder's scratch
/// directory, so `bun install` would fail with a much worse message than this.
fn refuse_non_registry_deps(manifest_json: &str) -> Result<()> {
    let raw: serde_json::Value = serde_json::from_str(manifest_json)?;
    for table in ["dependencies", "devDependencies", "optionalDependencies"] {
        let Some(map) = raw.get(table).and_then(serde_json::Value::as_object) else {
            continue;
        };
        for (name, spec) in map {
            let Some(spec) = spec.as_str() else { continue };
            let non_registry = ["file:", "link:", "workspace:", "git+", "git:", "http:", "https:"]
                .iter()
                .any(|p| spec.starts_with(p));
            if non_registry {
                bail!(
                    "{table}.{name} is {spec:?}, which is not a registry dependency. The corpus \
                     compares resolved versions, and a non-registry dep has none — drop it from \
                     the case manifest."
                );
            }
        }
    }
    Ok(())
}

/// Every package name the live cache holds, read back off disk.
///
/// `PackumentCache` has no listing API (it is a key-value store keyed by a name
/// the caller already has), so the recorder walks the directory and undoes
/// `cache_file_name`'s one mangling: `/` → `%2f`, which is injective because a
/// raw `%` is refused by the name validator.
fn cached_package_names(dir: &Path) -> Result<Vec<String>> {
    let mut names = Vec::new();
    if !dir.exists() {
        return Ok(names);
    }
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let Some(stem) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(stem) = stem.strip_suffix(".json") else { continue };
        // Skip the atomic-write temp files `PackumentCache::store` leaves if a
        // process died mid-write; they start with a dot.
        if stem.starts_with('.') {
            continue;
        }
        names.push(stem.replace("%2f", "/"));
    }
    names.sort();
    Ok(names)
}

/// Fold an oracle's selections into the keep set, so a version only *bun* or
/// only *pnpm* chose survives the prune.
fn merge_lock_versions(
    keep: &mut BTreeMap<String, BTreeSet<String>>,
    lock: &Path,
) -> Result<()> {
    for (name, version) in conformance::lock_selections(lock)? {
        keep.entry(name).or_default().insert(version);
    }
    Ok(())
}

/// `YYYY-MM-DD`, from the system clock. Recording is the one part of this
/// harness that is allowed to read a clock; the gate never does.
fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs / 86_400;
    // Civil-from-days (Howard Hinnant's algorithm), so the recorder does not
    // pull a date crate into this crate's dependency set for one stamp.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}
