//! Numbered invariants for the recon harness (Phase 6, plan 06-01).
//!
//! Each `check_iN_*` function asserts one invariant and returns `Ok(())`
//! if it holds, or an error string if it doesn't. The integration
//! tests in `tests/dst_faults.rs` and the golden-replay test in
//! `tests/golden_replay.rs` call these directly; CI also surfaces them
//! via the float-free guard in `tests/floats.rs`.
//!
//! ## The six invariants
//!
//! - **I-1 Determinism**: identical `(pools, params)` produce a
//!   byte-identical report (same hash).
//! - **I-2 Integer-only**: no `f32` / `f64` in value paths.
//!   Verified by `tests/floats.rs` (sub-string scan), not here.
//! - **I-3 Schema enforcement**: opening a ledger with a different
//!   schema version is a hard error.
//! - **I-4 No silent skips**: every detected divergence is recorded.
//! - **I-5 EOF terminates**: the reader returns `None` at EOF; no
//!   terminator frame is required or expected.
//! - **I-6 Hash match**: the recorded hash in a `ReconReport` body
//!   matches the recomputed hash.

use std::io::Write;

use dl_ledger::{LedgerError, LedgerReader, LedgerWriter, LEDGER_MAGIC, LEDGER_SCHEMA_VERSION};

use crate::pipeline::{replay_pools_to_ledger, ReconReport, ReplayParams};
use crate::ReconError;
use dl_state::Pool;

// ---------------------------------------------------------------------------
// I-1: Determinism
// ---------------------------------------------------------------------------

