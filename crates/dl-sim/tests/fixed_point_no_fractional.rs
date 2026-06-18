//! Integer-only invariant guard for `dl-sim`.
//!
//! Phase 4 v1.0 contract: every fill, cost, sizing, and net-profit
//! computation is in `u64`/`u128`/`i128` fixed-point. No fractional
//! types anywhere in the value path.
//!
//! This test scans the crate's source for `f32` / `f64` /
//! `floating_point` and fails if any are found. The check is loose:
//! it flags substrings, so a comment mentioning "f64" would fail.
//! That's intentional — the rule is "don't even mention floats in the
//! value path code".
//!
//! Same pattern as `dl-detect/tests/fixed_point_no_floats.rs` and the
//! `no_floats_in_values` guard in `dl-state`.

use std::path::Path;

#[test]
fn dl_sim_has_no_floats_in_value_paths() {
    let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders: Vec<(String, String)> = Vec::new();

    fn visit(dir: &Path, offenders: &mut Vec<(String, String)>) {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for entry in rd.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    visit(&p, offenders);
                } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
                    if let Ok(content) = std::fs::read_to_string(&p) {
                        for needle in ["f32", "f64", "float", "Float"] {
                            for (lineno, line) in content.lines().enumerate() {
                                if line.contains(needle) {
                                    let rel = p
                                        .strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")))
                                        .unwrap_or(&p);
                                    offenders.push((
                                        format!("{}", rel.display()),
                                        format!(
                                            "line {}: contains '{}': {}",
                                            lineno + 1,
                                            needle,
                                            line.trim()
                                        ),
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    visit(&src_dir, &mut offenders);

    if !offenders.is_empty() {
        let msg: Vec<String> = offenders
            .into_iter()
            .map(|(p, l)| format!("  {} -> {}", p, l))
            .collect();
        panic!(
            "dl-sim value path must be float-free. Offenders:\n{}",
            msg.join("\n")
        );
    }
}
