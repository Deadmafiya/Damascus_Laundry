//! Integration test for the on-chain sweep (DAM-38 spec §3.4,
//! §3.5, §4) — the recorded-response path.
//!
//! Acceptance: "Integration test using a captured bundle fixture
//! (or a recorded response, if no live)." (DAM-102 AC #1.)
//!
//! Plays back a JSON fixture through an in-memory `BundleFetcher`
//! and asserts the five spec §3.5 divergence categories all fire
//! correctly. The fixture is hand-authored and lives in this file
//! so it can be re-recorded when a real mainnet bundle lands
//! (DAM-21 dry-run window).

use std::collections::HashMap;
use std::sync::Arc;

use dl_ledger::entry::Decision;
use dl_recon::onchain_sweep::{
    classify, sweep, write_onchain_sweep_json, BundleFetch, BundleFetcher,
    CycleOnchainDivergenceKind, OnchainSweepError, OnchainSweepReport,
    TIP_DRIFT_TOLERANCE_LAMPORTS,
};
use dl_recon::reconcile::ReconRow;

#[derive(Debug, Default, Clone)]
struct RecordedFetcher {
    responses: HashMap<String, BundleFetch>,
}

impl RecordedFetcher {
    fn from_json(json: &str) -> Self {
        let mut responses = HashMap::new();
        let v: serde_json::Value =
            serde_json::from_str(json).expect("fixture must be valid JSON");
        let obj = v.as_object().expect("fixture must be a JSON object");
        for (sig, body) in obj {
            let f: BundleFetch = serde_json::from_value(body.clone())
                .expect("fixture entry must deserialize as BundleFetch");
            responses.insert(sig.clone(), f);
        }
        Self { responses }
    }
}

impl BundleFetcher for RecordedFetcher {
    fn fetch(&self, signature: &str) -> Result<BundleFetch, OnchainSweepError> {
        match self.responses.get(signature) {
            Some(f) => Ok(f.clone()),
            None => Err(OnchainSweepError::MissingSignature(
                signature.to_string(),
                "not in fixture".to_string(),
            )),
        }
    }
}

fn paper_row(seq: u64, hash: u64, tip: u64, pnl: i128, decision: Decision) -> ReconRow {
    ReconRow {
        seq,
        cycle_hash: hash,
        predicted_lamports: pnl,
        realized_lamports: pnl,
        delta_lamports: 0,
        decision,
        tip_lamports: tip,
    }
}

fn real_fixture() -> String {
    let s88 = "5".repeat(88);
    format!(
        r#"{{
  "1229782938247303441": {{
    "pre_balance": 1000000000, "post_balance": 1050000000, "funded_amount": 0,
    "tip_lamports": 12000, "slot": 350000000,
    "tx_signature": "{s88}", "landed": true, "reverted": false
  }},
  "2459565876494606882": {{
    "pre_balance": 1000000000, "post_balance": 970000000, "funded_amount": 0,
    "tip_lamports": 10000, "slot": 350000001,
    "tx_signature": "{s88}", "landed": true, "reverted": false
  }},
  "3689348814741910323": {{
    "pre_balance": 1000000000, "post_balance": 1030000000, "funded_amount": 0,
    "tip_lamports": 0, "slot": 350000002,
    "tx_signature": "{s88}", "landed": true, "reverted": false
  }},
  "4919131752989213764": {{
    "pre_balance": 1000000000, "post_balance": 1000000000, "funded_amount": 0,
    "tip_lamports": 10000, "slot": 350000003,
    "tx_signature": "{s88}", "landed": true, "reverted": true
  }}
}}"#
    )
}

#[test]
fn sweep_classifies_all_five_divergence_kinds() {
    let fetcher = RecordedFetcher::from_json(&real_fixture());
    let rows = vec![
        paper_row(0, 0x1111_1111_1111_1111u64, 10_000, 50_000, Decision::WouldTrade),
        paper_row(1, 0x2222_2222_2222_2222u64, 10_000, 50_000, Decision::WouldTrade),
        paper_row(2, 0x3333_3333_3333_3333u64, 0, 0, Decision::WouldTrade),
        paper_row(3, 0x4444_4444_4444_4444u64, 10_000, 50_000, Decision::WouldTrade),
        paper_row(4, 0x9999_9999_9999_9999u64, 10_000, 50_000, Decision::WouldTrade),
    ];

    let report = sweep(&rows, &fetcher);

    let d = &report.divergences;
    assert_eq!(d.get("tip_drift").copied(), Some(1), "tip_drift");
    assert_eq!(d.get("simulation_lied_yes").copied(), Some(1), "simulation_lied_yes");
    assert_eq!(d.get("simulation_lied_no").copied(), Some(1), "simulation_lied_no");
    assert_eq!(d.get("reverted_after_ok").copied(), Some(1), "reverted_after_ok");
    assert_eq!(d.get("missing_signature").copied(), Some(1), "missing_signature");

    assert_eq!(report.bundles_submitted, 5);
    assert_eq!(report.bundles_landed, 4);
    assert_eq!(report.gross_pnl_lamports, 50_000_000);
    assert_eq!(report.tip_paid_lamports, 32_000);
    assert_eq!(report.revert_cost_lamports, 0);
    assert_eq!(report.net_pnl_lamports, 49_968_000);

    assert_eq!(report.per_cycle.len(), 5);
    for (i, row) in report.per_cycle.iter().enumerate() {
        assert_eq!(row.seq, i as u64);
    }
}

#[test]
fn tip_drift_threshold_is_the_spec_value() {
    assert_eq!(TIP_DRIFT_TOLERANCE_LAMPORTS, 1_000);
}

