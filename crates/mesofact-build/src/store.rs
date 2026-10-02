//! Content-addressed store of *unpacked* npm packages (R771-F1, W319 §2).
//!
//! One entry per package, keyed by the sha512 integrity the lockfile already
//! carries, holding the tree as npm's tarball unpacks it (leading `package/`
//! component stripped). Every project on the machine materializes out of the
//! same entry, so `react@18.3.1` is downloaded and unpacked once rather than
//! once per project.
//!
//! **Not per-file addressing.** pnpm hashes individual files so identical
//! files dedupe across package *versions*; that needs a per-file index, mode
//! and symlink records, and N links per install. Per-package addressing
//! already collects the win that matters, and the choice is an *internal* one:
//! a caller asks for "the tree for this integrity" either way, so switching
//! later touches nothing outside this module (W319 §2).
//!
//! **Entries are immutable and self-verifying.** The directory name is the
//! hash of the tarball that produced it, so:
//!
//! - the tarball is verified *before* anything is written — a truncated or
//!   substituted download never reaches the store, under any key;
//! - unpacking happens in a temp directory on the same filesystem and is
//!   `rename`d into place, so a reader never observes a half-written entry
//!   and there is no poisoned-entry state to evict later (the previous
//!   tarball cache had to delete a bad `.tgz`; nothing durable is written
//!   here until it has already been proven correct);
//! - a concurrent installer that loses the rename race discards its own temp
//!   copy and uses the winner's entry. Both succeed.
//!
//! Deletion is out of scope (W319 §5): nothing here removes an entry, and GC
//! is a later mark-and-sweep from the set of lockfiles that reference an
//! integrity.

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha512};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Points the store at another directory — for CI, which wants it on a warm
/// volume that outlives the job, and for tests.
pub const STORE_DIR_ENV: &str = "MESOFACT_STORE_DIR";

#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// `$MESOFACT_STORE_DIR`, else `$XDG_CACHE_HOME/mesofact/store`, else
    /// `$HOME/.cache/mesofact/store` — the same base `cache_root()` used for
    /// the tarball cache this replaces.
    pub fn open() -> Result<Self> {
        if let Some(dir) = std::env::var_os(STORE_DIR_ENV) {
            if !dir.is_empty() {
                return Ok(Self::at(PathBuf::from(dir)));
            }
        }
        let base = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .ok_or_else(|| {
                anyhow!("neither {STORE_DIR_ENV}, XDG_CACHE_HOME nor HOME is set")
            })?;
        Ok(Self::at(base.join("mesofact").join("store")))
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where the entry for `integrity` lives, whether or not it exists:
    /// `<root>/sha512/<first 2 hex>/<remaining 126 hex>`.
    ///
    /// The lock's integrity is base64 (SRI); base64 contains `/`, so the
    /// digest is re-encoded as hex to be a path at all. Doing that decode here
    /// also validates the integrity string before it is used as a key — a
    /// malformed one is refused rather than turned into a plausible-looking
    /// directory name.
    pub fn entry_path(&self, integrity: &str) -> Result<PathBuf> {
        let digest = parse_sha512_integrity(integrity)?;
        let hex = hex_encode(&digest);
        Ok(self.root.join("sha512").join(&hex[..2]).join(&hex[2..]))
    }

    /// The entry for `integrity` if it is already present.
    pub fn lookup(&self, integrity: &str) -> Result<Option<PathBuf>> {
        let path = self.entry_path(integrity)?;
        Ok(path.is_dir().then_some(path))
    }

    /// The entry for `integrity`, fetching and inserting it if absent.
    ///
    /// `fetch` is called only on a miss, which is what makes a second project
    /// wanting the same package cost nothing: no request, no unpack, no copy
    /// of the tarball in memory.
    pub fn ensure<F>(&self, integrity: &str, fetch: F) -> Result<PathBuf>
    where
        F: FnOnce() -> Result<Vec<u8>>,
    {
        if let Some(hit) = self.lookup(integrity)? {
            return Ok(hit);
        }
        let tarball = fetch()?;
        self.insert_tarball(integrity, &tarball)
    }

    /// Verify `tarball` against `integrity`, unpack it, and land it as the
    /// entry for that integrity. Returns the entry directory.
    ///
    /// Verification happens first and the unpack happens in a sibling temp
    /// directory, so a failure at any point leaves the store exactly as it
    /// was.
    pub fn insert_tarball(&self, integrity: &str, tarball: &[u8]) -> Result<PathBuf> {
        let expected = parse_sha512_integrity(integrity)?;
        let got: [u8; 64] = Sha512::digest(tarball).into();
        if got != expected {
            bail!(
                "integrity mismatch: expected {integrity}, got sha512-{}",
                base64_encode(&got)
            );
        }

        let dest = self.entry_path(integrity)?;
        if dest.is_dir() {
            return Ok(dest);
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating store shard {}", parent.display()))?;
        }

        // Same filesystem as `dest` by construction (both under the store
        // root), which is what makes the rename below atomic.
        let temp = TempEntry::new(&self.root)?;
        unpack_tarball(tarball, temp.path())
            .with_context(|| format!("unpacking {integrity} into the store"))?;

        match std::fs::rename(temp.path(), &dest) {
            Ok(()) => {
                temp.disarm();
                Ok(dest)
            }
            // Lost the race: a concurrent installer landed the same content
            // under the same key first. Its entry is ours by definition —
            // the key is the hash — so drop our copy (TempEntry's Drop) and
            // use theirs. A non-empty destination directory is exactly what
            // makes `rename` refuse here.
            Err(_) if dest.is_dir() => Ok(dest),
            Err(e) => Err(e).with_context(|| {
                format!("landing store entry {} from {}", dest.display(), temp.path().display())
            }),
        }
    }
}

