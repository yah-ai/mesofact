//! A hit-counting, table-backed [`Transport`] — the thing that turns
//! "resolving a package with a warm cache issues no network request" from a
//! claim into an assertion.
//!
//! Behind the `testing` feature so it does not ship in a normal build, but
//! `pub` under it so the `jsget` facade can prove the *same* property across
//! its own seam without re-implementing a fake.

use anyhow::Result;
use std::cell::RefCell;
use std::collections::HashMap;

use crate::transport::{Request, Response, Transport};

/// Answers from a table and records every request it was given.
///
/// The assertion that matters is on [`FakeTransport::hits`]: a regression that
/// reintroduces a round trip fails here even though the returned packument is
/// still perfectly correct.
#[derive(Default)]
pub struct FakeTransport {
    bodies: HashMap<String, (Option<String>, String)>,
    requests: RefCell<Vec<Request>>,
}

impl FakeTransport {
    pub fn new() -> Self {
        Self::default()
    }

    /// Serve `body` at `url`, with `etag` as its validator. A URL not in the
    /// table answers 404, which is what a real registry does for a package
    /// that does not exist.
    pub fn serving(mut self, url: &str, etag: Option<&str>, body: &str) -> Self {
        self.bodies
            .insert(url.to_string(), (etag.map(str::to_owned), body.to_string()));
        self
    }

    pub fn requests(&self) -> Vec<Request> {
        self.requests.borrow().clone()
    }

    pub fn urls(&self) -> Vec<String> {
        self.requests.borrow().iter().map(|r| r.url.clone()).collect()
    }

    pub fn hits(&self) -> usize {
        self.requests.borrow().len()
    }
}

impl Transport for FakeTransport {
    fn get(&self, request: &Request) -> Result<Response> {
        self.requests.borrow_mut().push(request.clone());
        let Some((etag, body)) = self.bodies.get(&request.url) else {
            return Ok(Response::NotFound);
        };
        // A conditional request whose validator still matches is a 304 — the
        // same answer a real registry gives, so the 304 branch is exercised
        // with no network anywhere in the test.
        if request.if_none_match.is_some() && request.if_none_match == *etag {
            return Ok(Response::NotModified);
        }
        Ok(Response::Body { etag: etag.clone(), bytes: body.clone().into_bytes() })
    }
}
