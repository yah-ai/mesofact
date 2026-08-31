//! `mes` — the mesofact dev toolchain. The CLI itself is
//! [`mesofact_dev::cli`]; this is only the entry point.

#[tokio::main]
async fn main() -> std::process::ExitCode {
    mesofact_dev::cli::run().await
}
