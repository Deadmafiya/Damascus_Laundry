# Research: Raydium AMM v4 (`raydium-amm`) — program ID + `AmmInfo` layout

Captured during Phase 2 task 02-02-02. All values verified against the
upstream source as of the date below. If the upstream struct changes,
re-verify and update this file **before** updating the decoder.

## Sources

| | |
|---|---|
| Repo | https://github.com/raydium-io/raydium-amm |
| Source file | `program/src/state.rs` |
| Branch / ref | `master` (HEAD at fetch time) |
| Fetched | 2026-06-17 (this session) |
| Local copy | `crates/dl-state/docs/raydium_state.rs` |

## Constants

| Field | Value | Source |
|---|---|---|
| Raydium AMM v4 program ID (mainnet) | `675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8` | confirmed via `getAccountInfo` (executable=true) |
| Default `trade_fee` | `25 / 10000` (0.25%) | `Fees::initialize` in state.rs |
| Default `swap_fee` | `25 / 10000` (0.25%) | same |
| Default `pnl` | `12 / 100` (12%) | same |
| Default `min_separate` | `5 / 10000` (0.05%) | same |
| AmmInfo total size | **752 bytes** | computed from `#[repr(C, packed)]` field layout |
| Endianness | little-endian | Solana target; `#[cfg(target_endian = "little")]` in source |

## `AmmInfo` byte offsets (packed, LE)

| Offset (bytes) | Size | Type | Field | Notes |
|---:|---:|---|---|---|
| 0 | 8 | u64 | `status` | `AmmStatus` enum; we accept ≥ 1 (Initialized) |
| 8 | 8 | u64 | `nonce` | PDA nonce for the pool authority |
| 32 | 8 | u64 | `coin_decimals` | **BASE mint decimals** |
| 40 | 8 | u64 | `pc_decimals` | **QUOTE mint decimals** |
| 144 | 8 | u64 | `fees.trade_fee_numerator` | `n` in `n/d` |
| 152 | 8 | u64 | `fees.trade_fee_denominator` | `d` in `n/d` |
| 336 | 32 | Pubkey | `coin_vault` | **BASE vault** (SPL token account) |
| 368 | 32 | Pubkey | `pc_vault` | **QUOTE vault** (SPL token account) |
| 400 | 32 | Pubkey | `coin_vault_mint` | **BASE mint pubkey** |
| 432 | 32 | Pubkey | `pc_vault_mint` | **QUOTE mint pubkey** |
| 720 | 8 | u64 | `lp_amount` | total LP supply (for completeness) |

(Full 54-field layout is in `crates/dl-state/docs/raydium_state.rs`; we
read only the offsets above in the decoder.)

## Reserves are NOT in `AmmInfo`

This is the critical point: the pool's *on-hand token reserves* are
stored in the two SPL token accounts `coin_vault` and `pc_vault`, **not**
in the `AmmInfo` blob. To assemble a complete `Pool` we need:

1. Decode `AmmInfo` (752 bytes, owned by
   `675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8`) → mints, vaults,
   decimals, fee fraction.
2. Fetch the token account at `coin_vault` → `base_reserve` (u64 base
   units) from its `amount` field (offset 64 in the SPL token state).
3. Fetch the token account at `pc_vault` → `quote_reserve` (u64 base
   units) similarly.

The mint *decimals* are present in `AmmInfo` itself (offsets 32 and 40),
so the second `getAccountInfo` on the mint is **not** required for
decimals — only if we want to fully reconstruct the mint metadata.

## Discriminator

`AmmInfo` does **not** start with a discriminator byte. The pool is
identified by:

- The account's owner: must equal the Raydium AMM v4 program ID
  (`675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8`).
- `data_len() == 752`.
- `status` is in `{1..=7}` (per `AmmStatus::valid_status`).

The decoder must enforce all three. A mismatch on owner or data_len
returns `DecodeError::BadDiscriminator` with the offending bytes for
diagnostics; a 0 status returns `DecodeError::BadDiscriminator` with
`expected = [1,0,0,...]` for symmetry.

## Verification procedure used

```bash
curl -sS https://raw.githubusercontent.com/raydium-io/raydium-amm/master/program/src/state.rs \
  > crates/dl-state/docs/raydium_state.rs
# (offsets computed by walking the #[repr(C, packed)] struct field-by-field)
# Pack::LEN for Fees = 64 confirmed at line ~570 of state.rs
```

The 752-byte total was also cross-checked by hand: 16 `u64` (128 B) +
`Fees` (8 × 8 = 64 B) + `StateData` (5 u64 + 2 u64 padding + 1 u64 + 4
u128 + 2 u64 = 40+16+8+64+16 = 144 B) + 2 vault Pubkeys (64 B) + 2
vault-mint Pubkeys (64 B) + 4 more Pubkeys (128 B) + `padding1`
`[u64; 8]` (64 B) + `amm_owner` (32 B) + 3 u64 (24 B) + 1 u64 padding
(8 B) = 752 B.
