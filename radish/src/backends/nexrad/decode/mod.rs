//! In-tree NEXRAD Level 2 byte-level decoder. **Not yet wired to the
//! production read path** — see plan `0003-internal-nexrad-decoder`.
//!
//! Phase 1 + 2 deliver:
//!
//! * `error` — typed `NexradDecodeError`.
//! * `reader` — `SliceReader` with explicit big-endian primitives
//!   and the load-bearing `try_skip_to(target)` resync.
//! * `record` — LDM record splitter + bzip2 (parallel via rayon).
//! * `volume` — optional 24-byte Volume Header parser.
//! * `header` — `MessageHeader` + `MessageType` enum.
//! * `messages` — `decode_messages` loop with the boundary fix that
//!   the upstream `nexrad-decode 1.0.0-rc.3` is missing (see
//!   `/tmp/radish-phantom-radials-bug.md`).
//!
//! Phase 3 (next PR) will replace the `Raw` payload variant emitted
//! by `decode_messages` with typed MSG_31 / MSG_2 / MSG_5 parsers,
//! still without changing the boundary-resync behaviour. Phase 7
//! wires this module into `adapter.rs::convert_scan` and drops the
//! runtime dependency on `nexrad-decode`.

pub(super) mod error;
pub(super) mod header;
pub(super) mod messages;
pub(super) mod reader;
pub(super) mod record;
pub(super) mod volume;

#[cfg(test)]
mod integration_test;
