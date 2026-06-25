//! `cycle_writer` — emits `cycle.v1` JSONL records from detected cycles.
//!
//! Per `docs/contracts/cycle.v1.md` and `docs/contracts/cycle.v1.schema.json`.
//! The writer produces two files next to the wallet:
//!
//! 1. `wallet.cycles.v1.jsonl` — the contract-compliant v1 record
//!    (one per line). This is the file the Data pipeline
//!    (DAM-43) reads.
//! 2. `wallet.cycles.jsonl` — the v0 ad-hoc shape (pool_address
//!    as a cycle hash, `base_mint = "unknown"`, `dex = "raydium"`).
//!    The ArbiNexus bridge consumes this until DAM-44 lands; the
//!    shim keeps the bridge unchanged for one release.
//!
//! The contract's `cycle_id` is `blake3(sorted_legs_json || detected_at_slot)`
//! rendered as 64 lowercase hex chars; the helper lives in `dl-core` so the
//! pipeline (DAM-43) can recompute it from the same canonical inputs.
//!
//! This module is intentionally a library module (not a `main.rs` private fn)
//! so the schema-conformance test in `tests/cycle_writer_schema.rs` can call
//! it directly without spinning up a full `dl-app` process.

use dl_core::cycle_id_hex;
use dl_core::LegKey;
use std::io::Write;

/// Context passed from the call site to the writer so every emitted
/// `cycle.v1` record carries the fields the writer cannot derive from
/// the cycle alone.
#[derive(Debug, Clone, Copy)]
pub struct CycleWriteContext {
    /// UUIDv4 generated once at process start; the writer forwards it
    /// on every row so downstream queries can filter on it.
    pub bot_run_id: uuid::Uuid,
    /// `"ws:mainnet"` for live runs, `"capture:replay"` for
    /// `--feed capture <path>`. The pipeline partitions on this.
    pub source_feed: &'static str,
    /// Slot of the pool update that produced the cycle (the
    /// triggering `AccountUpdate`). The contract accepts 0; until
    /// vault subscriptions land this is the default.
    pub detected_at_slot: u64,
}

/// Map an `AmmKind` to the contract enum string. The schema enum is
/// `[raydium, orca, meteora]` today; this is the only place that knows
/// about new variants, so we centralize the mapping here.
pub fn amm_kind_to_str(kind: dl_state::pool::AmmKind) -> &'static str {
    use dl_state::pool::AmmKind;
    match kind {
        AmmKind::RaydiumAmmV4 => "raydium",
        AmmKind::OrcaWhirlpool => "orca",
        AmmKind::MeteoraDlmm => "meteora",
    }
}

/// Map an `EvalParams` instance to the contract enum string. The
/// contract has two named variants: `conservative_default` and
/// `optimistic`. We compare by value (the struct is `Copy + Eq`).
pub fn evaluator_name(p: &dl_sim::ev::EvalParams) -> &'static str {
    if *p == dl_sim::ev::EvalParams::conservative_default() {
        "conservative_default"
    } else if *p == dl_sim::ev::EvalParams::optimistic() {
        "optimistic"
    } else {
        // A non-default instance (e.g. a custom tip via
        // `conservative_default_with_tip`); the contract enum is
        // closed, so we map to the closest named variant. The
        // conservative side is what gates the trade, so mapping
        // to it is the honest choice.
        "conservative_default"
    }
}

/// Map a `Direction` to the contract enum string.
pub fn direction_to_str(d: dl_state::cycle::Direction) -> &'static str {
    use dl_state::cycle::Direction;
    match d {
        Direction::BaseToQuote => "BaseToQuote",
        Direction::QuoteToBase => "QuoteToBase",
    }
}

/// Look up a leg's pool in the snapshot; returns the matching `Pool`'s
/// `AmmKind` and `fee_bps`. Falls back to `RaydiumAmmV4` / 30 bps if
/// the pool is missing (the registry may not have caught up). The
/// fallback preserves the v0 writer's behaviour and keeps the v1
/// schema `dex` enum invariant — no unknown strings can leak.
pub fn leg_pool_lookup(
    registry: &dl_state::PoolRegistry,
    pool_addr: &[u8; 32],
) -> (dl_state::pool::AmmKind, u16) {
    if let Some(p) = registry.get(pool_addr) {
        (p.kind, p.fee_bps)
    } else {
        (
            dl_state::pool::AmmKind::RaydiumAmmV4,
            30, // Raydium AMM v4 default fee
        )
    }
}

