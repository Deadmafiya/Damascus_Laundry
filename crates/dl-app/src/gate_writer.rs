//! `gate_writer` — emits a JSONL log of `gate_approved` events for
//! DAM-79 / SLO #3 reconciliation.
//!
//! ## Why a separate file
//!
//! The `cycle.v1` writer (`cycle_writer.rs`) records *detection
//! decisions*. This writer records *submission gate decisions*:
//! the moment a bundle the simulate-gate approved is handed to
//! Jito. Joining the two by `bundle_id` is what surfaces silent
//! reverts (a gate approval that never produces a Landed /
//! FailedCleanly outcome within `landing_timeout_ms * 3`).
//!
//! ## Wire format
//!
//! One JSON object per line, schema-tagged `bundle_event.v1`:
//!
//! ```json
//! { "schema": "bundle_event.v1",
//!   "kind":    "gate_approved",
//!   "ts_unix_ms": 1734567890123,
//!   "cycle_id": "<64 hex chars>",
//!   "bundle_id":"<jito-uuid>",
//!   "slot": 0,
//!   "sim_net_lamports": 12345,
//!   "input_mint":  "<base58>",
//!   "output_mint": "<base58>",
//!   "input_amount_lamports": 1000000000,
//!   "tip_lamports": 10000 }
//! ```
//!
//! The file path is `wallet.gate_events.jsonl` next to the wallet
//! (`append_jsonl_path(wallet_path, "gate_events.jsonl")`).
//!
//! ## Outcomes join
//!
//! The companion `dl-recon replay --bundles …` binary reads two
//! files and joins on `bundle_id`:
//! - this file (`gate_events.jsonl`): the gate approvals
//! - an `outcomes.jsonl` (one line per landed / failed-cleanly
//!   bundle, with `kind` and `bundle_id`) produced by the live
//!   runner's landing poller
//!
//! The recon join classifies every approved bundle into
//! Landed / FailedCleanly / SilentRevert and reports
//! `silent_revert_count` — the SLO #3 counter.

use std::io::Write;

use dl_core::cycle_id_hex;
use dl_core::LegKey;
use dl_state::cycle::Cycle;
use dl_state::Pubkey;

/// Schema tag for the gate-event JSONL rows. Distinct from
/// `cycle.v1` so the recon pipeline can route by `schema`.
pub const SCHEMA: &str = "bundle_event.v1";

/// `kind` value for gate-approval rows.
pub const KIND_GATE_APPROVED: &str = "gate_approved";

/// One gate-approval event, ready to serialize.
#[derive(Debug, Clone)]
pub struct GateApprovedEvent {
    /// Wall-clock millis since epoch.
    pub ts_unix_ms: u64,
    /// `cycle_id_hex(legs, slot)` from the detection — 64 lowercase
    /// hex chars. Same algorithm as `cycle_writer` so the two
    /// files can be cross-referenced.
    pub cycle_id: String,
    /// Jito `bundle_id` (UUIDv4 string from the block engine).
    /// Empty string when the gate approves but the Jito submit
    /// has not yet been issued (DAM-79 scope: emit *on Approve*;
    /// the bundle_id is set before Jito returns so it is non-empty
    /// in normal flow).
    pub bundle_id: String,
    /// Slot at the moment of gate decision. Best-effort 0 when
    /// the RPC didn't return one. The recon join uses this only
    /// as a tiebreaker; the wall-clock `ts_unix_ms` is the
    /// authoritative ordering field.
    pub slot: u64,
    /// Simulated net PnL (lamports). From `SimulationReport::net_pnl_lamports`;
    /// `0` when the report didn't carry an explicit value (the
    /// Jupiter-swap common case where the dl-assert tx enforces
    /// profitability on-chain).
    pub sim_net_lamports: i64,
    /// Input mint (base58) for operator readability. Empty if
    /// the legs could not be resolved.
    pub input_mint: String,
    /// Output mint (base58) for operator readability. Empty if
    /// the legs could not be resolved.
    pub output_mint: String,
    /// Input size in lamports (snapshot at decision time).
    pub input_amount_lamports: u64,
    /// Jito tip in lamports (snapshot at decision time).
    pub tip_lamports: u64,
}

