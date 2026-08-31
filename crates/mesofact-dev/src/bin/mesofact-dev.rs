//! `mesofact-dev` — the previous name for the `mes` binary, byte-identical in
//! behaviour and kept only so existing callers keep working.
//!
//! It stays because the old name is load-bearing outside this workspace: ~30
//! spawn sites in the parent yah camp (kamaji, desktop, `cli/serve_build.rs`,
//! `xtask/install.rs`, `local.sh`) exec it by name, and `yah-mesofact-store`
//! hardcodes it in both `EXPECTED_BINS` and `SHIM_NAMES` — so dropping it
//! would make every installed version slot report
//! `Incomplete { missing: ["mesofact-dev"] }`.
//!
//! The legacy `mesofact-dev <DIR> --port N …` invocation still parses, as the
//! CLI's bare form (see [`mesofact_dev::cli`], and the
//! `legacy_mesofact_dev_invocation_still_parses` test that holds it). Delete
//! this target once those callers say `mes`.

#[tokio::main]
async fn main() -> std::process::ExitCode {
    mesofact_dev::cli::run().await
}
