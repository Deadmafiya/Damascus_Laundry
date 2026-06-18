//! Integer-only guard for `dl-ledger`.
//!
//! Mirrors the `fixed_point_no_fractional` guards in dl-feed, dl-state,
//! dl-detect, dl-sim. Asserts that the value path (everything except
//! `format::format_spec`, the human-readable test doc) is free of
//! fractional types — no `f32`, no `f64`, no `bf16`, no `f16`.
//!
//! Doc comments and the spec string itself may mention these words
//! (they appear in the float-free justification), so the test scans
//! `*.rs` files in `src/`, excluding `format_spec`'s return value
//! (which lives in `format.rs` and would naturally mention "magic"
//! etc.).
//!
//! This test does *not* check the format spec string — that's covered
//! by `ledger_roundtrip::format_spec_locks_key_fields`.

use std::fs;
use std::path::Path;

const FORBIDDEN: &[&str] = &["f32", "f64", "f16", "bf16"];

#[test]
fn no_fractional_types_in_dl_ledger_src() {
    let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders: Vec<(String, String)> = Vec::new();

    fn walk(dir: &Path, offenders: &mut Vec<(String, String)>) {
        if !dir.is_dir() {
            return;
        }
        for entry in fs::read_dir(dir).expect("read_dir") {
            let entry = entry.expect("entry");
            let p = entry.path();
            if p.is_dir() {
                walk(&p, offenders);
            } else if p.extension().map(|e| e == "rs").unwrap_or(false) {
                let text = fs::read_to_string(&p).expect("read source");
                for (lineno, line) in text.lines().enumerate() {
                    // Skip doc comments — they are allowed to name
                    // forbidden types when explaining why those types
                    // are forbidden. The guard checks the *code*, not
                    // the explanation.
                    let trimmed = line.trim_start();
                    if trimmed.starts_with("//!") || trimmed.starts_with("///") {
                        continue;
                    }
                    for token in FORBIDDEN {
                        if line.contains(token) {
                            offenders.push((
                                p.display().to_string(),
                                format!(
                                    "line {}: token `{}` in `{}`",
                                    lineno + 1,
                                    token,
                                    line.trim()
                                ),
                            ));
                        }
                    }
                }
            }
        }
    }

    walk(&src_dir, &mut offenders);
    assert!(
        offenders.is_empty(),
        "dl-ledger src/ contains fractional types:\n{}",
        offenders
            .iter()
            .map(|(p, s)| format!("  {}: {}", p, s))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn this_file_is_under_tests_dir() {
    // Sanity: this test lives in tests/, not src/. The lint above
    // walks src/ only.
    let this = file!();
    assert!(this.contains("tests/"));
}
