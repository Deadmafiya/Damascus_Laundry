//! Float-free CI guard for `dl-signer`.
//!
//! Allows `ratelimit.rs` to use `f64` (the only float in the workspace's
//! value path; the alternative integer token-bucket would be more code
//! for the same correctness). Asserts no other floats exist.

use std::path::Path;

#[test]
fn no_floats_in_dl_signer_value_path() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut bad = Vec::new();
    visit(&root, &mut bad);
    assert!(bad.is_empty(), "unexpected f32/f64 in dl-signer: {bad:#?}");
}

fn visit(p: &Path, bad: &mut Vec<String>) {
    if p.is_file() {
        let s = std::fs::read_to_string(p).unwrap();
        // ratelimit.rs is the one allowed site.
        if p.ends_with("ratelimit.rs") {
            return;
        }
        for (i, line) in s.lines().enumerate() {
            let t = line.trim_start();
            if t.starts_with("//") {
                continue; // allow doc / line comments
            }
            if has_f32_f64(line) {
                bad.push(format!("{}:{}: {}", p.display(), i + 1, line));
            }
        }
    } else if p.is_dir() {
        for e in std::fs::read_dir(p).unwrap() {
            visit(&e.unwrap().path(), bad);
        }
    }
}

fn has_f32_f64(line: &str) -> bool {
    // Match `f32` / `f64` as bare tokens, not inside identifiers or
    // string literals. Simple regex-free scanner.
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'f' && i + 2 < bytes.len() {
            let kind = &bytes[i + 1..i + 3];
            if kind == b"32" || kind == b"64" {
                // Word boundary on both sides.
                let left_ok = i == 0 || !is_ident_byte(bytes[i - 1]);
                let right_ok = i + 3 == bytes.len() || !is_ident_byte(bytes[i + 3]);
                if left_ok && right_ok {
                    return true;
                }
            }
        }
        i += 1;
    }
    false
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}
