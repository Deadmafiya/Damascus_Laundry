//! Schema-conformance tests for the `cycle_writer` module.
//!
//! Per DAM-43 §"Schema-conformance check": call the writer with a
//! fixture cycle, parse the emitted line, validate against
//! `docs/contracts/cycle.v1.schema.json`, and assert zero validation
//! errors. Use the `rejects` cases from the contract's test-fixtures
//! list to assert `dl-app` never produces a reject.
//!
//! Schema validation is hand-rolled: the schema is small and tight,
//! and adding a `jsonschema` crate for one test is too much weight.
//! Every required field is checked for type, the `pattern` on
//! `cycle_id`, the enums on `decision` / `evaluator` / `source_feed`
//! / per-leg `dex` / per-leg `direction`, and the `additionalProperties`
//! rules. Anything the contract demands but the writer does not
//! emit is caught here, not in production.

use dl_app::cycle_writer::{
    amm_kind_to_str, append_cycle_jsonl, build_cycle_v0_shim, build_cycle_v1_record,
    direction_to_str, evaluator_name, leg_pool_lookup, write_line, CycleWriteContext,
};
use dl_sim::ev::EvalParams;
use dl_state::cycle::{Cycle, Direction, Leg, Pubkey as CyclePubkey};
use dl_state::pool::{AmmKind, Pool};
use dl_state::PoolRegistry;
use std::path::PathBuf;
use uuid::Uuid;

// ── Schema validator (minimal, contract-driven) ─────────────────────

#[derive(Debug)]
enum SchemaError {
    MissingField(&'static str),
    WrongType {
        field: &'static str,
        expected: &'static str,
    },
    Pattern {
        field: &'static str,
        value: String,
    },
    Enum {
        field: &'static str,
        value: String,
    },
    Length {
        field: &'static str,
        min: usize,
        max: usize,
        got: usize,
    },
    Unknown {
        field: &'static str,
    },
}

impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SchemaError::MissingField(n) => write!(f, "missing required field `{n}`"),
            SchemaError::WrongType { field, expected } => {
                write!(f, "field `{field}` has wrong type (expected {expected})")
            }
            SchemaError::Pattern { field, value } => {
                write!(f, "field `{field}` does not match pattern: {value}")
            }
            SchemaError::Enum { field, value } => {
                write!(f, "field `{field}` has unknown enum value: {value}")
            }
            SchemaError::Length {
                field,
                min,
                max,
                got,
            } => write!(f, "field `{field}` length {got} not in [{min},{max}]"),
            SchemaError::Unknown { field } => write!(
                f,
                "field `{field}` is unknown (additionalProperties: false)"
            ),
        }
    }
}

