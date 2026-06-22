//! Hand-rolled `cycle.v1` / `trade.v1` validator.
//!
//! We do not depend on the `jsonschema` crate (it is not in the offline
//! cargo cache as of 2026-06-21). Instead we implement a strict,
//! closed-enum validator that covers every reject reason listed in
//! `docs/contracts/cycle.v1.md`.

use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;
use thiserror::Error;

use crate::reject::RejectReason;

/// One validate failure. The `reject_reason` is the canonical enum the
/// reject writer uses; the `message` is the human-readable detail.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("validation failed ({reason}): {message}")]
pub struct ValidationError {
    pub reason: RejectReason,
    pub message: String,
}

impl ValidationError {
    pub fn new(reason: RejectReason, message: impl Into<String>) -> Self {
        ValidationError {
            reason,
            message: message.into(),
        }
    }
}

/// Regex for plaintext signing-material detection. Defensive.
static SIGNING_MATERIAL: OnceLock<Regex> = OnceLock::new();

fn signing_material_re() -> &'static Regex {
    SIGNING_MATERIAL.get_or_init(|| {
        Regex::new(
            r"(?x)
        \b[0-9a-fA-F]{64}\b
        |
        \b[1-9A-HJ-NP-Za-km-z]{64,88}\b
    ",
        )
        .expect("static regex compiles")
    })
}

/// Validate a parsed `cycle.v1` JSON value.
pub fn validate_cycle_v1(value: &Value) -> Result<(), ValidationError> {
    let schema = value
        .get("schema")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ValidationError::new(RejectReason::SchemaMissing, "`schema` field missing"))?;
    if schema != "cycle.v1" {
        return Err(ValidationError::new(
            RejectReason::SchemaUnknown,
            format!("unknown schema `{schema}` (expected `cycle.v1`)"),
        ));
    }

    let cycle_id = value
        .get("cycle_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ValidationError::new(RejectReason::CycleIdInvalid, "`cycle_id` not a string"))?;
    if !is_blake3_hex(cycle_id) {
        return Err(ValidationError::new(
            RejectReason::CycleIdInvalid,
            format!("`cycle_id` must be 64 lowercase hex chars; got len={}", cycle_id.len()),
        ));
    }

    let ts = value
        .get("detected_at_unix_ms")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| {
            ValidationError::new(RejectReason::FieldTypeWrong, "`detected_at_unix_ms` not an integer")
        })?;
    if ts < 0 {
        return Err(ValidationError::new(
            RejectReason::FieldTypeWrong,
            "`detected_at_unix_ms` is negative",
        ));
    }

    let legs = value
        .get("legs")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ValidationError::new(RejectReason::FieldTypeWrong, "`legs` not an array"))?;
    if legs.is_empty() {
        return Err(ValidationError::new(
            RejectReason::LegsEmpty,
            "`legs` array is empty",
        ));
    }
    for (i, leg) in legs.iter().enumerate() {
        validate_leg(leg, i)?;
    }

    let dexes = value
        .get("dexes")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ValidationError::new(RejectReason::FieldTypeWrong, "`dexes` not an array"))?;
    if dexes.is_empty() {
        return Err(ValidationError::new(
            RejectReason::FieldTypeWrong,
            "`dexes` array is empty",
        ));
    }
    for (i, d) in dexes.iter().enumerate() {
        let s = d
            .as_str()
            .ok_or_else(|| ValidationError::new(RejectReason::FieldTypeWrong, "`dexes[i]` not a string"))?;
        if !is_valid_dex(s) {
            return Err(ValidationError::new(
                RejectReason::DexInvalid,
                format!("`dexes[{i}] = {s}` not in enum"),
            ));
        }
    }

    for (field, must_be_nonneg) in [
        ("detected_at_slot", true),
        ("gross_bps", false),
        ("fee_bps_sum", true),
        ("input_lamports", true),
        ("output_lamports", true),
    ] {
        let n = value.get(field).and_then(|v| v.as_i64()).ok_or_else(|| {
            ValidationError::new(
                RejectReason::FieldTypeWrong,
                format!("`{field}` not an integer"),
            )
        })?;
        if must_be_nonneg && n < 0 {
            return Err(ValidationError::new(
                RejectReason::FieldTypeWrong,
                format!("`{field}` is negative"),
            ));
        }
    }

    let decision = value
        .get("decision")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ValidationError::new(RejectReason::FieldTypeWrong, "`decision` not a string"))?;
    if !is_valid_decision(decision) {
        return Err(ValidationError::new(
            RejectReason::DecisionInvalid,
            format!("`decision` = `{decision}` not in enum"),
        ));
    }
    let evaluator = value
        .get("evaluator")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ValidationError::new(RejectReason::FieldTypeWrong, "`evaluator` not a string"))?;
    if !is_valid_evaluator(evaluator) {
        return Err(ValidationError::new(
            RejectReason::EvaluatorInvalid,
            format!("`evaluator` = `{evaluator}` not in enum"),
        ));
    }
    let source_feed = value
        .get("source_feed")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ValidationError::new(RejectReason::FieldTypeWrong, "`source_feed` not a string"))?;
    if !is_valid_source_feed(source_feed) {
        return Err(ValidationError::new(
            RejectReason::SourceFeedInvalid,
            format!("`source_feed` = `{source_feed}` not in enum"),
        ));
    }

    let bot_run_id = value
        .get("bot_run_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ValidationError::new(RejectReason::FieldTypeWrong, "`bot_run_id` not a string"))?;
    if bot_run_id.is_empty() {
        return Err(ValidationError::new(
            RejectReason::FieldTypeWrong,
            "`bot_run_id` is empty",
        ));
    }

    if let Some(obj) = value.as_object() {
        for k in obj.keys() {
            if k.starts_with("_priv_") {
                return Err(ValidationError::new(
                    RejectReason::PrivateField,
                    format!("field `{k}` starts with `_priv_`"),
                ));
            }
        }
    }

    Ok(())
}

