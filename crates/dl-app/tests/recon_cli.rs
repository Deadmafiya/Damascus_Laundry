//! CLI integration tests for `dl-app recon` (Phase 6, plan 02).
//!
//! Drives the recon subcommand through `recon::run_dispatch` with a
//! synthetic capture and the synthetic anchor fixture. Verifies the
//! CLI returns the expected `ReconCliResult` shape.

use std::path::PathBuf;

use dl_app::recon::{run_dispatch, run_for_test, ReconCliResult};

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("crates")
        .join("dl-recon")
        .join("assets")
        .join("anchors.v0.jsonl")
}

fn synthetic_capture() -> Vec<u8> {
    // Empty capture — the harness produces an empty report, which
    // diverges from every anchor at -100%. This is a useful smoke test
    // that the CLI machinery wires up end-to-end.
    let mut sink = Vec::new();
    let mut w = dl_feed::capture::CaptureWriter::new(&mut sink).expect("writer");
    w.write_event(&dl_core::FeedEvent::Slot { slot: 1 })
        .expect("slot");
    w.into_inner().expect("flush");
    sink
}

#[test]
fn recon_cli_run_returns_divergences_for_empty_capture() {
    let capture = synthetic_capture();
    let result = run_for_test(capture, &fixture_path(), false);
    match result {
        ReconCliResult::Divergences(bad) => {
            assert!(
                !bad.is_empty(),
                "expected divergences against synthetic anchors"
            );
        }
        other => panic!("expected Divergences, got {other:?}"),
    }
}

#[test]
fn recon_cli_run_with_calibrate_returns_structured_result() {
    let capture = synthetic_capture();
    let result = run_for_test(capture, &fixture_path(), true);
    // Calibration runs but a totally empty engine can't match any
    // anchor at -100%, so the result is still Divergences. The
    // important thing is the call path works.
    match result {
        ReconCliResult::Divergences(_) | ReconCliResult::Ok => {}
        other => panic!("expected Ok or Divergences, got {other:?}"),
    }
}

#[test]
fn recon_cli_run_dispatch_argv() {
    // Drive the parse_args path with synthetic argv.
    let args = vec![
        "--capture".to_string(),
        "/nonexistent".to_string(),
        "--anchors".to_string(),
        fixture_path().to_string_lossy().to_string(),
    ];
    let result = run_dispatch(&args);
    // The capture path doesn't exist → Error path.
    match result {
        ReconCliResult::Error(msg) => {
            assert!(msg.contains("open capture"), "got: {msg}");
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn recon_cli_run_dispatch_missing_args() {
    let args = vec!["--anchors".to_string(), "/tmp/x".to_string()];
    let result = run_dispatch(&args);
    match result {
        ReconCliResult::Error(msg) => {
            assert!(
                msg.contains("--capture") || msg.contains("required"),
                "got: {msg}"
            );
        }
        other => panic!("expected Error, got {other:?}"),
    }
}