fn validate_cycle_v1(v: &serde_json::Value) -> Result<(), Vec<SchemaError>> {
    let mut errs = Vec::new();
    let obj = match v.as_object() {
        Some(o) => o,
        None => {
            errs.push(SchemaError::WrongType {
                field: "<root>",
                expected: "object",
            });
            return Err(errs);
        }
    };

    // Allowed top-level fields (additionalProperties: false).
    const ALLOWED: &[&str] = &[
        "schema",
        "cycle_id",
        "detected_at_unix_ms",
        "detected_at_slot",
        "bot_run_id",
        "dexes",
        "legs",
        "base_mint",
        "quote_mint",
        "gross_bps",
        "fee_bps_sum",
        "decision",
        "evaluator",
        "input_lamports",
        "output_lamports",
        "source_feed",
    ];
    for k in obj.keys() {
        if !ALLOWED.contains(&k.as_str()) {
            errs.push(SchemaError::Unknown { field: k.as_str() });
        }
    }

    // schema: const "cycle.v1"
    match obj.get("schema").and_then(|v| v.as_str()) {
        Some("cycle.v1") => {}
        Some(s) => errs.push(SchemaError::Enum {
            field: "schema",
            value: s.to_string(),
        }),
        None => errs.push(SchemaError::MissingField("schema")),
    }

    // cycle_id: ^[0-9a-f]{64}$
    match obj.get("cycle_id").and_then(|v| v.as_str()) {
        Some(s)
            if s.len() == 64
                && s.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) => {}
        Some(s) => errs.push(SchemaError::Pattern {
            field: "cycle_id",
            value: s.to_string(),
        }),
        None => errs.push(SchemaError::MissingField("cycle_id")),
    }

    // detected_at_unix_ms: integer >= 0
    match obj.get("detected_at_unix_ms") {
        Some(serde_json::Value::Number(n)) if n.as_u64().is_some() => {}
        Some(_) => errs.push(SchemaError::WrongType {
            field: "detected_at_unix_ms",
            expected: "integer >= 0",
        }),
        None => errs.push(SchemaError::MissingField("detected_at_unix_ms")),
    }

    // detected_at_slot: integer >= 0
    match obj.get("detected_at_slot") {
        Some(serde_json::Value::Number(n)) if n.as_u64().is_some() => {}
        Some(_) => errs.push(SchemaError::WrongType {
            field: "detected_at_slot",
            expected: "integer >= 0",
        }),
        None => errs.push(SchemaError::MissingField("detected_at_slot")),
    }

    // bot_run_id: string in UUID format
    match obj.get("bot_run_id").and_then(|v| v.as_str()) {
        Some(s) => match Uuid::parse_str(s) {
            Ok(_) => {}
            Err(_) => errs.push(SchemaError::Pattern {
                field: "bot_run_id",
                value: s.to_string(),
            }),
        },
        None => errs.push(SchemaError::MissingField("bot_run_id")),
    }

    // dexes: array, min 1, items in [raydium, orca, meteora]
    match obj.get("dexes") {
        Some(serde_json::Value::Array(arr)) if !arr.is_empty() => {
            for d in arr {
                match d.as_str() {
                    Some("raydium") | Some("orca") | Some("meteora") => {}
                    Some(s) => errs.push(SchemaError::Enum {
                        field: "dexes[]",
                        value: s.to_string(),
                    }),
                    None => errs.push(SchemaError::WrongType {
                        field: "dexes[]",
                        expected: "string",
                    }),
                }
            }
        }
        Some(serde_json::Value::Array(_)) => errs.push(SchemaError::Length {
            field: "dexes",
            min: 1,
            max: usize::MAX,
            got: 0,
        }),
        Some(_) => errs.push(SchemaError::WrongType {
            field: "dexes",
            expected: "array<string>",
        }),
        None => errs.push(SchemaError::MissingField("dexes")),
    }

    // legs: array, min 2, each item validated
    match obj.get("legs") {
        Some(serde_json::Value::Array(arr)) if arr.len() >= 2 => {
            for (i, leg) in arr.iter().enumerate() {
                validate_leg(leg, i, &mut errs);
            }
        }
        Some(serde_json::Value::Array(arr)) => errs.push(SchemaError::Length {
            field: "legs",
            min: 2,
            max: usize::MAX,
            got: arr.len(),
        }),
        Some(_) => errs.push(SchemaError::WrongType {
            field: "legs",
            expected: "array<object>",
        }),
        None => errs.push(SchemaError::MissingField("legs")),
    }

    // base_mint / quote_mint: string
    for f in ["base_mint", "quote_mint"] {
        match obj.get(f).and_then(|v| v.as_str()) {
            Some(_) => {}
            None => errs.push(SchemaError::MissingField(f)),
        }
    }

    // gross_bps: integer
    match obj.get("gross_bps") {
        Some(serde_json::Value::Number(n)) if n.as_i64().is_some() => {}
        Some(_) => errs.push(SchemaError::WrongType {
            field: "gross_bps",
            expected: "integer",
        }),
        None => errs.push(SchemaError::MissingField("gross_bps")),
    }

    // fee_bps_sum: integer >= 0
    match obj.get("fee_bps_sum") {
        Some(serde_json::Value::Number(n)) if n.as_u64().is_some() => {}
        Some(_) => errs.push(SchemaError::WrongType {
            field: "fee_bps_sum",
            expected: "integer >= 0",
        }),
        None => errs.push(SchemaError::MissingField("fee_bps_sum")),
    }

    // decision: enum
    match obj.get("decision").and_then(|v| v.as_str()) {
        Some("WouldTrade") | Some("WouldNotTrade") => {}
        Some(s) => errs.push(SchemaError::Enum {
            field: "decision",
            value: s.to_string(),
        }),
        None => errs.push(SchemaError::MissingField("decision")),
    }

    // evaluator: enum
    match obj.get("evaluator").and_then(|v| v.as_str()) {
        Some("conservative_default") | Some("optimistic") => {}
        Some(s) => errs.push(SchemaError::Enum {
            field: "evaluator",
            value: s.to_string(),
        }),
        None => errs.push(SchemaError::MissingField("evaluator")),
    }

    // input_lamports / output_lamports: integer >= 0
    for f in ["input_lamports", "output_lamports"] {
        match obj.get(f) {
            Some(serde_json::Value::Number(n)) if n.as_u64().is_some() => {}
            Some(_) => errs.push(SchemaError::WrongType {
                field: f,
                expected: "integer >= 0",
            }),
            None => errs.push(SchemaError::MissingField(f)),
        }
    }

    // source_feed: enum
    match obj.get("source_feed").and_then(|v| v.as_str()) {
        Some("ws:mainnet") | Some("ws:devnet") | Some("capture:replay") => {}
        Some(s) => errs.push(SchemaError::Enum {
            field: "source_feed",
            value: s.to_string(),
        }),
        None => errs.push(SchemaError::MissingField("source_feed")),
    }

    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs)
    }
}