/// Build the `cycle.v1` JSON object for one detected cycle. The
/// function is pure (no I/O) so tests can call it and assert the
/// emitted shape; the file-writing `append_cycle_jsonl` reuses it.
pub fn build_cycle_v1_record(
    cycle: &dl_state::cycle::Cycle,
    registry: &dl_state::PoolRegistry,
    conservative_eval: &dl_sim::ev::EvalParams,
    input_lamports: u64,
    output_lamports: u64,
    write_ctx: CycleWriteContext,
) -> serde_json::Value {
    // ── Per-leg resolution ─────────────────────────────────────────
    // Build the canonical leg key list, the per-leg JSON view, and
    // the per-leg dex list. The legs array in the v1 record carries
    // the full graph path; `dexes` is the distinct DEX list across
    // legs (a de-duped view for cheap join keys).
    let mut leg_keys: Vec<LegKey> = Vec::with_capacity(cycle.legs.len());
    let mut leg_views: Vec<serde_json::Value> = Vec::with_capacity(cycle.legs.len());
    let mut fee_bps_sum: u32 = 0;
    let mut distinct_dexes: Vec<String> = Vec::new();
    for leg in &cycle.legs {
        let (kind, fee_bps) = leg_pool_lookup(registry, &leg.pool.0);
        fee_bps_sum = fee_bps_sum.saturating_add(u32::from(fee_bps));
        let dex = amm_kind_to_str(kind).to_string();
        if !distinct_dexes.iter().any(|d| d == &dex) {
            distinct_dexes.push(dex.clone());
        }
        leg_keys.push(LegKey {
            pool: leg.pool.0,
            direction_base_to_quote: matches!(
                leg.direction,
                dl_state::cycle::Direction::BaseToQuote
            ),
            // Widen i64 → i128 losslessly; the contract renders the
            // weight as a JSON integer so the consumer can read it
            // back at any width.
            weight: i128::from(leg.weight),
        });
        leg_views.push(serde_json::json!({
            "pool": bs58::encode(leg.pool.0).into_string(),
            "dex": dex,
            "direction": direction_to_str(leg.direction),
            "weight": i128::from(leg.weight),
        }));
    }

    // ── Mints (best-effort, per contract) ──────────────────────────
    // 2-leg cycles: first leg's base mint, last leg's quote mint.
    // >2 legs: contract says best-effort → emit empty strings so the
    // pipeline does not poison the join with "unknown" placeholders.
    let (base_mint, quote_mint) = if cycle.legs.len() == 2 {
        let first = &cycle.legs[0];
        let last = &cycle.legs[cycle.legs.len() - 1];
        let base = if matches!(first.direction, dl_state::cycle::Direction::BaseToQuote) {
            first.pool.0
        } else {
            last.pool.0
        };
        let quote = if matches!(last.direction, dl_state::cycle::Direction::BaseToQuote) {
            last.pool.0
        } else {
            first.pool.0
        };
        (bs58::encode(base).into_string(), bs58::encode(quote).into_string())
    } else {
        (String::new(), String::new())
    };

    // ── Deterministic cycle_id ─────────────────────────────────────
    let cycle_id = cycle_id_hex(&leg_keys, write_ctx.detected_at_slot);

    // ── gross_bps (signed) ─────────────────────────────────────────
    let gross_bps: i64 = if output_lamports > input_lamports {
        let diff = (output_lamports - input_lamports) as u128;
        ((diff.saturating_mul(10_000)) / (input_lamports as u128).max(1)) as i64
    } else {
        0
    };

    let now_ms: u64 = chrono::Utc::now().timestamp_millis().max(0) as u64;
    let bot_run_id_str = write_ctx.bot_run_id.to_string();
    let evaluator = evaluator_name(conservative_eval);

    serde_json::json!({
        "schema": "cycle.v1",
        "cycle_id": cycle_id,
        "detected_at_unix_ms": now_ms,
        "detected_at_slot": write_ctx.detected_at_slot,
        "bot_run_id": bot_run_id_str,
        "dexes": distinct_dexes,
        "legs": leg_views,
        "base_mint": base_mint,
        "quote_mint": quote_mint,
        "gross_bps": gross_bps,
        "fee_bps_sum": fee_bps_sum,
        "decision": "WouldTrade",
        "evaluator": evaluator,
        "input_lamports": input_lamports,
        "output_lamports": output_lamports,
        "source_feed": write_ctx.source_feed,
    })
}

