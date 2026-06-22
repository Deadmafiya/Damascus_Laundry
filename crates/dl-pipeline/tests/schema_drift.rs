//! Schema-drift CI guard (DAM-46 §"CI gates").
//!
//! The spec says: if `docs/contracts/cycle.v1.schema.json` is touched,
//! the contract doc (`docs/contracts/cycle.v1.md`) must be touched in
//! the same commit. We can't enforce that in `cargo test` (no commit
//! context), but we can enforce the cheap invariant: the
//! `cycle.v1.schema.json` file lists every required field, and the
//! `cycle.v1.md` contract doc mentions each of those fields in the
//! prose. A schema field added without a doc update fails this test.

use std::fs;
use std::path::Path;

fn required_fields_from_schema() -> Vec<String> {
    let schema_path = Path::new("../../docs/contracts/cycle.v1.schema.json");
    let s = fs::read_to_string(schema_path).expect("read cycle.v1.schema.json");
    let v: serde_json::Value = serde_json::from_str(&s).expect("parse schema");
    v.get("required")
        .and_then(|r| r.as_array())
        .expect("required array")
        .as_slice()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect()
}

fn contract_doc_text() -> String {
    let doc_path = Path::new("../../docs/contracts/cycle.v1.md");
    fs::read_to_string(doc_path)
        .expect("read cycle.v1.md")
        .to_lowercase()
}

#[test]
fn every_required_field_is_mentioned_in_the_contract_doc() {
    let fields = required_fields_from_schema();
    let doc = contract_doc_text();
    for f in &fields {
        assert!(
            doc.contains(&f.to_lowercase()),
            "required field `{f}` is not mentioned in docs/contracts/cycle.v1.md — \
             update the contract doc in the same commit as the schema change (DAM-46 §CI gates)"
        );
    }
}
