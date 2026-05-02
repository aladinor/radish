//! Format detection for NEXRAD Level 2 (Archive II / AR2V) files.
//!
//! NEXRAD files are commonly distributed with no extension at all
//! (e.g. `KLOT20260310_231412_V06`), so this module provides three
//! signals callers can OR together:
//!
//! 1. an extension match (`ar2`, `ar2v`),
//! 2. a magic-byte check (`AR2V` at byte 0),
//! 3. a filename pattern check (`AAAA########_######` with optional `_V##`).

use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Length of the canonical filename core: 4 ICAO + 8 date + `_` + 6 time.
const NEXRAD_NAME_CORE_LEN: usize = 19;
/// Length of the canonical filename plus a `_V##` suffix.
const NEXRAD_NAME_WITH_VOLUME_LEN: usize = 23;

/// File extensions for NEXRAD Level 2 archive files.
pub(crate) const EXTENSIONS: &[&str] = &["ar2", "ar2v"];

/// Returns `true` if the first four bytes of the file are the NEXRAD volume
/// header magic `AR2V`. Returns `false` on any I/O error.
pub(crate) fn is_ar2v(path: &Path) -> bool {
    let mut buf = [0u8; 4];
    match File::open(path).and_then(|mut f| f.read_exact(&mut buf)) {
        Ok(()) => &buf == b"AR2V",
        Err(_) => false,
    }
}

/// Returns `true` if the path's file name matches the canonical NEXRAD naming
/// convention (e.g. `KLOT20260310_231412_V06`).
pub(crate) fn matches_nexrad_filename(path: &Path) -> bool {
    nexrad_icao_from_name(path).is_some()
}

/// Extracts the 4-letter ICAO from a path whose file name matches the
/// canonical NEXRAD naming convention. Returns `None` for any non-matching
/// name (this is the strict counterpart to a regex match — there is no
/// "best effort" path that could leak a non-NEXRAD prefix as an ICAO).
pub(crate) fn icao_from_filename(path: &Path) -> Option<&str> {
    nexrad_icao_from_name(path)
}

fn nexrad_icao_from_name(path: &Path) -> Option<&str> {
    let name = path.file_name()?.to_str()?;
    let bytes = name.as_bytes();
    if bytes.len() != NEXRAD_NAME_CORE_LEN && bytes.len() != NEXRAD_NAME_WITH_VOLUME_LEN {
        return None;
    }
    // `AAAAYYYYMMDD_HHMMSS` core; optional `_VNN` tail.
    let core_ok = bytes[..4].iter().all(|&b| b.is_ascii_uppercase())
        && bytes[4..12].iter().all(|&b| b.is_ascii_digit())
        && bytes[12] == b'_'
        && bytes[13..19].iter().all(|&b| b.is_ascii_digit());
    if !core_ok {
        return None;
    }
    if bytes.len() == NEXRAD_NAME_WITH_VOLUME_LEN {
        let tail_ok = bytes[19] == b'_'
            && bytes[20] == b'V'
            && bytes[21..23].iter().all(|&b| b.is_ascii_digit());
        if !tail_ok {
            return None;
        }
    }
    // Safe: we just verified the first four bytes are ASCII uppercase, so the
    // string slice falls on a UTF-8 boundary.
    Some(&name[..4])
}

/// Combined check: extension OR magic OR filename pattern.
pub(crate) fn looks_like_nexrad(path: &Path) -> bool {
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        if EXTENSIONS.iter().any(|e| e.eq_ignore_ascii_case(ext)) {
            return true;
        }
    }
    matches_nexrad_filename(path) || is_ar2v(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn filename_pattern_matches_canonical() {
        assert!(matches_nexrad_filename(&PathBuf::from(
            "KLOT20260310_231412_V06"
        )));
        assert!(matches_nexrad_filename(&PathBuf::from(
            "/some/path/KATX20230520_201643"
        )));
    }

    #[test]
    fn filename_pattern_rejects_non_nexrad() {
        assert!(!matches_nexrad_filename(&PathBuf::from("foo.nc")));
        assert!(!matches_nexrad_filename(&PathBuf::from(
            "klot20260310_231412_V06" // lowercase prefix
        )));
        assert!(!matches_nexrad_filename(&PathBuf::from(
            "KLOT20260310-231412" // wrong separator
        )));
    }

    #[test]
    fn icao_from_filename_extracts_klot() {
        assert_eq!(
            icao_from_filename(&PathBuf::from("/x/KLOT20260310_231412_V06")),
            Some("KLOT")
        );
        assert_eq!(
            icao_from_filename(&PathBuf::from("KATX20230520_201643")),
            Some("KATX")
        );
    }

    #[test]
    fn icao_from_filename_rejects_non_nexrad() {
        assert_eq!(icao_from_filename(&PathBuf::from("klot...")), None);
        assert_eq!(icao_from_filename(&PathBuf::from("AAAAfoo")), None);
        assert_eq!(icao_from_filename(&PathBuf::from("KATX-not-nexrad")), None);
    }

    #[test]
    fn magic_check_returns_false_for_missing_file() {
        assert!(!is_ar2v(&PathBuf::from("/no/such/file/here.ar2v")));
    }
}