/// Build the JSON object for one gate-approval event. Pure
/// (no I/O) so tests can call it and assert the emitted shape.
///
/// `detected_at_slot` is the slot the cycle was originally
/// detected on; this is the same input `cycle_writer::build_cycle_v1_record`
/// uses, so the emitted `cycle_id` matches the detection-time
/// hash. Pass `0` when the live path doesn't carry the detection
/// slot — the `cycle_id` will still be deterministic per-cycle
/// but will not match the `cycle.v1` row for the same cycle.
pub fn build_gate_approved_event(
    cycle: &Cycle,
    detected_at_slot: u64,
    bundle_id: &str,
    slot: u64,
    sim_net_lamports: i64,
    input_mint: Pubkey,
    output_mint: Pubkey,
    input_amount_lamports: u64,
    tip_lamports: u64,
) -> serde_json::Value {
    // Build the canonical LegKey list so the cycle_id matches the
    // detection-time hash byte-for-byte.
    let leg_keys: Vec<LegKey> = cycle
        .legs
        .iter()
        .map(|l| LegKey {
            pool: l.pool.0,
            direction_base_to_quote: matches!(l.direction, dl_state::cycle::Direction::BaseToQuote),
            weight: i128::from(l.weight),
        })
        .collect();
    let cycle_id = cycle_id_hex(&leg_keys, detected_at_slot);
    let now_ms: u64 = chrono::Utc::now().timestamp_millis().max(0) as u64;
    serde_json::json!({
        "schema": SCHEMA,
        "kind": KIND_GATE_APPROVED,
        "ts_unix_ms": now_ms,
        "cycle_id": cycle_id,
        "bundle_id": bundle_id,
        "slot": slot,
        "sim_net_lamports": sim_net_lamports,
        "input_mint": bs58::encode(input_mint.0).into_string(),
        "output_mint": bs58::encode(output_mint.0).into_string(),
        "input_amount_lamports": input_amount_lamports,
        "tip_lamports": tip_lamports,
    })
}

/// Append one gate-approval event to `wallet.gate_events.jsonl`.
///
/// `wallet_path` is the wallet file path; the writer derives the
/// event file path as `wallet.gate_events.jsonl` (replaces the
/// file extension). This matches the `cycle_writer::append_jsonl_path`
/// rule: `wallet.json` → `wallet.<name>`.
///
/// The function is best-effort: a write failure is logged via
/// `eprintln!` and the function returns `Ok(())` (the live path
/// must not halt on telemetry failure). Callers that need
/// strict success semantics should call `build_gate_approved_event`
/// directly and handle the result.
pub fn append_gate_event_jsonl(
    wallet_path: &std::path::Path,
    event: &serde_json::Value,
) -> std::io::Result<()> {
    let path = append_gate_jsonl_path(wallet_path);
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    let line = format!("{}\n", event);
    f.write_all(line.as_bytes())?;
    f.flush()
}

