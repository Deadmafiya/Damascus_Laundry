//! Float-free CI guard for `dl-pipeline` (DAM-46, integer-only invariant).
//!
//! Scans every `.rs` file under `src/` and `tests/` for the strings
//! `f32` or `f64` *as type tokens*. A hit fails the build because
//! the pipeline must remain integer-only in every value path; the
//! only module allowed to touch floats is the read-only query layer
//! (`src/query/mod.rs`).
//!
//! Mirrors the `dl-recon/tests/floats.rs` guard.

use std::fs;
use std::path::Path;

const ROOTS: &[&str] = &["src", "tests"];

const ALLOWED_FILES: &[&str] = &["src/query/mod.rs"];

fn walk(p: &Path, files: &mut Vec<std::path::PathBuf>) {
    if p.is_file() {
        if p.extension().and_then(|s| s.to_str()) == Some("rs") {
            files.push(p.to_path_buf());
        }
        return;
    }
    if let Ok(rd) = fs::read_dir(p) {
        for entry in rd.flatten() {
            walk(&entry.path(), files);
        }
    }
}

#[test]
fn no_floats_in_pipeline_value_path() {
    let mut files = Vec::new();
    for root in ROOTS {
        let p = Path::new(root);
        if p.exists() {
            walk(p, &mut files);
        }
    }
    assert!(!files.is_empty(), "no .rs files found under src/ or tests/");

    let self_name = Path::new(file!()).file_name().unwrap().to_owned();
    for f in &files {
        if f.file_name() == Some(&self_name) {
            continue;
        }
        let canonical = f.to_string_lossy().replace('\\', "/");
        if ALLOWED_FILES.iter().any(|a| canonical.ends_with(a)) {
            continue;
        }
        let content = fs::read_to_string(f).expect("read source");
        for (lineno, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") {
                continue;
            }
            let has_f32 = is_bare_token(line, "f32");
            let has_f64 = is_bare_token(line, "f64");
            if has_f32 || has_f64 {
                panic!(
                    "float token found in {}:{}: {}",
                    f.display(),
                    lineno + 1,
                    line.trim()
                );
            }
        }
    }
}

fn is_bare_token(line: &str, token: &str) -> bool {
    let bytes = line.as_bytes();
    let t = token.as_bytes();
    let mut start = 0;
    while let Some(idx) = line[start..].find(token) {
        let abs = start + idx;
        let before_ok =
            abs == 0 || !(bytes[abs - 1].is_ascii_alphanumeric() || bytes[abs - 1] == b'_');
        let after_abs = abs + t.len();
        let after_ok = after_abs >= bytes.len()
            || !(bytes[after_abs].is_ascii_alphanumeric() || bytes[after_abs] == b'_');
        if before_ok && after_ok {
            return true;
        }
        start = abs + 1;
    }
    false
}

#[test]
fn doc_mentions_integer_only_invariant() {
    let lib_rs = fs::read_to_string("src/lib.rs").expect("read lib.rs");
    assert!(
        lib_rs.to_lowercase().contains("integer-only"),
        "lib.rs doc must mention the integer-only invariant"
    );
}

#[test]
fn is_bare_token_distinguishes_u32_and_f32() {
    assert!(is_bare_token("let x: f32 = 0;", "f32"));
    assert!(!is_bare_token("let x: u32 = 0;", "f32"));
    assert!(!is_bare_token("u32::MAX", "f32"));
    assert!(is_bare_token("f64::consts::PI", "f64"));
    assert!(!is_bare_token("u64::MAX", "f64"));
}