fn validate_leg(leg: &Value, idx: usize) -> Result<(), ValidationError> {
    let obj = leg.as_object().ok_or_else(|| {
        ValidationError::new(RejectReason::FieldTypeWrong, format!("`legs[{idx}]` not an object"))
    })?;
    let pool = obj
        .get("pool")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ValidationError::new(RejectReason::FieldTypeWrong, format!("`legs[{idx}].pool` not a string")))?;
    if pool.len() < 32 || pool.len() > 64 {
        return Err(ValidationError::new(
            RejectReason::FieldTypeWrong,
            format!("`legs[{idx}].pool` length {} not in [32, 64]", pool.len()),
        ));
    }
    let dex = obj
        .get("dex")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ValidationError::new(RejectReason::FieldTypeWrong, format!("`legs[{idx}].dex` not a string")))?;
    if !is_valid_dex(dex) {
        return Err(ValidationError::new(
            RejectReason::DexInvalid,
            format!("`legs[{idx}].dex = {dex}` not in enum"),
        ));
    }
    let direction = obj
        .get("direction")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ValidationError::new(RejectReason::FieldTypeWrong, format!("`legs[{idx}].direction` not a string")))?;
    if !is_valid_direction(direction) {
        return Err(ValidationError::new(
            RejectReason::DirectionInvalid,
            format!("`legs[{idx}].direction = {direction}` not in enum"),
        ));
    }
    let weight = obj
        .get("weight")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| ValidationError::new(RejectReason::FieldTypeWrong, format!("`legs[{idx}].weight` not an integer")))?;
    let _ = weight;
    Ok(())
}

fn is_valid_dex(s: &str) -> bool {
    matches!(s, "raydium" | "orca" | "meteora")
}

fn is_valid_decision(s: &str) -> bool {
    matches!(s, "WouldTrade" | "WouldNotTrade")
}