fn validate_leg(leg: &serde_json::Value, idx: usize, errs: &mut Vec<SchemaError>) {
    let field = |s: &str| -> String { format!("legs[{idx}].{s}") };
    let obj = match leg.as_object() {
        Some(o) => o,
        None => {
            errs.push(SchemaError::WrongType {
                field: "legs[]",
                expected: "object",
            });
            return;
        }
    };
    const ALLOWED: &[&str] = &["pool", "dex", "direction", "weight"];
    for k in obj.keys() {
        if !ALLOWED.contains(&k.as_str()) {
            errs.push(SchemaError::Unknown {
                field: field(k).leak(),
            });
        }
    }
    match obj.get("pool").and_then(|v| v.as_str()) {
        Some(s) if (32..=64).contains(&s.len()) => {}
        Some(s) => errs.push(SchemaError::Length {
            field: field("pool").leak(),
            min: 32,
            max: 64,
            got: s.len(),
        }),
        None => errs.push(SchemaError::MissingField(field("pool").leak())),
    }
    match obj.get("dex").and_then(|v| v.as_str()) {
        Some("raydium") | Some("orca") | Some("meteora") => {}
        Some(s) => errs.push(SchemaError::Enum {
            field: field("dex").leak(),
            value: s.to_string(),
        }),
        None => errs.push(SchemaError::MissingField(field("dex").leak())),
    }
    match obj.get("direction").and_then(|v| v.as_str()) {
        Some("BaseToQuote") | Some("QuoteToBase") => {}
        Some(s) => errs.push(SchemaError::Enum {
            field: field("direction").leak(),
            value: s.to_string(),
        }),
        None => errs.push(SchemaError::MissingField(field("direction").leak())),
    }
    match obj.get("weight") {
        Some(serde_json::Value::Number(n)) if n.as_i64().is_some() => {}
        Some(_) => errs.push(SchemaError::WrongType {
            field: field("weight").leak(),
            expected: "integer",
        }),
        None => errs.push(SchemaError::MissingField(field("weight").leak())),
    }
}

// ── Test helpers ──────────────────────────────────────────────────────

fn pool(addr: [u8; 32], kind: AmmKind, fee_bps: u16) -> Pool {
    Pool {
        address: CyclePubkey(addr),
        kind,
        base_mint: CyclePubkey([0x01; 32]),
        quote_mint: CyclePubkey([0x02; 32]),
        base_decimals: 6,
        quote_decimals: 9,
        base_reserve: 1_000_000_000,
        quote_reserve: 1_000_000_000,
        fee_bps,
        last_update_slot: 0,
        ..Default::default()
    }
}

/// Build a 2-leg cycle on the given pools. Each tuple is
/// `(pool_addr, direction, weight)`.
fn two_leg_cycle(legs: &[([u8; 32], Direction, i64)]) -> Cycle {
    let legs_v: Vec<Leg> = legs
        .iter()
        .map(|(addr, dir, w)| Leg {
            pool: CyclePubkey(*addr),
            direction: *dir,
            weight: *w,
        })
        .collect();
    let weight_sum: i64 = legs_v.iter().map(|l| l.weight).sum();
    Cycle {
        seq: 0,
        legs: legs_v,
        weight_sum,
        expected_profit_bps: 0,
    }
}

