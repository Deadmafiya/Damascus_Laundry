//! Golden-file replay test (Phase 6, plan 06-01).
//!
//! Locks the recon harness's output to a fixed FNV-1a 64 hash. If the
//! harness ever produces a different byte-stream for the same input,
//! the hash drifts and this test fails.
//!
//! The golden value is committed alongside the test. When intentionally
//! changing the harness, regenerate the golden and bump it.

use std::fs;
use std::path::PathBuf;

use dl_recon::fixture::{synthesize_pools, ReconFixture, SynthPoolSpec};
use dl_recon::pipeline::{replay_pools_to_ledger, ReplayParams};

/// Golden hash. Compute via:
///
///     cargo test -p dl-recon --test golden_replay -- --nocapture
///
/// and copy the printed `report_hash` here after a deliberate change.
const GOLDEN_HASH_TRIANGLE: u64 = 9_565_092_578_115_491_832;

fn specs_triangle() -> Vec<SynthPoolSpec> {
    vec![
        SynthPoolSpec {
            address: [1u8; 32],
            base_reserve: 1_000_000,
            quote_reserve: 1_000_000,
            fee_bps: 30,
        },
        SynthPoolSpec {
            address: [2u8; 32],
            base_reserve: 1_000_000,
            quote_reserve: 1_000_000,
            fee_bps: 30,
        },
        SynthPoolSpec {
            address: [3u8; 32],
            base_reserve: 1_000_000,
            quote_reserve: 1_100_000,
            fee_bps: 30,
        },
    ]
}

fn three_mints() -> Vec<[u8; 32]> {
    vec![[0xaa; 32], [0xbb; 32], [0xcc; 32]]
}

#[test]
fn golden_replay_triangle_matches() {
    let pools = synthesize_pools(&specs_triangle(), &three_mints());
    let params = ReplayParams::default();
    let report = replay_pools_to_ledger(&pools, &params).expect("replay");

    eprintln!("report_hash = {}", report.report_hash);
    eprintln!("cycle_records.len() = {}", report.cycle_records.len());
    eprintln!("divergences.len() = {}", report.divergences.len());

    // The fixture bundle produces a self-consistent (pools, capture, ledger) triple.
    let fx = ReconFixture::build(&specs_triangle(), &three_mints(), &params);
    assert!(fx.capture.starts_with(b"DLF-CAP1"));
    assert!(fx.ledger.starts_with(b"DLD-LDG1"));

    // Golden check: if we have a committed value, enforce it.
    if GOLDEN_HASH_TRIANGLE != 0 && report.report_hash != GOLDEN_HASH_TRIANGLE {
        panic!(
            "golden hash drift!\n  expected: {}\n  got:      {}\n\
             If this is an intentional harness change, run the test once with\n\
             GOLDEN_HASH_TRIANGLE = 0 to print the new value, then commit it.",
            GOLDEN_HASH_TRIANGLE, report.report_hash
        );
    }
}

#[test]
fn golden_replay_empty_pools_is_stable() {
    let params = ReplayParams::default();
    let report = replay_pools_to_ledger(&[], &params).expect("empty replay");
    // Empty pool set produces empty report + FNV offset.
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    assert_eq!(report.cycle_records.len(), 0);
    assert_eq!(report.divergences.len(), 0);
    assert_eq!(report.report_hash, FNV_OFFSET);
}

#[test]
fn fixture_round_trip_via_capture_path() {
    use dl_feed::capture::CapturedFeed;
    let fx = ReconFixture::build(&specs_triangle(), &three_mints(), &ReplayParams::default());
    // Open the capture as a CapturedFeed (its public type).
    let _feed: CapturedFeed<_> =
        CapturedFeed::open(fx.capture.as_slice()).expect("CapturedFeed::open");
}

#[test]
fn fixture_round_trip_via_ledger_reader() {
    use dl_ledger::LedgerReader;
    let fx = ReconFixture::build(&specs_triangle(), &three_mints(), &ReplayParams::default());
    let mut reader = LedgerReader::open(fx.ledger.as_slice()).expect("LedgerReader::open");
    let mut count = 0usize;
    while let Some(_entry) = reader.read_entry().expect("read") {
        count += 1;
    }
    eprintln!("ledger entries = {}", count);
}

#[test]
fn golden_file_on_disk_matches_runtime() {
    // Optional on-disk golden file. If `tests/fixtures/golden_triangle.hash`
    // exists, its content must equal the runtime hash. If absent, this
    // test is a no-op.
    let path = golden_path();
    if !path.exists() {
        return;
    }
    let on_disk = fs::read_to_string(&path)
        .expect("golden hash file readable")
        .trim()
        .parse::<u64>()
        .expect("golden hash file parses as u64");
    let pools = synthesize_pools(&specs_triangle(), &three_mints());
    let params = ReplayParams::default();
    let report = replay_pools_to_ledger(&pools, &params).expect("replay");
    assert_eq!(
        on_disk, report.report_hash,
        "on-disk golden {} != runtime {}; \
         update tests/fixtures/golden_triangle.hash if intentional",
        on_disk, report.report_hash
    );
}

fn golden_path() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests");
    p.push("fixtures");
    p.push("golden_triangle.hash");
    p
}
