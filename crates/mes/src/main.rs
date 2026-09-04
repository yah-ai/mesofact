//! `mes` — the mesofact dev toolchain.
//!
//! This is the entry point and nothing else. The CLI itself is
//! [`mesofact_dev::cli`], which is where it lives so that the definition sits
//! next to the dev-tier affordances it drives rather than in a bin target that
//! nothing can call into. See `../Cargo.toml` for why the command and the
//! library carry different names.

#[tokio::main]
async fn main() -> std::process::ExitCode {
    mesofact_dev::cli::run().await
}

#[cfg(test)]
mod tests {
    //! The one thing this manifest can get wrong, made mechanical.
    //!
    //! Features do not cross a package boundary on their own, so every feature
    //! `mesofact-dev` declares has to be re-exposed here by hand. Forgetting
    //! one is INVISIBLE: `mes` still builds, still runs, and silently ignores
    //! the flag — `cargo install mes --no-default-features` would link V8 and
    //! rolldown anyway, losing the lean static/SPA path that exists so a
    //! consumer can skip the V8 toolchain. Nothing else in this workspace
    //! notices, which is why the check lives here rather than in a doc.

    use std::collections::{BTreeMap, BTreeSet};

    type Features = BTreeMap<String, Vec<String>>;

    fn features_of(manifest: &str) -> Features {
        let doc: toml::Value = toml::from_str(manifest).expect("manifest parses");
        let Some(table) = doc.get("features").and_then(|f| f.as_table()) else {
            return Features::new();
        };
        table
            .iter()
            .map(|(k, v)| {
                let entries = v
                    .as_array()
                    .expect("a feature's value is an array")
                    .iter()
                    .map(|e| e.as_str().expect("a feature entry is a string").to_string())
                    .collect();
                (k.clone(), entries)
            })
            .collect()
    }

    #[test]
    fn every_mesofact_dev_feature_is_forwarded() {
        let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let dev_manifest = here.join("../mesofact-dev/Cargo.toml");
        let Ok(dev_text) = std::fs::read_to_string(&dev_manifest) else {
            // Packaged `.crate` tarballs carry no siblings. Nothing to check.
            return;
        };
        let dev = features_of(&dev_text);
        let mes = features_of(&std::fs::read_to_string(here.join("Cargo.toml")).unwrap());

        assert_eq!(
            dev.keys().collect::<BTreeSet<_>>(),
            mes.keys().collect::<BTreeSet<_>>(),
            "mes must declare exactly the features mesofact-dev does — a feature \
             present in only one of the two is either unreachable or a no-op"
        );

        assert_eq!(
            dev["default"], mes["default"],
            "the default feature set must match, or `cargo install mes` and \
             `cargo build -p mesofact-dev` link different things"
        );

        for (name, dev_entries) in dev.iter().filter(|(k, _)| k.as_str() != "default") {
            let forwarded = format!("mesofact-dev/{name}");
            assert!(
                mes[name].contains(&forwarded),
                "mes's `{name}` does not enable `{forwarded}`, so passing it does nothing"
            );
            // An implication between mesofact-dev's own features (today:
            // `ssr` pulls `publish`, so the facade's `publish` cfg and this
            // crate's can't disagree) has to be mirrored, or `mes --features
            // ssr` and `mesofact-dev --features ssr` diverge.
            for implied in dev_entries.iter().filter(|e| dev.contains_key(*e)) {
                assert!(
                    mes[name].contains(implied),
                    "mesofact-dev's `{name}` implies `{implied}`; mes's `{name}` must too"
                );
            }
        }
    }
}