/// **I-1**: Two replays of `(pools, params)` produce byte-identical
/// [`ReconReport`]s. Asserts `report_a.report_hash == report_b.report_hash`
/// AND `report_a == report_b`.
pub fn check_i1_determinism(pools: &[Pool], params: &ReplayParams) -> Result<(), String> {
    let a = replay_pools_to_ledger(pools, params).map_err(|e| format!("replay_a: {}", e))?;
    let b = replay_pools_to_ledger(pools, params).map_err(|e| format!("replay_b: {}", e))?;
    if a.report_hash != b.report_hash {
        return Err(format!(
            "report_hash diverged across replays: {} vs {}",
            a.report_hash, b.report_hash
        ));
    }
    if a != b {
        return Err("reports are not element-wise equal across replays".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// I-2: Integer-only
// ---------------------------------------------------------------------------

/// **I-2**: a static check. The `tests/floats.rs` integration test
/// greps the entire crate for `f32` / `f64` and panics on a hit.
/// This function is here as documentation; it returns `Ok(())`
/// unconditionally so callers can wire it into CI runners.
pub fn check_i2_integer_only() -> Result<(), String> {
    Ok(())
}

// ---------------------------------------------------------------------------
// I-3: Schema enforcement
// ---------------------------------------------------------------------------

/// **I-3**: a ledger with `LEDGER_SCHEMA_VERSION + 1` is rejected by
/// [`LedgerReader::open`] with [`LedgerError::SchemaMismatch`].
pub fn check_i3_schema_enforcement() -> Result<(), String> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(LEDGER_MAGIC);
    bytes.extend_from_slice(&(LEDGER_SCHEMA_VERSION + 1).to_le_bytes());
    let err = LedgerReader::open(bytes.as_slice())
        .err()
        .ok_or_else(|| "reader accepted future-schema ledger".to_string())?;
    match err {
        LedgerError::SchemaMismatch { found, expected } => {
            if found != LEDGER_SCHEMA_VERSION + 1 || expected != LEDGER_SCHEMA_VERSION {
                return Err(format!(
                    "schema mismatch reported wrong values: found={} expected={}",
                    found, expected
                ));
            }
            Ok(())
        }
        other => Err(format!("expected SchemaMismatch, got {:?}", other)),
    }
}

/// **I-3 (bad magic)**: a file whose first 8 bytes are not
/// [`LEDGER_MAGIC`] is rejected with [`LedgerError::BadMagic`].
pub fn check_i3_bad_magic_rejected() -> Result<(), String> {
    let bytes = b"NOTAMAGI";
    match LedgerReader::open(&bytes[..]) {
        Err(LedgerError::BadMagic) => Ok(()),
        other => Err(format!("expected BadMagic, got {:?}", other)),
    }
}

// ---------------------------------------------------------------------------
// I-4: No silent skips
// ---------------------------------------------------------------------------

/// **I-4**: A [`ReconReport`]'s `divergences` field is the *complete*
/// set of mismatches against the source ledger (06-02 will fill it).
/// In 06-01 the field is always empty. This check ensures that no
/// `Divergence` is silently dropped by the reporting path: a fresh
/// report's divergences vec is empty iff there were no mismatches.
pub fn check_i4_no_silent_skips(report: &ReconReport) -> Result<(), String> {
    // 06-01 invariant: divergences must be empty (single source of
    // truth). 06-02 will extend this to assert every source-ledger
    // entry has a corresponding divergence-or-match in the report.
    if !report.divergences.is_empty() {
        return Err(format!(
            "06-01: expected empty divergences, got {}",
            report.divergences.len()
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// I-5: EOF terminates
// ---------------------------------------------------------------------------

/// **I-5**: After writing N entries to a ledger via [`LedgerWriter`],
/// the matching reader returns exactly N entries then `Ok(None)`.
/// No terminator frame is required.
pub fn check_i5_eof_terminates(n: usize) -> Result<(), String> {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut w = LedgerWriter::new(&mut buf).map_err(|e| format!("writer open: {}", e))?;
        for seq in 0..n as u64 {
            let entry = dummy_entry(seq);
            w.write_entry(&entry)
                .map_err(|e| format!("write {}: {}", seq, e))?;
        }
        w.into_inner()
            .map_err(|e: LedgerError| format!("writer flush: {}", e))?;
    }
    let mut r = LedgerReader::open(buf.as_slice()).map_err(|e| format!("reader open: {}", e))?;
    let mut got = 0usize;
    while let Some(_entry) = r.read_entry().map_err(|e| format!("read: {}", e))? {
        got += 1;
    }
    if got != n {
        return Err(format!("reader yielded {} entries, expected {}", got, n));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// I-6: Hash match
// ---------------------------------------------------------------------------

/// **I-6**: The `report_hash` recorded in the [`ReconReport`] body
/// matches the hash recomputed from `cycle_records`. The harness
/// recomputes on every read; if a tampering bug (or a serialization
/// issue) caused a drift, this surfaces it.
pub fn check_i6_hash_match(report: &ReconReport) -> Result<(), String> {
    // We can't directly call the private `hash_records` from
    // `pipeline.rs`. Re-derive the hash via the same FNV-1a 64 walk
    // over the bincode of each entry. If this ever drifts, it's a
    // genuine implementation bug.
    let computed = recompute_hash(&report.cycle_records);
    if computed != report.report_hash {
        return Err(format!(
            "hash drift: recorded {} vs computed {}",
            report.report_hash, computed
        ));
    }
    Ok(())
}

/// Public helper: re-hash a slice of records the same way
/// `pipeline::hash_records` does. Mirrors the FNV-1a 64 algorithm.
pub fn recompute_hash(records: &[crate::pipeline::CycleRecord]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = OFFSET;
    for rec in records {
        for byte in rec.seq.to_le_bytes() {
            h ^= byte as u64;
            h = h.wrapping_mul(PRIME);
        }
        let bytes = bincode::serialize(&rec.entry).expect("bincode serialize");
        for byte in bytes {
            h ^= byte as u64;
            h = h.wrapping_mul(PRIME);
        }
    }
    h
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn dummy_entry(seq: u64) -> dl_ledger::LedgerEntry {
    use dl_ledger::{Decision, LedgerHash};
    use dl_sim::cost::CostBreakdown;
    use dl_sim::ev::ExpectedValue;
    use dl_sim::net_profit::NetProfit;

    let one = dl_sim::ev::Prob::from_scaled_clamped(1_000_000_000_000_000_000);
    let zero_costs = CostBreakdown {
        base_sig_fee_lamports: 0,
        priority_fee_lamports: 0,
        jito_tip_lamports: 0,
        jito_tip_fee_lamports: 0,
        total_lamports: 0,
    };
    let net = NetProfit {
        input_amount: 0,
        gross_output: 0,
        total_costs: zero_costs,
        net_profit: 0,
        net_profit_bps: 0,
        profitable: false,
    };
    let ev = ExpectedValue {
        e_pnl: 0,
        p_detect: one,
        p_win: one,
        p_land: one,
        expected_failed_cost: 0,
    };
    dl_ledger::LedgerEntry {
        seq,
        entry_id: seq,
        cycle_hash: LedgerHash(0xdead_beef),
        net,
        optimistic: ev,
        conservative: ev,
        decision: Decision::WouldNotTrade,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use dl_state::pool::{AmmKind, Pool};
    use dl_state::Pubkey;

    fn empty_pools() -> Vec<Pool> {
        vec![]
    }

    fn one_pool(addr: [u8; 32], reserves: (u64, u64)) -> Pool {
        Pool {
            address: Pubkey(addr),
            kind: AmmKind::RaydiumAmmV4,
            base_mint: Pubkey([0xaa; 32]),
            quote_mint: Pubkey([0xbb; 32]),
            base_decimals: 9,
            quote_decimals: 6,
            base_reserve: reserves.0,
            quote_reserve: reserves.1,
            fee_bps: 30,
            last_update_slot: 1,
        }
    }

    #[test]
    fn i1_determinism_holds_for_empty_pools() {
        check_i1_determinism(&empty_pools(), &ReplayParams::default()).unwrap();
    }

    #[test]
    fn i1_determinism_holds_for_single_pool() {
        let pools = vec![one_pool([1u8; 32], (1_000_000, 1_000_000))];
        check_i1_determinism(&pools, &ReplayParams::default()).unwrap();
    }

    #[test]
    fn i3_schema_enforcement() {
        check_i3_schema_enforcement().unwrap();
        check_i3_bad_magic_rejected().unwrap();
    }

    #[test]
    fn i4_no_silent_skips_on_empty_report() {
        let params = ReplayParams::default();
        let report = replay_pools_to_ledger(&empty_pools(), &params).unwrap();
        check_i4_no_silent_skips(&report).unwrap();
    }

    #[test]
    fn i5_eof_terminates_at_various_sizes() {
        for n in [0usize, 1, 2, 5, 16] {
            check_i5_eof_terminates(n).unwrap();
        }
    }

    #[test]
    fn i6_hash_match_round_trips() {
        let params = ReplayParams::default();
        let report = replay_pools_to_ledger(&empty_pools(), &params).unwrap();
        check_i6_hash_match(&report).unwrap();
    }
}

/// `ReconError` is re-exported at the crate root, so downstream tests
/// can convert from the underlying `LedgerError` via `?`.
#[allow(dead_code)]
fn _ensure_reexport(e: ReconError) -> LedgerError {
    match e {
        ReconError::Ledger(l) => l,
        _ => LedgerError::Truncated,
    }
}

/// Write `data` to a sink; kept here so callers can use the same
/// `Write` impl pattern when extending the harness.
#[allow(dead_code)]
pub fn write_all_sink<W: Write>(mut sink: W, data: &[u8]) -> std::io::Result<()> {
    sink.write_all(data)
}