fn write_ctx(slot: u64) -> CycleWriteContext {
    CycleWriteContext {
        bot_run_id: Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
        source_feed: "ws:mainnet",
        detected_at_slot: slot,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[test]
fn writer_emits_valid_cycle_v1_record() {
    let pool_a = [0xA1u8; 32];
    let pool_b = [0xB2u8; 32];
    let mut reg = PoolRegistry::new();
    reg.insert(pool(pool_a, AmmKind::RaydiumAmmV4, 25));
    reg.insert(pool(pool_b, AmmKind::OrcaWhirlpool, 30));

    let cycle = two_leg_cycle(&[
        (pool_a, Direction::BaseToQuote, -100_000),
        (pool_b, Direction::QuoteToBase, -95_000),
    ]);
    let ctx = write_ctx(350_000_000);
    let v1 = build_cycle_v1_record(
        &cycle,
        &reg,
        &EvalParams::conservative_default(),
        1_000_000_000,
        1_005_000_000,
        ctx,
    );

    // Validate the emitted record against the contract schema.
    if let Err(errs) = validate_cycle_v1(&v1) {
        for e in &errs {
            eprintln!("  schema error: {e}");
        }
        panic!("cycle.v1 record did not validate");
    }
}

#[test]
fn writer_emits_valid_cycle_v1_record_for_three_leg() {
    // 3+ leg cycles: base_mint / quote_mint are best-effort and
    // should be empty strings (not "unknown"). The contract's
    // `minLength: 1` does NOT apply (it's not in the schema for
    // base/quote_mint — only for `pool`).
    let pool_a = [0xA1u8; 32];
    let pool_b = [0xB2u8; 32];
    let pool_c = [0xC3u8; 32];
    let mut reg = PoolRegistry::new();
    reg.insert(pool(pool_a, AmmKind::RaydiumAmmV4, 25));
    reg.insert(pool(pool_b, AmmKind::OrcaWhirlpool, 30));
    reg.insert(pool(pool_c, AmmKind::MeteoraDlmm, 20));

    let cycle = Cycle {
        seq: 0,
        legs: vec![
            Leg {
                pool: CyclePubkey(pool_a),
                direction: Direction::BaseToQuote,
                weight: -300_000,
            },
            Leg {
                pool: CyclePubkey(pool_b),
                direction: Direction::QuoteToBase,
                weight: -295_000,
            },
            Leg {
                pool: CyclePubkey(pool_c),
                direction: Direction::BaseToQuote,
                weight: -290_000,
            },
        ],
        weight_sum: -885_000,
        expected_profit_bps: 0,
    };
    let v1 = build_cycle_v1_record(
        &cycle,
        &reg,
        &EvalParams::conservative_default(),
        1_000_000_000,
        1_012_000_000,
        write_ctx(350_000_020),
    );
    if let Err(errs) = validate_cycle_v1(&v1) {
        for e in &errs {
            eprintln!("  schema error: {e}");
        }
        panic!("3-leg cycle.v1 record did not validate");
    }
    assert_eq!(v1.get("base_mint").and_then(|v| v.as_str()), Some(""));
    assert_eq!(v1.get("quote_mint").and_then(|v| v.as_str()), Some(""));
    // 3 distinct dexes, in first-seen order.
    assert_eq!(
        v1.get("dexes").and_then(|v| v.as_array()).map(|a| a.len()),
        Some(3)
    );
}

#[test]
fn writer_emits_well_formed_v0_shim() {
    // The shim keeps the v0 bridge running until DAM-44 lands.
    let shim = build_cycle_v0_shim("deadbeef", 50, 1_781_894_309_697);
    assert_eq!(shim.get("dex").and_then(|v| v.as_str()), Some("raydium"));
    assert_eq!(
        shim.get("base_mint").and_then(|v| v.as_str()),
        Some("unknown")
    );
    assert_eq!(
        shim.get("quote_mint").and_then(|v| v.as_str()),
        Some("unknown")
    );
    assert_eq!(
        shim.get("pool_address").and_then(|v| v.as_str()),
        Some("deadbeef")
    );
    assert_eq!(shim.get("fee_bps").and_then(|v| v.as_i64()), Some(30));
}

#[test]
fn append_cycle_jsonl_writes_two_files() {
    let tmp = std::env::temp_dir().join(format!("cycle_writer_test_{}", Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    let wallet_path = tmp.join("wallet.json");
    // Touch the wallet file so the .jsonl naming is meaningful.
    std::fs::write(&wallet_path, b"{}").unwrap();

    let pool_a = [0xA1u8; 32];
    let pool_b = [0xB2u8; 32];
    let mut reg = PoolRegistry::new();
    reg.insert(pool(pool_a, AmmKind::RaydiumAmmV4, 25));
    reg.insert(pool(pool_b, AmmKind::OrcaWhirlpool, 30));

    let cycle = two_leg_cycle(&[
        (pool_a, Direction::BaseToQuote, -100_000),
        (pool_b, Direction::QuoteToBase, -95_000),
    ]);
    append_cycle_jsonl(
        &wallet_path,
        &cycle,
        &reg,
        &EvalParams::conservative_default(),
        1_000_000_000,
        1_005_000_000,
        write_ctx(350_000_000),
    );

    let v1_path = tmp.join("cycles.v1.jsonl");
    let v0_path = tmp.join("cycles.jsonl");
    assert!(v1_path.exists(), "wallet.cycles.v1.jsonl was not written");
    assert!(
        v0_path.exists(),
        "wallet.cycles.jsonl (shim) was not written"
    );

    // Parse the v1 line, validate against the schema.
    let v1_text = std::fs::read_to_string(&v1_path).unwrap();
    let v1_line = v1_text.lines().next().unwrap();
    let v1_value: serde_json::Value = serde_json::from_str(v1_line).unwrap();
    if let Err(errs) = validate_cycle_v1(&v1_value) {
        for e in &errs {
            eprintln!("  schema error: {e}");
        }
        panic!("appended v1 line did not validate");
    }

    // Cleanup
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn cycle_id_is_deterministic() {
    // Re-running the writer with the same inputs must produce the
    // same `cycle_id`. This is the contract's idempotency clause;
    // it's what makes a 30-day backfill possible.
    let pool_a = [0xA1u8; 32];
    let pool_b = [0xB2u8; 32];
    let mut reg = PoolRegistry::new();
    reg.insert(pool(pool_a, AmmKind::RaydiumAmmV4, 25));
    reg.insert(pool(pool_b, AmmKind::OrcaWhirlpool, 30));

    let cycle = two_leg_cycle(&[
        (pool_a, Direction::BaseToQuote, -100_000),
        (pool_b, Direction::QuoteToBase, -95_000),
    ]);
    let v1_a = build_cycle_v1_record(
        &cycle,
        &reg,
        &EvalParams::conservative_default(),
        1_000_000_000,
        1_005_000_000,
        write_ctx(350_000_000),
    );
    let v1_b = build_cycle_v1_record(
        &cycle,
        &reg,
        &EvalParams::conservative_default(),
        1_000_000_000,
        1_005_000_000,
        write_ctx(350_000_000),
    );
    assert_eq!(
        v1_a.get("cycle_id").and_then(|v| v.as_str()),
        v1_b.get("cycle_id").and_then(|v| v.as_str()),
    );
}

#[test]
fn cycle_id_changes_with_slot() {
    // Different slot → different cycle_id. This is what makes the
    // bot's per-slot re-emission a fresh row rather than a duplicate.
    let pool_a = [0xA1u8; 32];
    let pool_b = [0xB2u8; 32];
    let mut reg = PoolRegistry::new();
    reg.insert(pool(pool_a, AmmKind::RaydiumAmmV4, 25));
    reg.insert(pool(pool_b, AmmKind::OrcaWhirlpool, 30));
    let cycle = two_leg_cycle(&[
        (pool_a, Direction::BaseToQuote, -100_000),
        (pool_b, Direction::QuoteToBase, -95_000),
    ]);
    let v1_a = build_cycle_v1_record(
        &cycle,
        &reg,
        &EvalParams::conservative_default(),
        1_000_000_000,
        1_005_000_000,
        write_ctx(1),
    );
    let v1_b = build_cycle_v1_record(
        &cycle,
        &reg,
        &EvalParams::conservative_default(),
        1_000_000_000,
        1_005_000_000,
        write_ctx(2),
    );
    assert_ne!(
        v1_a.get("cycle_id").and_then(|v| v.as_str()),
        v1_b.get("cycle_id").and_then(|v| v.as_str()),
    );
}

#[test]
fn rejects_fixture_fails_validation() {
    // The contract ships `tests/fixtures/cycle/v1/missing_schema.jsonl`
    // and `bad_legs.jsonl` as records that the pipeline must REJECT.
    // Sanity check: load each, run our validator, expect Err.
    let manifest = env!("CARGO_MANIFEST_DIR");
    let fixtures_dir = PathBuf::from(manifest).join("../tests/fixtures/cycle/v1");

    let missing = std::fs::read_to_string(fixtures_dir.join("missing_schema.jsonl")).unwrap();
    let v: serde_json::Value = serde_json::from_str(missing.lines().next().unwrap()).unwrap();
    assert!(
        validate_cycle_v1(&v).is_err(),
        "missing_schema should fail validation"
    );

    let bad_legs = std::fs::read_to_string(fixtures_dir.join("bad_legs.jsonl")).unwrap();
    let v: serde_json::Value = serde_json::from_str(bad_legs.lines().next().unwrap()).unwrap();
    assert!(
        validate_cycle_v1(&v).is_err(),
        "bad_legs (empty) should fail validation"
    );
}

#[test]
fn happy_fixture_passes_validation() {
    // The `happy.jsonl` fixture ships 10 valid records. Load them,
    // validate each, expect all Ok.
    let manifest = env!("CARGO_MANIFEST_DIR");
    let fixtures_dir = PathBuf::from(manifest).join("../tests/fixtures/cycle/v1");
    let text = std::fs::read_to_string(fixtures_dir.join("happy.jsonl")).unwrap();
    let mut count = 0;
    for line in text.lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        if let Err(errs) = validate_cycle_v1(&v) {
            for e in &errs {
                eprintln!("  schema error: {e}");
            }
            panic!("happy.jsonl line {count} did not validate");
        }
        count += 1;
    }
    assert_eq!(count, 10, "expected 10 lines in happy.jsonl");
}

#[test]
fn mapping_helpers_match_contract_enums() {
    // The contract enum is closed: `[raydium, orca, meteora]`. These
    // tests pin the helper mappings so an enum change breaks the
    // build, not production.
    assert_eq!(amm_kind_to_str(AmmKind::RaydiumAmmV4), "raydium");
    assert_eq!(amm_kind_to_str(AmmKind::OrcaWhirlpool), "orca");
    assert_eq!(amm_kind_to_str(AmmKind::MeteoraDlmm), "meteora");
    assert_eq!(direction_to_str(Direction::BaseToQuote), "BaseToQuote");
    assert_eq!(direction_to_str(Direction::QuoteToBase), "QuoteToBase");
    assert_eq!(
        evaluator_name(&EvalParams::conservative_default()),
        "conservative_default"
    );
    assert_eq!(evaluator_name(&EvalParams::optimistic()), "optimistic");
}

#[test]
fn pool_lookup_falls_back_to_raydium_v4() {
    // A pool not in the registry falls back to Raydium AMM v4 / 30
    // bps. This preserves the v0 writer's behaviour and keeps the
    // v1 schema's `dex` enum invariant.
    let reg = PoolRegistry::new();
    let (kind, fee) = leg_pool_lookup(&reg, &[0xFF; 32]);
    assert_eq!(kind, AmmKind::RaydiumAmmV4);
    assert_eq!(fee, 30);
}

#[test]
fn write_line_appends_one_jsonl_line() {
    // The low-level writer must append a single newline-terminated
    // JSON object. No trailing whitespace, no extra lines.
    let tmp = std::env::temp_dir().join(format!("wlt_{}", Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    let path = tmp.join("x.jsonl");
    let v = serde_json::json!({"a": 1});
    write_line(&path, &v).unwrap();
    write_line(&path, &v).unwrap();
    let s = std::fs::read_to_string(&path).unwrap();
    assert_eq!(s, "{\"a\":1}\n{\"a\":1}\n");
    let _ = std::fs::remove_dir_all(&tmp);
}
