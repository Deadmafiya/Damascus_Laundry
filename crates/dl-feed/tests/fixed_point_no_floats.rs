//! CI guard: dl-feed's value paths must be **float-free**.
//!
//! Why: any `f32` / `f64` in our event decoding, capture pipeline, or
//! replay logic would silently round-trip differently across machines
//! and break AC-1 (determinism). The display layer in `dl-core` is the
//! only place floats are allowed; this crate is on the value-path
//! side of that boundary.
//!
//! How it works: walks every `.rs` file under `../src/`, scans each
//! line for the bare tokens `f32` and `f64` *as type identifiers* (word
//! boundaries on both sides), and fails the test on any hit. Lines
//! that start with `//` (doc / line comments) are skipped.
//!
//! If you need a float here for a legitimate reason, push the value
//! through `dl-core::display` instead and exempt that path in the
//! allow-list below.
//!
//! Run with `cargo test -p dl-feed --test fixed_point_no_floats`.

use std::path::Path;

#[test]
fn no_floats_in_dl_feed_value_path() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut bad = Vec::new();
    visit(&root, &mut bad);
    assert!(bad.is_empty(), "f32/f64 found in dl-feed src: {bad:#?}");
}

fn visit(p: &Path, bad: &mut Vec<String>) {
    if p.is_file() {
        let s = std::fs::read_to_string(p).unwrap();
        for (i, line) in s.lines().enumerate() {
            let t = line.trim_start();
            if t.starts_with("//") {
                continue; // allow doc / line comments
            }
            if regex_lite_f32_f64(line) {
                bad.push(format!("{}:{}: {}", p.display(), i + 1, line));
            }
        }
    } else if p.is_dir() {
        for e in std::fs::read_dir(p).unwrap() {
            visit(&e.unwrap().path(), bad);
        }
    }
}

fn regex_lite_f32_f64(line: &str) -> bool {
    // Cheap, dependency-free: match word-boundary f32 / f64.
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'f' && i + 2 < bytes.len() {
            // Check "f32" or "f64" starting at i.
            let is_32 = bytes[i + 1] == b'3' && bytes[i + 2] == b'2';
            let is_64 = bytes[i + 1] == b'6' && bytes[i + 2] == b'4';
            if is_32 || is_64 {
                // boundary check: char before 'f' must not be alnum / '_'
                let before_ok =
                    i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
                let after_idx = i + 3;
                let after_ok = after_idx >= bytes.len()
                    || !(bytes[after_idx].is_ascii_alphanumeric() || bytes[after_idx] == b'_');
                if before_ok && after_ok {
                    return true;
                }
            }
        }
        i += 1;
    }
    false
}
