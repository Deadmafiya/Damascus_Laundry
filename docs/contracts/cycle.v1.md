# Contract: `cycle.v1`

Status: **draft** (Data, DAM-41 / DAM-46). Final after CTO sign-off.
Generated JSON Schema: see `cycle.v1.schema.json` (next to this file; generated, do not hand-edit).

## Wire format

- One JSON object per line.
- File extension: `.jsonl` or `.jsonl.gz`. The pipeline accepts both.
- Encoding: UTF-8, no BOM.
- Trailing newline required per line. A final newline at EOF is optional.

## Envelope

Every line MUST start with:

```json
{ "schema": "cycle.v1", ... }
```

## Required fields

The `cycle.v1` record carries every field the pipeline needs to
ingest it without loss. The required fields (from
`cycle.v1.schema.json` `required` array) are:

| Field | Type | Meaning |
|-------|------|---------|
| `schema` | const `"cycle.v1"` | Contract version sentinel. |
| `cycle_id` | 64 lowercase hex chars | Stable hash of the cycle. `blake3(sorted_legs_json || detected_at_slot)`. The pipeline dedups on this. |
| `detected_at_unix_ms` | non-negative integer | UTC ms epoch. Partition key. |
| `detected_at_slot` | non-negative integer | Solana slot at detection. |
| `bot_run_id` | UUIDv4 string | The bot run that emitted the record. |
| `dexes` | array of enum | Distinct DEXes touched. Closed enum: `raydium`, `orca`, `meteora`. |
| `legs` | array of leg objects | Cycle path in order. Each leg: `pool`, `dex`, `direction`, `weight`. |
| `base_mint` | string | Base mint pubkey (base58). |
| `quote_mint` | string | Quote mint pubkey (base58). |
| `gross_bps` | integer (signed) | Gross profit in basis points, before fees. |
| `fee_bps_sum` | non-negative integer | Sum of fee basis points across legs. |
| `decision` | enum | `WouldTrade` or `WouldNotTrade`. |
| `evaluator` | enum | The named `EvalParams` variant: `conservative_default`, `optimistic`. |
| `input_lamports` | non-negative integer | Lamports in. |
| `output_lamports` | non-negative integer | Lamports out. |
| `source_feed` | enum | The feed that sourced the prices: `ws:mainnet`, `ws:devnet`, `capture:replay`. |

## Leg object

Each entry in `legs` is:

```json
{ "pool": "<base58 pubkey>", "dex": "raydium|orca|meteora", "direction": "BaseToQuote|QuoteToBase", "weight": <integer> }
```

`weight` is rendered as a JSON integer. The bot uses a `1e18`-scaled
fixed point, so the typed in-memory column is `i64` (a real cycle
weight is well within `i64::MAX`).

## Reject reasons

When a line fails validation, the pipeline writes a `Reject` row to
`dl_pipeline_rejects/<pipeline_run_id>.jsonl` with one of the
following canonical reasons (closed enum — adding a variant is a
contract change):

| Reason string | When |
|---------------|------|
| `schema_missing` | No `schema` field on the line. |
| `schema_unknown` | `schema` is not `cycle.v1` / `trade.v1`. |
| `cycle_id_invalid` | `cycle_id` is not 64 lowercase hex chars. |
| `legs_empty` | `legs` array is empty (must be ≥ 2). |
| `decision_invalid` | `decision` not in enum. |
| `evaluator_invalid` | `evaluator` not a known `EvalParams` variant. |
| `source_feed_invalid` | `source_feed` not in enum. |
| `direction_invalid` | `legs[i].direction` not in enum. |
| `dex_invalid` | `legs[i].dex` or `dexes[i]` not in enum. |
| `field_type_wrong` | A required field failed its type check. |
| `private_field` | A field name starts with `_priv_`. |
| `signing_material` | The line matched the signing-material regex. |
| `not_json` | The line was not valid JSON. |

## Privacy

The validator rejects any line whose field names start with `_priv_`
or that match the `signing_material` regex (defensive; the writer
should never emit such a field). Pipeline policy per
`docs/architecture/data-architecture-v1.md` §4.

## Versioning

The `schema` field is the version sentinel. The pipeline rejects any
line whose `schema` is not the literal `"cycle.v1"`. A breaking change
to the wire shape is a new schema (e.g. `cycle.v2`) and a new
contract doc. The old contract is supported for at least one release
on the read path (the writer emits both for one release as a
fallback).
