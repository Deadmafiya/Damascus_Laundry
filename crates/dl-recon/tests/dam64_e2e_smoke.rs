//! DAM-64 full end-to-end smoke test:
//! synth .dlg -> emit-reconciliation binary -> JSON -> dl-calibrate --from-reconciliation
//!
//! Run with: `cargo test -p dl-recon --test dam64_e2e_smoke -- --nocapture`

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

use dl_recon::fixture::{synthesize_pools, ReconFixture, SynthPoolSpec};
use dl_recon::pipeline::ReplayParams;

#[test]
fn full_pipeline_dlg_to_recon_to_calibration() {
    // 1. Build a synth .dlg.
    let specs = vec![
        SynthPoolSpec { address: [1u8; 32], base_reserve: 1_000_000, quote_reserve: 1_000_000, fee_bps: 30 },
        SynthPoolSpec { address: [2u8; 32], base_reserve: 1_000_000, quote_reserve: 1_000_000, fee_bps: 30 },
        SynthPoolSpec { address: [3u8; 32], base_reserve: 1_000_000, quote_reserve: 1_100_000, fee_bps: 30 },
    ];
    let mints = vec![[0xaa; 32], [0xbb; 32], [0xcc; 32]];
    let fx = ReconFixture::build(&specs, &mints, &ReplayParams::default());
    let tmpdir = std::env::temp_dir().join(format!("dam64-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&tmpdir).unwrap();
    let ledger_path: PathBuf = tmpdir.join("synth.dlg");
    let recon_json: PathBuf = tmpdir.join("recon.json");
    let cal_json: PathBuf = tmpdir.join("cal.json");
    std::fs::write(&ledger_path, &fx.ledger).expect("write .dlg");
    println!("[smoke] wrote {} ({} bytes)", ledger_path.display(), fx.ledger.len());

    // 2. Run emit-reconciliation binary.
    let emit = Command::new(option_env!("CARGO_BIN_EXE_emit-reconciliation")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/debug/emit-reconciliation")))
        .arg("--ledger").arg(&ledger_path)
        .arg("--out").arg(&recon_json)
        .output().expect("emit-reconciliation");
    assert!(emit.status.success(), "emit failed: {}", String::from_utf8_lossy(&emit.stderr));
    let recon_bytes = std::fs::read(&recon_json).expect("read recon.json");
    assert!(!recon_bytes.is_empty(), "recon.json is empty");
    println!("[smoke] wrote {} ({} bytes)", recon_json.display(), recon_bytes.len());

    // 3. Run dl-calibrate --from-reconciliation binary.
    let cal = Command::new(option_env!("CARGO_BIN_EXE_dl-calibrate")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/dam64-b/target/debug/dl-calibrate")))
        .arg("--from-reconciliation").arg(&recon_json)
        .arg("--out").arg(&cal_json)
        .output().expect("dl-calibrate");
    let cal_stdout = String::from_utf8_lossy(&cal.stdout);
    let cal_stderr = String::from_utf8_lossy(&cal.stderr);
    println!("[smoke] dl-calibrate stdout: {}", cal_stdout);
    println!("[smoke] dl-calibrate stderr: {}", cal_stderr);
    assert!(cal.status.success(), "dl-calibrate failed (status={}): stderr={}", cal.status, cal_stderr);
    assert!(cal_json.exists(), "cal.json not produced");

    // 4. Validate the calibration JSON is parseable and has the expected shape.
    let cal_text = std::fs::read_to_string(&cal_json).expect("read cal.json");
    let cal_val: serde_json::Value = serde_json::from_str(&cal_text).expect("parse cal.json");
    let result = cal_val.get("result").expect("cal.json has 'result'");
    let sample_size = result.get("sample_size").and_then(|v| v.as_u64()).expect("sample_size");
    assert!(sample_size > 0, "sample_size should be > 0, got {sample_size}");
    println!("[smoke] cal.json has sample_size={}", sample_size);

    // 5. Cleanup.
    let _ = std::fs::remove_dir_all(&tmpdir);
    println!("[smoke] PASS: full dlg -> recon -> cal pipeline works end-to-end");
}
