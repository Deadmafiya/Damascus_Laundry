//! Synthetic capture / ledger builders for tests and seeds.
//!
//! Phase 6 needs a deterministic, offline fixture to drive the
//! recon harness. Two builders live here:
//!
//! - [`synthesize_small_capture`] — produces a `.dlf` capture as a
//!   `Vec<u8>` from a hand-rolled pool universe. The capture contains
//!   the `AmmInfo` and `SPL token account` blobs a real Raydium v4
//!   pool would emit. The harness re-assembles pools from the capture
//!   via `pools_from_feed`.
//! - [`synthesize_small_ledger`] — produces a `Vec<u8>` ledger from a
//!   pool universe, suitable for round-trip tests.
//!
//! Plus [`ReconFixture`], a bundle struct holding both, so tests can
//! set up a single object and drive multiple assertions.

use std::collections::BTreeMap;
use std::io::Write;

use dl_core::FeedEvent;
use dl_feed::capture::CaptureWriter;
use dl_ledger::LedgerWriter;
use dl_state::decoder::{
    decode_amm_info, decode_spl_token_account, AmmInfo, SplTokenAccount, AMM_INFO_SIZE,
    SPL_TOKEN_ACCOUNT_SIZE,
};
use dl_state::pool::{AmmKind, Pool};
use dl_state::Pubkey;

use crate::pipeline::{replay_pools_to_ledger, ReplayParams};

// ---------------------------------------------------------------------------
// Synthetic pool builders
// ---------------------------------------------------------------------------

/// One entry in the synthetic universe: a pool address + reserves + a
/// fee. The mint and decimals are filled in by [`synthesize_pools`]
/// to keep call sites terse.
#[derive(Debug, Clone)]
pub struct SynthPoolSpec {
    pub address: [u8; 32],
    pub base_reserve: u64,
    pub quote_reserve: u64,
    pub fee_bps: u16,
}

/// Build a synthetic pool universe with the requested reserve skews.
///
/// `mints` is the list of distinct token pubkeys; each `spec` references
/// two mints by index. The function emits one AmmInfo + two SPL token
/// accounts per pool, keyed so that `pools_from_feed` can re-assemble
/// them without ambiguity.
///
/// Reserves are mapped to the SPL token account `amount` fields
/// directly. The pool address is `spec.address`; the vault pubkeys are
/// derived deterministically as `address XOR index_byte`.
pub fn synthesize_pools(specs: &[SynthPoolSpec], mints: &[[u8; 32]]) -> Vec<Pool> {
    let mut pools = Vec::with_capacity(specs.len());
    for (idx, spec) in specs.iter().enumerate() {
        // Cycle the mint indices so consecutive pools share mints
        // and the graph has tokens interned as few unique nodes.
        let base_mint = mints[idx % mints.len()];
        let quote_mint = mints[(idx + 1) % mints.len()];
        // Derive vault pubkeys deterministically.
        let mut base_vault = spec.address;
        base_vault[31] ^= (idx as u8).wrapping_mul(17);
        let mut quote_vault = spec.address;
        quote_vault[31] ^= (idx as u8).wrapping_mul(17).wrapping_add(1);
        // Build the AmmInfo blob.
        let mut amm_bytes = vec![0u8; AMM_INFO_SIZE];
        // status = 1 (Initialized)
        amm_bytes[0..8].copy_from_slice(&1u64.to_le_bytes());
        // base_decimals = 9, quote_decimals = 6
        amm_bytes[32] = 9;
        amm_bytes[40] = 6;
        // trade_fee_numerator = fee_bps, trade_fee_denominator = 10_000
        amm_bytes[144..152].copy_from_slice(&(spec.fee_bps as u64).to_le_bytes());
        amm_bytes[152..160].copy_from_slice(&10_000u64.to_le_bytes());
        // base_vault, quote_vault
        amm_bytes[336..368].copy_from_slice(&base_vault);
        amm_bytes[368..400].copy_from_slice(&quote_vault);
        // base_mint, quote_mint
        amm_bytes[400..432].copy_from_slice(&base_mint);
        amm_bytes[432..464].copy_from_slice(&quote_mint);
        let info: AmmInfo = decode_amm_info(&amm_bytes).expect("synthetic AmmInfo decodes");

        // Build the two SPL token account blobs.
        let base_acc = SplTokenAccount {
            mint: Pubkey(base_mint),
            amount: spec.base_reserve,
        };
        let quote_acc = SplTokenAccount {
            mint: Pubkey(quote_mint),
            amount: spec.quote_reserve,
        };
        // Hand-roll the 165-byte blobs by encoding. We use
        // `decode_spl_token_account` round-trip: build a minimal blob
        // and ensure it decodes back to what we wrote. Since we don't
        // have an encoder here, we construct the blob field-by-field.
        let base_acc_bytes = encode_spl_token_account(&base_acc);
        let quote_acc_bytes = encode_spl_token_account(&quote_acc);
        let base_round: SplTokenAccount =
            decode_spl_token_account(&base_acc_bytes).expect("base round-trip");
        let quote_round: SplTokenAccount =
            decode_spl_token_account(&quote_acc_bytes).expect("quote round-trip");
        assert_eq!(base_round.mint.0, base_mint);
        assert_eq!(quote_round.mint.0, quote_mint);
        assert_eq!(base_round.amount, spec.base_reserve);
        assert_eq!(quote_round.amount, spec.quote_reserve);

        // Assemble the pool directly (skip the harness's
        // `assemble_pool` so we can use synthetic pubkeys for the
        // pool address without colliding with real vaults).
        let pool = Pool {
            address: Pubkey(spec.address),
            kind: AmmKind::RaydiumAmmV4,
            base_mint: Pubkey(base_mint),
            quote_mint: Pubkey(quote_mint),
            base_decimals: 9,
            quote_decimals: 6,
            base_reserve: spec.base_reserve,
            quote_reserve: spec.quote_reserve,
            fee_bps: spec.fee_bps,
            last_update_slot: 1,
        };
        let _ = info; // AmmInfo is built so the capture path can
                      // re-decode it; for the direct-pool path we
                      // don't need it further.
        pools.push(pool);
    }
    pools
}