/// Build the v0 back-compat shim for the ArbiNexus bridge. Same
/// fields the v1.1.5-v1.1.7 writer emitted. The bridge keeps reading
/// this until DAM-44 lands; until then the writer emits one of these
/// per row alongside the v1 record.
pub fn build_cycle_v0_shim(
    cycle_id: &str,
    gross_bps: i64,
    now_ms: u64,
) -> serde_json::Value {
    serde_json::json!({
        "pool_address": cycle_id, // the v0 "pool_address" was always a cycle hash
        "dex": "raydium",         // single-DEX paper mode in v1.1.5
        "base_mint": "unknown",
        "quote_mint": "unknown",
        "gross_bps": gross_bps,
        "fee_bps": 30,            // Raydium AMM v4 default fee
        "detected_at_unix_ms": now_ms,
    })
}

/// Append a single detected cycle to the wallet's JSONL output.
///
/// Writes to **two** files, in order:
///
/// 1. `wallet.cycles.v1.jsonl` — one `cycle.v1` record per line,
///    matching `docs/contracts/cycle.v1.md` and the JSON Schema at
///    `docs/contracts/cycle.v1.schema.json`. The Data pipeline
///    (DAM-43) reads this file. `cycle_id` is the deterministic
///    `blake3(sorted_legs_json || detected_at_slot)` hash, rendered
///    as 64 lowercase hex chars (the contract's `pattern: ^[0-9a-f]{64}$`).
///
/// 2. `wallet.cycles.jsonl` — the v0 ad-hoc shape. DAM-44
///    swaps the ArbiNexus bridge over to the v1 file; until then the
///    bridge keeps reading the old shape. We keep emitting the shim
///    for one release, then remove it (tracked as a follow-up).
///
/// `registry` is the per-call `PoolRegistry` that the detector built
/// from the live snapshot; we use it to resolve per-leg `dex` and
/// `fee_bps`. `conservative_eval` is the `EvalParams` instance the
/// trade gate used; the `evaluator` field is its contract name.
/// `detected_at_unix_ms` is captured here so all rows in a single
/// run are timestamped at the moment of write, not the moment of
/// detection (sub-millisecond; irrelevant for the join).
pub fn append_cycle_jsonl(
    wallet_path: &std::path::Path,
    cycle: &dl_state::cycle::Cycle,
    registry: &dl_state::PoolRegistry,
    conservative_eval: &dl_sim::ev::EvalParams,
    input_lamports: u64,
    output_lamports: u64,
    write_ctx: CycleWriteContext,
) {
    let v1 = build_cycle_v1_record(
        cycle,
        registry,
        conservative_eval,
        input_lamports,
        output_lamports,
        write_ctx,
    );

    // ── Write v1 line to wallet.cycles.v1.jsonl ────────────────────
    let v1_path = append_jsonl_path(wallet_path, "cycles.v1.jsonl");
    if let Err(e) = write_line(&v1_path, &v1) {
        eprintln!("dl-app run: cycles.v1.jsonl write failed: {e}");
    }

    // ── Back-compat shim: v0 shape, written to wallet.cycles.jsonl ─
    // The v0 shape is what DAM-44's bridge consumes today. We keep
    // the same defaults the old writer hard-coded so existing bridge
    // behaviour is byte-identical for the fields it reads.
    let cycle_id = v1.get("cycle_id").and_then(|v| v.as_str()).unwrap_or("");
    let gross_bps = v1.get("gross_bps").and_then(|v| v.as_i64()).unwrap_or(0);
    let now_ms = v1
        .get("detected_at_unix_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let v0 = build_cycle_v0_shim(cycle_id, gross_bps, now_ms);
    let v0_path = append_jsonl_path(wallet_path, "cycles.jsonl");
    if let Err(e) = write_line(&v0_path, &v0) {
        eprintln!("dl-app run: cycles.jsonl write failed: {e}");
    }
}

/// Compute the on-disk path for a `.jsonl` next to the wallet file.
/// Centralized so the writer and the test fixtures use one rule:
/// `wallet.json` → `wallet.<name>`.
pub fn append_jsonl_path(wallet_path: &std::path::Path, name: &str) -> std::path::PathBuf {
    let mut p = wallet_path.to_path_buf();
    p.set_file_name(name);
    p
}

/// Open `path` in append mode, write `value` as one JSON line, flush.
/// Errors are surfaced to the caller; the writer logs and continues.
pub fn write_line(
    path: &std::path::Path,
    value: &serde_json::Value,
) -> std::io::Result<()> {
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let line = format!("{}\n", value);
    f.write_all(line.as_bytes())?;
    f.flush()
}
