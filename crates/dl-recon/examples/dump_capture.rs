//! Dump a synthetic `.dlf` capture to disk for end-to-end testing
//! of `dl-calibration --from-capture`.
//!
//! Usage:
//!   cargo run --release -p dl-recon --example dump_capture -- /tmp/example.dlf
use std::io::Write;
use std::path::PathBuf;

use dl_recon::fixture::{synthesize_small_capture, SynthPoolSpec};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = PathBuf::from(
        args.get(1)
            .cloned()
            .unwrap_or_else(|| "/tmp/damascus_example.dlf".to_string()),
    );
    // Build a richer multi-cycle universe: 3 distinct triangles,
    // each with a slight reserve skew so the cycle detector finds
    // an arbitrage loop. The DAM-35 acceptance criterion is to
    // produce a calibration report with `n > 0` captures.
    let mut specs = Vec::new();
    let mut mints: Vec<[u8; 32]> = Vec::new();
    // Triangle 1: 1, 2, 3 → mints aa, bb, cc
    for i in 0u8..3 {
        specs.push(SynthPoolSpec {
            address: [i + 1; 32],
            base_reserve: 1_000_000,
            quote_reserve: 1_000_000 + (i as u64 + 1) * 50_000, // increasing skew → arbs
            fee_bps: 30,
        });
        mints.push([0xaa + i; 32]);
    }
    // Triangle 2: 4, 5, 6 → mints dd, ee, ff
    for i in 0u8..3 {
        specs.push(SynthPoolSpec {
            address: [i + 4; 32],
            base_reserve: 1_000_000,
            quote_reserve: 1_000_000 + (i as u64 + 1) * 75_000,
            fee_bps: 30,
        });
        mints.push([0xdd + i; 32]);
    }
    // Triangle 3: 7, 8, 9 → mints gg, hh, ii
    for i in 0u8..3 {
        specs.push(SynthPoolSpec {
            address: [i + 7; 32],
            base_reserve: 1_000_000,
            quote_reserve: 1_000_000 + (i as u64 + 1) * 25_000,
            fee_bps: 30,
        });
        mints.push([0x11 + i; 32]);
    }
    // Build mints cleanly
    mints.clear();
    mints.extend([[0xaa; 32], [0xbb; 32], [0xcc; 32], [0xdd; 32], [0xee; 32], [0xff; 32],
                  [0x11; 32], [0x22; 32], [0x33; 32]]);
    let capture = synthesize_small_capture(&specs, &mints);
    let mut f = std::fs::File::create(&out).expect("create out file");
    f.write_all(&capture).expect("write capture");
    eprintln!("dump_capture: wrote {} bytes to {} ({} pools)", capture.len(), out.display(), specs.len());
}