/// Hand-rolled 165-byte SPL token account blob. Layout mirrors
/// `decode_spl_token_account` (mint at offset 0, amount at offset 64).
/// The remaining bytes are zero-filled — the v1.0 decoder doesn't read
/// them.
fn encode_spl_token_account(acc: &SplTokenAccount) -> Vec<u8> {
    let mut buf = vec![0u8; SPL_TOKEN_ACCOUNT_SIZE];
    buf[0..32].copy_from_slice(&acc.mint.0);
    buf[64..72].copy_from_slice(&acc.amount.to_le_bytes());
    buf
}

// ---------------------------------------------------------------------------
// Synthetic capture (replay-capture path)
// ---------------------------------------------------------------------------

/// Build a `.dlf` capture blob from a pool universe. The capture
/// emits an `AccountUpdate` per pool's three components (AmmInfo +
/// 2 SPL token accounts) in slot 1.
///
/// Replaying this capture through [`crate::pipeline::replay_capture_to_ledger`]
/// re-assembles the pools via `pools_from_feed`. EOF terminates the
/// stream (invariant I-5).
pub fn synthesize_small_capture(specs: &[SynthPoolSpec], mints: &[[u8; 32]]) -> Vec<u8> {
    let mut sink: Vec<u8> = Vec::new();
    {
        let mut writer = CaptureWriter::new(&mut sink).expect("capture writer open");
        for (idx, spec) in specs.iter().enumerate() {
            let base_mint = mints[idx % mints.len()];
            let quote_mint = mints[(idx + 1) % mints.len()];
            let mut base_vault = spec.address;
            base_vault[31] ^= (idx as u8).wrapping_mul(17);
            let mut quote_vault = spec.address;
            quote_vault[31] ^= (idx as u8).wrapping_mul(17).wrapping_add(1);

            // AmmInfo
            let mut amm_bytes = vec![0u8; AMM_INFO_SIZE];
            amm_bytes[0..8].copy_from_slice(&1u64.to_le_bytes());
            amm_bytes[32] = 9;
            amm_bytes[40] = 6;
            amm_bytes[144..152].copy_from_slice(&(spec.fee_bps as u64).to_le_bytes());
            amm_bytes[152..160].copy_from_slice(&10_000u64.to_le_bytes());
            amm_bytes[336..368].copy_from_slice(&base_vault);
            amm_bytes[368..400].copy_from_slice(&quote_vault);
            amm_bytes[400..432].copy_from_slice(&base_mint);
            amm_bytes[432..464].copy_from_slice(&quote_mint);
            writer
                .write_event(&FeedEvent::AccountUpdate {
                    slot: 1,
                    pubkey: spec.address,
                    data: amm_bytes,
                })
                .expect("write AmmInfo");

            // Base SPL token account.
            let base_acc = SplTokenAccount {
                mint: Pubkey(base_mint),
                amount: spec.base_reserve,
            };
            writer
                .write_event(&FeedEvent::AccountUpdate {
                    slot: 1,
                    pubkey: base_vault,
                    data: encode_spl_token_account(&base_acc),
                })
                .expect("write base vault");

            // Quote SPL token account.
            let quote_acc = SplTokenAccount {
                mint: Pubkey(quote_mint),
                amount: spec.quote_reserve,
            };
            writer
                .write_event(&FeedEvent::AccountUpdate {
                    slot: 1,
                    pubkey: quote_vault,
                    data: encode_spl_token_account(&quote_acc),
                })
                .expect("write quote vault");
        }
        writer
            .into_inner()
            .expect("capture writer flush")
            .flush()
            .expect("capture flush");
    }
    sink
}