#[test]
fn missing_signature_zeroes_realized_fields() {
    let fetcher = RecordedFetcher::default();
    let paper = paper_row(0, 0xdead_beef, 10_000, 50_000, Decision::WouldTrade);
    let row = sweep(&[paper], &fetcher).per_cycle.into_iter().next().unwrap();
    assert!(row.divergences.contains(&CycleOnchainDivergenceKind::MissingSignature));
    assert_eq!(row.realized_tip_lamports, 0);
    assert_eq!(row.realized_pnl_lamports, 0);
    assert!(!row.landed);
    assert!(row.tx_signature.is_empty());
}

#[test]
fn would_not_trade_rows_are_recorded_without_fetch() {
    struct PanickingFetcher;
    impl BundleFetcher for PanickingFetcher {
        fn fetch(&self, _: &str) -> Result<BundleFetch, OnchainSweepError> {
            panic!("fetcher must not be called for WouldNotTrade rows");
        }
    }
    let fetcher = PanickingFetcher;
    let rows = vec![paper_row(0, 0xdead, 0, -10_000, Decision::WouldNotTrade)];
    let report = sweep(&rows, &fetcher);
    assert_eq!(report.bundles_submitted, 0);
    assert_eq!(report.bundles_landed, 0);
    assert_eq!(report.per_cycle.len(), 1);
    assert!(report.per_cycle[0].divergences.is_empty());
}

#[test]
fn json_output_has_spec_section_4_shape() {
    let fetcher = RecordedFetcher::from_json(&real_fixture());
    let rows = vec![
        paper_row(0, 0x1111_1111_1111_1111u64, 10_000, 50_000, Decision::WouldTrade),
        paper_row(1, 0x2222_2222_2222_2222u64, 10_000, 50_000, Decision::WouldTrade),
    ];
    let report = sweep(&rows, &fetcher);

    let mut buf = Vec::new();
    write_onchain_sweep_json(&mut buf, &report).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();

    for key in [
        "bundles_submitted",
        "bundles_landed",
        "gross_pnl_lamports",
        "tip_paid_lamports",
        "rpc_cost_lamports",
        "revert_cost_lamports",
        "net_pnl_lamports",
        "per_cycle",
        "divergences",
    ] {
        assert!(v.get(key).is_some(), "missing spec §4 key: {key}");
    }
    let divs = v.get("divergences").and_then(|x| x.as_object()).unwrap();
    for k in [
        "tip_drift",
        "simulation_lied_yes",
        "simulation_lied_no",
        "reverted_after_ok",
        "missing_signature",
    ] {
        assert!(divs.get(k).is_some(), "missing spec §4 divergence key: {k}");
    }
    let per_cycle = v.get("per_cycle").and_then(|x| x.as_array()).unwrap();
    assert_eq!(per_cycle.len(), 2);
    assert_eq!(per_cycle[0].get("seq").and_then(|x| x.as_u64()), Some(0));
    assert_eq!(per_cycle[1].get("seq").and_then(|x| x.as_u64()), Some(1));
}

#[test]
fn per_cycle_row_has_spec_4_field_set() {
    let fetcher = RecordedFetcher::from_json(&real_fixture());
    let rows = vec![paper_row(
        0,
        0x1111_1111_1111_1111u64,
        10_000,
        50_000,
        Decision::WouldTrade,
    )];
    let report = sweep(&rows, &fetcher);
    let row = &report.per_cycle[0];
    let v = serde_json::to_value(row).unwrap();
    for key in [
        "seq",
        "cycle_hash",
        "paper_tip_lamports",
        "realized_tip_lamports",
        "paper_pnl_lamports",
        "realized_pnl_lamports",
        "landed",
        "reverted",
        "slot",
        "tx_signature",
        "divergences",
    ] {
        assert!(v.get(key).is_some(), "missing per_cycle key: {key}");
    }
}

#[test]
fn arc_fetcher_works_with_dyn_dispatch() {
    let fetcher: Arc<dyn BundleFetcher> =
        Arc::new(RecordedFetcher::from_json(&real_fixture()));
    let rows = vec![paper_row(
        0,
        0x1111_1111_1111_1111u64,
        10_000,
        50_000,
        Decision::WouldTrade,
    )];
    let report = sweep(&rows, fetcher.as_ref());
    assert_eq!(report.bundles_submitted, 1);
    assert_eq!(report.bundles_landed, 1);
    assert_eq!(report.divergences.get("tip_drift").copied(), Some(1));
}

#[test]
fn empty_sweep_emits_clean_report() {
    let fetcher = RecordedFetcher::default();
    let report = sweep(&[], &fetcher);
    let r: OnchainSweepReport = report;
    assert_eq!(r.bundles_submitted, 0);
    assert_eq!(r.bundles_landed, 0);
    assert_eq!(r.per_cycle.len(), 0);
    for (_k, v) in r.divergences {
        assert_eq!(v, 0);
    }
}

#[test]
fn classify_unit_path_matches_sweep_path() {
    let fetcher = RecordedFetcher::from_json(&real_fixture());
    let paper = paper_row(
        0,
        0x1111_1111_1111_1111u64,
        10_000,
        50_000,
        Decision::WouldTrade,
    );
    let sweep_row = sweep(&[paper.clone()], &fetcher)
        .per_cycle
        .into_iter()
        .next()
        .unwrap();
    let fetch = fetcher.fetch(&paper.cycle_hash.to_string()).unwrap();
    let direct_row = classify(&paper, Ok(&fetch));
    assert_eq!(sweep_row.divergences, direct_row.divergences);
    assert_eq!(sweep_row.realized_tip_lamports, direct_row.realized_tip_lamports);
    assert_eq!(sweep_row.realized_pnl_lamports, direct_row.realized_pnl_lamports);
}
