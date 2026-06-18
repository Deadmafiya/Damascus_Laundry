//! Integration tests for `dl_recon::onchain` (Phase 6, plan 02).
//!
//! Drives `AnchorDataset::load_jsonl` against the synthetic fixture
//! and asserts the schema round-trips, all six anchors load, and a
//! known-empty `ReconReport` produces the expected divergence vector.

use std::path::PathBuf;

use dl_recon::onchain::{AnchorDataset, AnchorName};
use dl_recon::pipeline::{replay_pools_to_ledger, ReplayParams};

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("anchors.v0.jsonl")
}

#[test]
fn load_synthetic_fixture_has_all_six_anchors() {
    let ds = AnchorDataset::load_jsonl(&fixture_path()).expect("load fixture");
    assert_eq!(ds.entries.len(), 6);
    for name in [
        AnchorName::AttemptCount,
        AnchorName::LandedArbCount,
        AnchorName::MeanTipLamports,
        AnchorName::MedianWinnerPnlSol,
        AnchorName::P95WinnerPnlSol,
        AnchorName::TipAsPctOfMev,
    ] {
        assert!(ds.get(name).is_some(), "missing anchor {:?}", name);
    }
}

#[test]
fn empty_report_diverges_on_attempt_count() {
    let ds = AnchorDataset::load_jsonl(&fixture_path()).expect("load fixture");
    // No pools ⇒ no cycles ⇒ empty report.
    let pools: Vec<dl_state::Pool> = Vec::new();
    let report = replay_pools_to_ledger(&pools, &ReplayParams::default()).expect("replay");
    let divs = ds.compare(&report).expect("compare");
    // AttemptCount anchor = 5_123_847. Report = 0. Divergence should
    // be -10_000 bps (i.e. -100%, exceeds 5% tolerance).
    let attempt = divs
        .iter()
        .find(|d| d.name == AnchorName::AttemptCount)
        .expect("attempt div");
    assert_eq!(attempt.divergence_bps, -10_000);
    assert!(attempt.exceeds_tolerance);
}

#[test]
fn attempt_count_anchor_matches_synthetic_value() {
    let ds = AnchorDataset::load_jsonl(&fixture_path()).expect("load fixture");
    let entry = ds.get(AnchorName::AttemptCount).expect("entry");
    assert_eq!(entry.value, 5_123_847);
    assert_eq!(entry.unit, "bundles");
    assert_eq!(entry.source, "jito-bot.constants.v0+helius-report.2025");
}

#[test]
fn landed_arb_count_tolerance_is_5_percent() {
    let ds = AnchorDataset::load_jsonl(&fixture_path()).expect("load fixture");
    let entry = ds.get(AnchorName::LandedArbCount).expect("entry");
    assert_eq!(entry.name.tolerance_bps(), 500);
}
