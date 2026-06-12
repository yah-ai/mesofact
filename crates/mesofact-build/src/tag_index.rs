//! `tag-index.json` emission — port of `packages/mesofact-build/src/tag-index.ts`.

use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Serialize)]
pub struct TagIndex {
    pub build_id: String,
    pub tags: BTreeMap<String, Vec<String>>,
}

pub struct Emission {
    pub url: String,
    pub tags: Vec<String>,
}

pub fn build_tag_index(build_id: &str, emissions: &[Emission]) -> TagIndex {
    let mut tags: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for e in emissions {
        for tag in &e.tags {
            tags.entry(tag.clone()).or_default().insert(e.url.clone());
        }
    }
    TagIndex {
        build_id: build_id.to_string(),
        tags: tags.into_iter().map(|(k, v)| (k, v.into_iter().collect())).collect(),
    }
}