fn is_valid_evaluator(s: &str) -> bool {
    matches!(s, "conservative_default" | "optimistic")
}

fn is_valid_direction(s: &str) -> bool {
    matches!(s, "BaseToQuote" | "QuoteToBase")
}

fn is_valid_source_feed(s: &str) -> bool {
    matches!(s, "ws:mainnet" | "ws:devnet" | "capture:replay")
}

fn is_blake3_hex(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// Public: scan a raw line for signing-material matches.
pub fn line_contains_signing_material(line: &str) -> bool {
    signing_material_re().is_match(line)
        && (line.contains("private_key")
            || line.contains("secret_key")
            || line.contains("seed")
            || line.contains("mnemonic")
            || line.contains("passphrase"))
}

/// Validate a parsed `trade.v1` JSON value.
pub fn validate_trade_v1(value: &Value) -> Result<(), ValidationError> {
    let schema = value
        .get("schema")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ValidationError::new(RejectReason::SchemaMissing, "`schema` field missing"))?;
    if schema != "trade.v1" {
        return Err(ValidationError::new(
            RejectReason::SchemaUnknown,
            format!("unknown schema `{schema}` (expected `trade.v1`)"),
        ));
    }
    for field in ["trade_id", "cycle_id", "bot_run_id", "decision", "evaluator"] {
        let s = value.get(field).and_then(|v| v.as_str()).ok_or_else(|| {
            ValidationError::new(
                RejectReason::FieldTypeWrong,
                format!("`{field}` not a string"),
            )
        })?;
        if s.is_empty() {
            return Err(ValidationError::new(
                RejectReason::FieldTypeWrong,
                format!("`{field}` is empty"),
            ));
        }
    }
    let ts = value
        .get("ts_unix_ms")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| {
            ValidationError::new(RejectReason::FieldTypeWrong, "`ts_unix_ms` not an integer")
        })?;
    if ts < 0 {
        return Err(ValidationError::new(
            RejectReason::FieldTypeWrong,
            "`ts_unix_ms` is negative",
        ));
    }
    for field in ["input_lamports", "output_lamports"] {
        let n = value.get(field).and_then(|v| v.as_i64()).ok_or_else(|| {
            ValidationError::new(
                RejectReason::FieldTypeWrong,
                format!("`{field}` not an integer"),
            )
        })?;
        if n < 0 {
            return Err(ValidationError::new(
                RejectReason::FieldTypeWrong,
                format!("`{field}` is negative"),
            ));
        }
    }
    let decision = value
        .get("decision")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ValidationError::new(RejectReason::FieldTypeWrong, "`decision` not a string"))?;
    if !is_valid_decision(decision) {
        return Err(ValidationError::new(
            RejectReason::DecisionInvalid,
            format!("`decision` = `{decision}` not in enum"),
        ));
    }
    let evaluator = value
        .get("evaluator")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ValidationError::new(RejectReason::FieldTypeWrong, "`evaluator` not a string"))?;
    if !is_valid_evaluator(evaluator) {
        return Err(ValidationError::new(
            RejectReason::EvaluatorInvalid,
            format!("`evaluator` = `{evaluator}` not in enum"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn happy_cycle() -> Value {
        json!({
            "schema": "cycle.v1",
            "cycle_id": "0".repeat(64),
            "detected_at_unix_ms": 1782000000000_i64,
            "detected_at_slot": 312345678_u64,
            "bot_run_id": "550e8400-e29b-41d4-a716-446655440000",
            "dexes": ["raydium", "orca"],
            "legs": [
                {"pool": "58oQChx4yWmvKdwLLZzBi4ChoCc2fqCUWBkwMihLYQo2", "dex": "raydium", "direction": "BaseToQuote", "weight": 3000000000000000_i64},
                {"pool": "Czfq3xZZDmsdGdUyrNLtRhGc47cXcZtLG4crryfu44zE", "dex": "orca", "direction": "QuoteToBase", "weight": -1750000000000000000_i64}
            ],
            "base_mint": "So11111111111111111111111111111111111111112",
            "quote_mint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
            "gross_bps": 17470,
            "fee_bps_sum": 60,
            "decision": "WouldTrade",
            "evaluator": "conservative_default",
            "input_lamports": 1000000000,
            "output_lamports": 1174700000,
            "source_feed": "ws:mainnet"
        })
    }

    #[test]
    fn happy_cycle_validates() {
        assert!(validate_cycle_v1(&happy_cycle()).is_ok());
    }

    #[test]
    fn missing_schema_is_rejected() {
        let mut v = happy_cycle();
        v.as_object_mut().unwrap().remove("schema");
        let err = validate_cycle_v1(&v).unwrap_err();
        assert_eq!(err.reason, RejectReason::SchemaMissing);
    }

    #[test]
    fn unknown_schema_is_rejected() {
        let mut v = happy_cycle();
        v["schema"] = json!("cycle.v2");
        let err = validate_cycle_v1(&v).unwrap_err();
        assert_eq!(err.reason, RejectReason::SchemaUnknown);
    }

    #[test]
    fn bad_cycle_id_is_rejected() {
        let mut v = happy_cycle();
        v["cycle_id"] = json!("not-hex");
        let err = validate_cycle_v1(&v).unwrap_err();
        assert_eq!(err.reason, RejectReason::CycleIdInvalid);
    }

    #[test]
    fn empty_legs_is_rejected() {
        let mut v = happy_cycle();
        v["legs"] = json!([]);
        let err = validate_cycle_v1(&v).unwrap_err();
        assert_eq!(err.reason, RejectReason::LegsEmpty);
    }

    #[test]
    fn bad_direction_is_rejected() {
        let mut v = happy_cycle();
        v["legs"][0]["direction"] = json!("sideways");
        let err = validate_cycle_v1(&v).unwrap_err();
        assert_eq!(err.reason, RejectReason::DirectionInvalid);
    }

    #[test]
    fn bad_dex_is_rejected() {
        let mut v = happy_cycle();
        v["legs"][0]["dex"] = json!("uniswap");
        let err = validate_cycle_v1(&v).unwrap_err();
        assert_eq!(err.reason, RejectReason::DexInvalid);
    }

    #[test]
    fn bad_decision_is_rejected() {
        let mut v = happy_cycle();
        v["decision"] = json!("Maybe");
        let err = validate_cycle_v1(&v).unwrap_err();
        assert_eq!(err.reason, RejectReason::DecisionInvalid);
    }

    #[test]
    fn bad_evaluator_is_rejected() {
        let mut v = happy_cycle();
        v["evaluator"] = json!("vibes");
        let err = validate_cycle_v1(&v).unwrap_err();
        assert_eq!(err.reason, RejectReason::EvaluatorInvalid);
    }

    #[test]
    fn bad_source_feed_is_rejected() {
        let mut v = happy_cycle();
        v["source_feed"] = json!("ws:testnet");
        let err = validate_cycle_v1(&v).unwrap_err();
        assert_eq!(err.reason, RejectReason::SourceFeedInvalid);
    }

    #[test]
    fn private_field_is_rejected() {
        let mut v = happy_cycle();
        v["_priv_balance"] = json!(1000);
        let err = validate_cycle_v1(&v).unwrap_err();
        assert_eq!(err.reason, RejectReason::PrivateField);
    }

    #[test]
    fn line_contains_signing_material_flags_secrets() {
        // 64-char base58-valid string (alphabet excludes 0, O, I, l).
        let bad = r#"{"private_key":"5aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;
        assert!(line_contains_signing_material(bad));
        let good = r#"{"bot_run_id":"550e8400-e29b-41d4-a716-446655440000"}"#;
        assert!(!line_contains_signing_material(good));
    }
}
