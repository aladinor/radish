//! Shared helpers for the fixture-backed golden-corpus parity tests
//! (`test_nexrad_level3_parity.rs`, `test_dealias_parity.rs`). Pulled out
//! after an adversarial review flagged `hex32`/`fixture_path`/
//! `load_expected` as byte-for-byte-identical (or near-identical, module
//! aside) across those two files — see
//! `plans/0011-nexrad-level3-wasm-backend.md` Phase 4/6's parity-gate
//! sections.
//!
//! `test_nexrad.rs` (pre-existing, Level 2 corpus) has its own inline
//! `hex32` too — not migrated here to keep this change scoped to the
//! files this session actually added, rather than touching an
//! established test file for a cosmetic win.
//!
//! A `tests/common/mod.rs` (not `tests/common.rs`) is the Cargo
//! convention for test-only code shared across integration test
//! binaries without becoming its own test binary.

#![allow(dead_code)] // not every consumer of this module uses every item

use std::path::PathBuf;

/// Hex-format a SHA-256 digest without pulling in the `hex` crate —
/// matches the convention already established in `test_nexrad.rs`.
pub fn hex32(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in bytes {
        std::fmt::Write::write_fmt(&mut s, format_args!("{b:02x}")).unwrap();
    }
    s
}

/// Resolve a fixture's on-disk path from an environment variable, or
/// `None` to skip cleanly (the fixture-parity convention this crate uses
/// throughout — see `radish/tests/fixtures/CORPUS.md`'s "Test gating"
/// sections).
pub fn fixture_path(env_var: &str, name: &str) -> Option<PathBuf> {
    let dir = std::env::var_os(env_var)?;
    let candidate = PathBuf::from(dir).join(name);
    candidate.is_file().then_some(candidate)
}

/// Deserialize a committed JSON sidecar from `dir/{name}.json`, panicking
/// with the file path on any failure (read or parse) — these sidecars
/// are committed, version-controlled fixtures; a missing or malformed
/// one is a test-setup bug, not a runtime condition to handle gracefully.
pub fn load_expected<T: serde::de::DeserializeOwned>(dir: &std::path::Path, name: &str) -> T {
    let path = dir.join(format!("{name}.json"));
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("read expected sidecar {}: {e}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("parse expected sidecar {}: {e}", path.display()))
}