/// A directory that deletes itself unless it is renamed away. Every early
/// return between the unpack and the rename has to leave the store clean, and
/// `?` makes that a Drop job rather than a discipline job.
struct TempEntry {
    path: PathBuf,
    armed: bool,
}

impl TempEntry {
    fn new(store_root: &Path) -> Result<Self> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let tmp_root = store_root.join("tmp");
        std::fs::create_dir_all(&tmp_root)
            .with_context(|| format!("creating {}", tmp_root.display()))?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = tmp_root.join(format!(
            "{}-{}-{}",
            std::process::id(),
            nanos,
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path)
            .with_context(|| format!("creating {}", path.display()))?;
        Ok(Self { path, armed: true })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for TempEntry {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// Unpack an npm tarball into `dest`, stripping the single leading directory
/// component every npm tarball roots its contents under (almost always
/// `package/`).
///
/// Two things this does that the pre-store extraction did not:
///
/// - **Refuses an entry that escapes the package root.** A tarball is remote
///   content; `package/../../..` would otherwise write outside the entry. The
///   integrity check proves the bytes are the ones the lock names, not that
///   they are benign.
/// - **Preserves the mode.** Executables in a package's `bin/` arrived as
///   0644 before, which would break them the moment anything links
///   `node_modules/.bin` (nothing does yet — see the module header).
fn unpack_tarball(tarball: &[u8], dest: &Path) -> Result<()> {
    let gz = flate2::read::GzDecoder::new(tarball);
    let mut archive = tar::Archive::new(gz);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let stripped: PathBuf = path.components().skip(1).collect();
        if stripped.as_os_str().is_empty() {
            continue;
        }
        if stripped.components().any(|c| !matches!(c, Component::Normal(_))) {
            bail!("tarball entry {} escapes the package root", path.display());
        }
        let out_path = dest.join(&stripped);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            std::fs::create_dir_all(&out_path)?;
        } else if kind.is_symlink() {
            let target = entry
                .link_name()?
                .ok_or_else(|| anyhow!("tarball entry {} is a symlink with no target", path.display()))?
                .into_owned();
            symlink_entry(&target, &out_path)?;
        } else if kind.is_file() {
            let mode = entry.header().mode().ok();
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            std::fs::write(&out_path, buf)?;
            set_mode(&out_path, mode)?;
        } else {
            // Hard links, devices and fifos are not things npm publishes, and
            // an entry type this does not understand is not something to
            // guess at silently.
            bail!(
                "tarball entry {} has unsupported type {:?}",
                path.display(),
                kind
            );
        }
    }
    Ok(())
}

#[cfg(unix)]
fn symlink_entry(target: &Path, out_path: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, out_path)
        .with_context(|| format!("symlinking {}", out_path.display()))
}