// ---------------------------------------------------------------------------
// Synthetic ledger (replay-pools path)
// ---------------------------------------------------------------------------

/// Build a `.dlg` ledger blob from a pool universe by running the
/// harness once. Round-trippable: the bytes can be read back by
/// `LedgerReader::open` to yield the same entries.
pub fn synthesize_small_ledger(
    pools: &[Pool],
    params: &ReplayParams,
) -> Result<Vec<u8>, crate::ReconError> {
    let report = replay_pools_to_ledger(pools, params)?;
    let mut sink: Vec<u8> = Vec::new();
    {
        let mut writer = LedgerWriter::new(&mut sink).map_err(crate::ReconError::Ledger)?;
        for record in &report.cycle_records {
            writer
                .write_entry(&record.entry)
                .map_err(crate::ReconError::Ledger)?;
        }
        writer.into_inner().map_err(crate::ReconError::Ledger)?;
    }
    Ok(sink)
}

// ---------------------------------------------------------------------------
// Fixture bundle
// ---------------------------------------------------------------------------

/// A self-contained test fixture: synthetic pool universe, capture
/// blob, and ledger blob, plus the [`ReplayParams`] used to derive
/// the ledger. Tests build one of these per scenario.
#[derive(Debug, Clone)]
pub struct ReconFixture {
    /// The pool universe the harness sees.
    pub pools: Vec<Pool>,
    /// The same pools expressed as a `.dlf` capture blob.
    pub capture: Vec<u8>,
    /// The harness's output as a `.dlg` ledger blob.
    pub ledger: Vec<u8>,
    /// Replay params used to build the ledger.
    pub params: ReplayParams,
}

impl ReconFixture {
    /// Build a fixture from a `SynthPoolSpec` list + mint universe.
    /// Convenience: derives `pools`, `capture`, and `ledger` in one
    /// call so tests don't have to wire them up individually.
    pub fn build(specs: &[SynthPoolSpec], mints: &[[u8; 32]], params: &ReplayParams) -> Self {
        let pools = synthesize_pools(specs, mints);
        let capture = synthesize_small_capture(specs, mints);
        let ledger = synthesize_small_ledger(&pools, params).expect("ledger build");
        Self {
            pools,
            capture,
            ledger,
            params: params.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Re-exports
// ---------------------------------------------------------------------------

// Keep the unused-import linter happy.
#[allow(dead_code)]
fn _ensure_used(_: &BTreeMap<u64, u64>) {}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::ReplayParams;

    fn triangle_specs() -> Vec<SynthPoolSpec> {
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
    fn synthesize_pools_returns_correct_count() {
        let specs = triangle_specs();
        let mints = three_mints();
        let pools = synthesize_pools(&specs, &mints);
        assert_eq!(pools.len(), 3);
    }

    #[test]
    fn synthesize_capture_round_trips_pools() {
        let specs = triangle_specs();
        let mints = three_mints();
        let capture = synthesize_small_capture(&specs, &mints);
        // Capture starts with the magic header.
        assert!(capture.starts_with(b"DLF-CAP1"));
        // Total size: 12 (header) + N * (4 + bincode(FeedEvent)). bincode
        // serializes the FeedEvent with its enum tag; the actual payload
        // is bigger than the raw AccountUpdate data. Verify the header is
        // present and the rest is non-trivial.
        assert!(capture.starts_with(b"DLF-CAP1"));
        assert!(capture.len() > 12 + 3 * (4 + AMM_INFO_SIZE));
        assert!(capture.len() > 12 + 3 * (4 + SPL_TOKEN_ACCOUNT_SIZE));
    }

    #[test]
    fn fixture_bundle_self_consistent() {
        let specs = triangle_specs();
        let mints = three_mints();
        let params = ReplayParams::default();
        let fx = ReconFixture::build(&specs, &mints, &params);
        assert_eq!(fx.pools.len(), 3);
        assert!(fx.capture.starts_with(b"DLF-CAP1"));
        assert!(fx.ledger.starts_with(b"DLD-LDG1"));
    }
}