/// Compute the on-disk path for the gate-events JSONL next to
/// the wallet file. Centralized so the writer and the test
/// fixtures use one rule: `wallet.json` → `wallet.gate_events.jsonl`.
pub fn append_gate_jsonl_path(wallet_path: &std::path::Path) -> std::path::PathBuf {
    let mut p = wallet_path.to_path_buf();
    p.set_file_name("wallet.gate_events.jsonl");
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use dl_state::cycle::{Cycle, Direction, Leg};
    use std::io::Read;

    fn two_leg_cycle() -> Cycle {
        let mut p1 = [0u8; 32];
        p1[31] = 1;
        let mut p2 = [0u8; 32];
        p2[31] = 2;
        Cycle::new(vec![
            Leg {
                pool: Pubkey(p1),
                direction: Direction::BaseToQuote,
                weight: -100,
            },
            Leg {
                pool: Pubkey(p2),
                direction: Direction::QuoteToBase,
                weight: 50,
            },
        ])
    }

    #[test]
    fn build_event_emits_required_fields() {
        let cycle = two_leg_cycle();
        let bundle_id = "11111111-2222-3333-4444-555555555555";
        let event = build_gate_approved_event(
            &cycle,
            12345,
            bundle_id,
            98765,
            7_777,
            Pubkey([0xaa; 32]),
            Pubkey([0xbb; 32]),
            1_000_000_000,
            10_000,
        );
        assert_eq!(event.get("schema").and_then(|v| v.as_str()), Some(SCHEMA));
        assert_eq!(
            event.get("kind").and_then(|v| v.as_str()),
            Some(KIND_GATE_APPROVED)
        );
        assert_eq!(event.get("bundle_id").and_then(|v| v.as_str()), Some(bundle_id));
        assert_eq!(event.get("slot").and_then(|v| v.as_u64()), Some(98765));
        assert_eq!(event.get("sim_net_lamports").and_then(|v| v.as_i64()), Some(7_777));
        assert_eq!(
            event.get("input_amount_lamports").and_then(|v| v.as_u64()),
            Some(1_000_000_000)
        );
        assert_eq!(event.get("tip_lamports").and_then(|v| v.as_u64()), Some(10_000));
        let cycle_id = event
            .get("cycle_id")
            .and_then(|v| v.as_str())
            .expect("cycle_id");
        assert_eq!(cycle_id.len(), 64, "cycle_id is 64 lowercase hex chars");
        assert!(
            cycle_id.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "cycle_id must be lowercase hex; got {cycle_id}"
        );
        // Mints render as base58.
        let input_mint = event
            .get("input_mint")
            .and_then(|v| v.as_str())
            .expect("input_mint");
        assert!(!input_mint.is_empty());
        let output_mint = event
            .get("output_mint")
            .and_then(|v| v.as_str())
            .expect("output_mint");
        assert!(!output_mint.is_empty());
    }

    #[test]
    fn append_gate_event_jsonl_writes_one_line() {
        let tmp_dir = tempdir_workaround();
        let wallet = tmp_dir.join("wallet.json");
        let cycle = two_leg_cycle();
        let event = build_gate_approved_event(
            &cycle,
            7,
            "bid",
            1,
            2,
            Pubkey([0x01; 32]),
            Pubkey([0x02; 32]),
            100,
            10,
        );
        append_gate_event_jsonl(&wallet, &event).expect("write 1");
        append_gate_event_jsonl(&wallet, &event).expect("write 2");

        let out_path = append_gate_jsonl_path(&wallet);
        let mut buf = String::new();
        std::fs::File::open(&out_path)
            .expect("open")
            .read_to_string(&mut buf)
            .expect("read");
        let lines: Vec<&str> = buf.lines().collect();
        assert_eq!(lines.len(), 2, "expected 2 lines, got {lines:?}");
        for line in &lines {
            // Round-trip the JSON to confirm it parses.
            let v: serde_json::Value = serde_json::from_str(line).expect("parse line");
            assert_eq!(v.get("schema").and_then(|x| x.as_str()), Some(SCHEMA));
            assert_eq!(
                v.get("kind").and_then(|x| x.as_str()),
                Some(KIND_GATE_APPROVED)
            );
        }
    }

    #[test]
    fn append_gate_jsonl_path_replaces_extension() {
        let p = std::path::PathBuf::from("/tmp/foo/wallet.json");
        let out = append_gate_jsonl_path(&p);
        assert_eq!(out, std::path::PathBuf::from("/tmp/foo/wallet.gate_events.jsonl"));
    }

    // tempdir_workaround keeps the test self-contained without
    // pulling in a tempfile crate dependency.
    fn tempdir_workaround() -> std::path::PathBuf {
        let base = std::env::temp_dir();
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = base.join(format!("dl-gate-writer-test-{pid}-{nanos}"));
        std::fs::create_dir_all(&dir).expect("mkdir tempdir");
        dir
    }
}