#[cfg(not(unix))]
fn symlink_entry(_target: &Path, out_path: &Path) -> Result<()> {
    bail!("tarball entry {} is a symlink; unpacking those is unix-only for now", out_path.display())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: Option<u32>) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    // Tarballs in the wild carry 0777 group/other bits from whatever built
    // them; keep only the meaningful distinction (is it executable) and write
    // the conventional mode for it.
    let executable = mode.is_some_and(|m| m & 0o111 != 0);
    let perms = std::fs::Permissions::from_mode(if executable { 0o755 } else { 0o644 });
    std::fs::set_permissions(path, perms)
        .with_context(|| format!("setting mode on {}", path.display()))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: Option<u32>) -> Result<()> {
    Ok(())
}

/// The 64 raw digest bytes an SRI `sha512-<base64>` names.
///
/// SRI permits several space-separated hashes; the strongest one wins and
/// sha512 is the only one this installer accepts, so the first token is read
/// and any weaker companion ignored. That also means `sha512-A` and
/// `sha512-A sha1-B` are the same key, which is right — they name the same
/// bytes.
fn parse_sha512_integrity(integrity: &str) -> Result<[u8; 64]> {
    let first = integrity
        .split_whitespace()
        .next()
        .ok_or_else(|| anyhow!("empty integrity string"))?;
    let Some(b64) = first.strip_prefix("sha512-") else {
        bail!("integrity {integrity:?} is not sha512 — only sha512 is verified");
    };
    let bytes = base64_decode(b64)
        .with_context(|| format!("integrity {integrity:?} is not valid base64"))?;
    let len = bytes.len();
    bytes
        .try_into()
        .map_err(|_| anyhow!("integrity {integrity:?} decodes to {len} bytes, not 64"))
}

const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard (non-url-safe, padded) base64 — npm integrity strings use it.
pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Inverse of [`base64_encode`]. Strict: padding must be well-formed and any
/// character outside the standard alphabet is an error, because the result is
/// used as a *key* — quietly tolerating junk would file two different
/// integrity strings under one entry.
fn base64_decode(s: &str) -> Result<Vec<u8>> {
    if !s.len().is_multiple_of(4) {
        bail!("length {} is not a multiple of 4", s.len());
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    for (ci, chunk) in bytes.chunks(4).enumerate() {
        let last = ci == bytes.len() / 4 - 1;
        let mut n: u32 = 0;
        let mut pad = 0;
        for (i, &c) in chunk.iter().enumerate() {
            let v = if c == b'=' {
                if !last || i < 2 {
                    bail!("misplaced padding");
                }
                pad += 1;
                0
            } else {
                if pad > 0 {
                    bail!("character after padding");
                }
                TABLE
                    .iter()
                    .position(|&t| t == c)
                    .ok_or_else(|| anyhow!("invalid base64 character {:?}", c as char))?
                    as u32
            };
            n = (n << 6) | v;
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Ok(out)
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn gzip_tarball(files: &[(&str, &str, u32)]) -> Vec<u8> {
        let mut tar = tar::Builder::new(Vec::new());
        for (path, body, mode) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(*mode);
            header.set_entry_type(tar::EntryType::Regular);
            // The name goes straight into the header rather than through
            // `set_path`/`append_data`, which refuse a `..` — the escaping
            // tarball this has to produce for the traversal test is exactly
            // the thing a well-behaved writer will not emit.
            let name = path.as_bytes();
            header.as_old_mut().name[..name.len()].copy_from_slice(name);
            header.set_cksum();
            tar.append(&header, body.as_bytes()).unwrap();
        }
        let raw = tar.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&raw).unwrap();
        gz.finish().unwrap()
    }

    fn integrity_of(bytes: &[u8]) -> String {
        format!("sha512-{}", base64_encode(&Sha512::digest(bytes)))
    }

    fn a_package() -> (Vec<u8>, String) {
        let tarball = gzip_tarball(&[
            ("package/package.json", r#"{"name":"demo","version":"1.0.0"}"#, 0o644),
            ("package/lib/index.js", "module.exports = 1;\n", 0o644),
            ("package/bin/demo", "#!/usr/bin/env node\n", 0o755),
        ]);
        let integrity = integrity_of(&tarball);
        (tarball, integrity)
    }

    #[test]
    fn base64_round_trips_and_matches_known_vectors() {
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"ab"), "YWI=");
        for case in [&b""[..], b"a", b"ab", b"abc", b"hello world", &[0u8, 255, 128, 7]] {
            assert_eq!(base64_decode(&base64_encode(case)).unwrap(), case, "{case:?}");
        }
        assert!(base64_decode("aGVsbG8").is_err(), "unpadded length");
        assert!(base64_decode("aGV*bG8=").is_err(), "invalid character");
        assert!(base64_decode("a=VsbG8=").is_err(), "misplaced padding");
    }

    #[test]
    fn entry_path_is_the_hex_digest_sharded_by_two() {
        let store = Store::at("/s");
        let integrity = format!("sha512-{}", base64_encode(&Sha512::digest(b"x")));
        let hex = hex_encode(&Sha512::digest(b"x"));
        assert_eq!(
            store.entry_path(&integrity).unwrap(),
            Path::new("/s/sha512").join(&hex[..2]).join(&hex[2..])
        );
        // The sha512 of anything is 64 bytes → 128 hex chars.
        assert_eq!(hex.len(), 128);
    }

    #[test]
    fn entry_path_refuses_an_integrity_it_cannot_key_by() {
        let store = Store::at("/s");
        for bad in [
            "sha1-YtRJHQGwB4z8m2vRvBpNLbTAWnQ=",
            "sha512-not!base64!",
            "sha512-aGVsbG8=", // decodes, but to 5 bytes rather than 64
            "",
        ] {
            assert!(store.entry_path(bad).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn multi_hash_integrity_keys_off_the_sha512() {
        let store = Store::at("/s");
        let sha512 = format!("sha512-{}", base64_encode(&Sha512::digest(b"x")));
        assert_eq!(
            store.entry_path(&sha512).unwrap(),
            store
                .entry_path(&format!("{sha512} sha1-YtRJHQGwB4z8m2vRvBpNLbTAWnQ="))
                .unwrap()
        );
    }

    #[test]
    fn insert_unpacks_stripped_of_the_leading_component() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::at(tmp.path());
        let (tarball, integrity) = a_package();

        let entry = store.insert_tarball(&integrity, &tarball).unwrap();
        assert_eq!(entry, store.entry_path(&integrity).unwrap());
        assert!(entry.starts_with(tmp.path().join("sha512")));
        assert_eq!(
            std::fs::read_to_string(entry.join("package.json")).unwrap(),
            r#"{"name":"demo","version":"1.0.0"}"#
        );
        assert_eq!(
            std::fs::read_to_string(entry.join("lib/index.js")).unwrap(),
            "module.exports = 1;\n"
        );
        assert!(!entry.join("package").exists(), "leading component not stripped");
        assert_eq!(store.lookup(&integrity).unwrap().as_deref(), Some(entry.as_path()));
    }

    #[cfg(unix)]
    #[test]
    fn insert_preserves_the_executable_bit() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::at(tmp.path());
        let (tarball, integrity) = a_package();
        let entry = store.insert_tarball(&integrity, &tarball).unwrap();

        let mode = |p: &str| {
            std::fs::metadata(entry.join(p)).unwrap().permissions().mode() & 0o777
        };
        assert_eq!(mode("bin/demo"), 0o755);
        assert_eq!(mode("package.json"), 0o644);
    }

    #[test]
    fn a_corrupted_download_never_lands_under_a_valid_key() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::at(tmp.path());
        let (tarball, integrity) = a_package();
        let truncated = &tarball[..tarball.len() / 2];

        let err = store.insert_tarball(&integrity, truncated).unwrap_err().to_string();
        assert!(err.contains("integrity mismatch"), "{err}");
        assert!(store.lookup(&integrity).unwrap().is_none());
        assert!(!tmp.path().join("sha512").exists(), "no shard directory was created");
        assert!(no_temp_entries(&store), "a temp entry was left behind");
    }

    #[test]
    fn a_tarball_escaping_its_root_lands_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::at(tmp.path());
        let tarball = gzip_tarball(&[
            ("package/ok.js", "1", 0o644),
            ("package/../../escaped", "pwned", 0o644),
        ]);
        let integrity = integrity_of(&tarball);

        // `{:#}` — the refusal is the *cause*, under the "unpacking …" context.
        let err = format!("{:#}", store.insert_tarball(&integrity, &tarball).unwrap_err());
        assert!(err.contains("escapes the package root"), "{err}");
        assert!(store.lookup(&integrity).unwrap().is_none());
        assert!(!tmp.path().join("escaped").exists());
        assert!(no_temp_entries(&store), "a temp entry was left behind");
    }

    #[test]
    fn a_second_insert_is_a_no_op_and_leaves_one_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::at(tmp.path());
        let (tarball, integrity) = a_package();

        let first = store.insert_tarball(&integrity, &tarball).unwrap();
        let second = store.insert_tarball(&integrity, &tarball).unwrap();
        assert_eq!(first, second);
        assert_eq!(entry_count(&store), 1);
        assert!(no_temp_entries(&store));
    }

    /// W319 §2's race: two installers unpack the same package at once, one
    /// loses the rename. Both must succeed against one entry.
    #[test]
    fn concurrent_inserts_leave_exactly_one_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::at(tmp.path());
        let (tarball, integrity) = a_package();

        let handles: Vec<_> = (0..8)
            .map(|_| {
                let store = store.clone();
                let tarball = tarball.clone();
                let integrity = integrity.clone();
                std::thread::spawn(move || store.insert_tarball(&integrity, &tarball).unwrap())
            })
            .collect();
        let paths: Vec<PathBuf> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        assert!(paths.iter().all(|p| *p == paths[0]), "{paths:?}");
        assert_eq!(entry_count(&store), 1);
        assert!(no_temp_entries(&store), "a loser left its temp copy behind");
        assert!(paths[0].join("lib/index.js").is_file());
    }

    /// The two-projects case from R771-F1's verify list: the second install
    /// of a package already in the store fetches nothing.
    #[test]
    fn ensure_fetches_only_on_a_miss() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::at(tmp.path());
        let (tarball, integrity) = a_package();
        let fetches = std::cell::Cell::new(0);
        let fetch = || {
            fetches.set(fetches.get() + 1);
            Ok(tarball.clone())
        };

        let project_a = store.ensure(&integrity, fetch).unwrap();
        let project_b = store.ensure(&integrity, fetch).unwrap();
        assert_eq!(project_a, project_b);
        assert_eq!(fetches.get(), 1, "the second project re-fetched");
        assert_eq!(entry_count(&store), 1);
    }

    #[test]
    fn ensure_propagates_a_fetch_failure_without_touching_the_store() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::at(tmp.path());
        let (_, integrity) = a_package();

        let err = store
            .ensure(&integrity, || bail!("GET … → 404 Not Found"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("404"), "{err}");
        assert!(store.lookup(&integrity).unwrap().is_none());
    }

    #[test]
    fn open_honours_the_env_override_shape() {
        // Not asserted against the real environment (tests share a process);
        // this pins the layout `open` would produce for a given root.
        let store = Store::at("/warm/volume");
        assert_eq!(store.root(), Path::new("/warm/volume"));
        assert!(store
            .entry_path(&format!("sha512-{}", base64_encode(&Sha512::digest(b"x"))))
            .unwrap()
            .starts_with("/warm/volume/sha512"));
    }

    fn entry_count(store: &Store) -> usize {
        let sha512 = store.root().join("sha512");
        let Ok(shards) = std::fs::read_dir(&sha512) else {
            return 0;
        };
        shards
            .filter_map(Result::ok)
            .filter_map(|shard| std::fs::read_dir(shard.path()).ok())
            .map(|entries| entries.filter_map(Result::ok).count())
            .sum()
    }

    fn no_temp_entries(store: &Store) -> bool {
        match std::fs::read_dir(store.root().join("tmp")) {
            Ok(mut d) => d.next().is_none(),
            Err(_) => true,
        }
    }
}
