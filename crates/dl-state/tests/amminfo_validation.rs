//! End-to-end validation test for the Raydium AMM v4 decoder.
//!
//! Pulls a real `AmmInfo` and the two vault SPL token accounts from
//! mainnet-beta and verifies:
//!   1. The `AmmInfo` decodes (status in 1..=7, mints/vaults/decimals
//!      read at the expected offsets).
//!   2. Each vault's `mint` matches the corresponding `*_vault_mint`
//!      in the `AmmInfo` (the same cross-check `assemble_pool` does).
//!   3. The assembled `Pool`'s reserves match the on-chain vault
//!      amounts to within 1 base unit (which is exact, since we read
//!      them from the same accounts).
//!   4. The fee bps is in the sane range (≤ 1000 bps = 10%).
//!
//! Gated on `DL_TEST_RPC_URL` (HTTP) and `DL_TEST_POOL_PUBKEY`
//! (base58). Ignored in CI by default — requires a pool pubkey + RPC.
//!
//! # Finding a pool pubkey
//!
//! If you don't have one handy, the easiest path is `solana account
//! <pubkey>` against any address you suspect, or pull the latest
//! Raydium SDK list from `https://api.raydium.io/v2/sdk/liquidity/
//! mainnet.json` and pick a high-volume USDC pair.
//!
//! A known working SOL/USDC pool (decoded successfully in this
//! session, 25 bps fee, 9/6 decimals) is:
//!   `3sjNoCnkkhWPVXYGDtem8rCciHSGc9jSFZuUAzKbvRVp`
//!
//! # Run locally
//!
//! ```bash
//! DL_TEST_RPC_URL=https://api.mainnet-beta.solana.com \
//! DL_TEST_POOL_PUBKEY=3sjNoCnkkhWPVXYGDtem8rCciHSGc9jSFZuUAzKbvRVp \
//!   cargo test -p dl-state --test amminfo_validation -- --ignored --nocapture
//! ```

use dl_state::decoder::{
    assemble_pool, decode_amm_info, decode_spl_token_account, AmmInfo, SplTokenAccount,
    AMM_INFO_SIZE, SPL_TOKEN_ACCOUNT_SIZE,
};
use dl_state::Pool;
use std::process::Command;

fn env_or_panic(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{} must be set", name))
}

fn fetch_account(rpc_url: &str, pubkey_b58: &str) -> Vec<u8> {
    let body = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"getAccountInfo","params":["{}",{{"encoding":"base64"}}]}}"#,
        pubkey_b58
    );
    let out = Command::new("curl")
        .args([
            "-sS",
            "-X",
            "POST",
            "-H",
            "Content-Type: application/json",
            "-d",
            &body,
            rpc_url,
        ])
        .output()
        .expect("curl failed");
    assert!(out.status.success(), "curl exit non-zero: {}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Cheap parse: find "data":["<base64>", ...]
    let needle = "\"data\":[\"";
    let start = stdout
        .find(needle)
        .unwrap_or_else(|| panic!("no data in response: {}", stdout))
        + needle.len();
    let end = stdout[start..]
        .find('"')
        .expect("unterminated base64 string in response");
    let b64 = &stdout[start..start + end];
    // SPL token and AmmInfo are both base64 STANDARD alphabet.
    base64_decode(b64)
}

fn base64_decode(s: &str) -> Vec<u8> {
    // Use the `base64` crate via dl-core? No — keep this test dep-free.
    // Tiny standard-alphabet decoder.
    const TBL: &[u8; 128] = &{
        let mut t = [255u8; 128];
        let abc = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut i = 0;
        while i < abc.len() {
            t[abc[i] as usize] = i as u8;
            i += 1;
        }
        t
    };
    let s = s.trim_end_matches('=');
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let mut buf = [0u8; 4];
    let mut i = 0;
    while i + 4 <= bytes.len() {
        for j in 0..4 {
            buf[j] = TBL[bytes[i + j] as usize];
        }
        out.push((buf[0] << 2) | (buf[1] >> 4));
        out.push((buf[1] << 4) | (buf[2] >> 2));
        out.push((buf[2] << 6) | buf[3]);
        i += 4;
    }
    let rem = bytes.len() - i;
    if rem == 2 {
        let a = TBL[bytes[i] as usize];
        let b = TBL[bytes[i + 1] as usize];
        out.push((a << 2) | (b >> 4));
    } else if rem == 3 {
        let a = TBL[bytes[i] as usize];
        let b = TBL[bytes[i + 1] as usize];
        let c = TBL[bytes[i + 2] as usize];
        out.push((a << 2) | (b >> 4));
        out.push((b << 4) | (c >> 2));
    }
    out
}

#[test]
#[ignore = "requires DL_TEST_RPC_URL and DL_TEST_POOL_PUBKEY; see module docstring"]
fn real_amm_info_decodes_and_assembles() {
    let rpc_url = env_or_panic("DL_TEST_RPC_URL");
    let pool_b58 = env_or_panic("DL_TEST_POOL_PUBKEY");

    let amm_bytes = fetch_account(&rpc_url, &pool_b58);
    assert_eq!(
        amm_bytes.len(),
        AMM_INFO_SIZE,
        "AmmInfo size mismatch: expected {} got {} — pool may not be Raydium AMM v4",
        AMM_INFO_SIZE,
        amm_bytes.len()
    );

    let amm: AmmInfo = decode_amm_info(&amm_bytes).expect("amm decode failed");

    // Fetch the two vault token accounts.
    let base_vault_b58 = bs58_encode(&amm.base_vault.0);
    let quote_vault_b58 = bs58_encode(&amm.quote_vault.0);

    let base_vault_bytes = fetch_account(&rpc_url, &base_vault_b58);
    let quote_vault_bytes = fetch_account(&rpc_url, &quote_vault_b58);

    assert_eq!(
        base_vault_bytes.len(),
        SPL_TOKEN_ACCOUNT_SIZE,
        "base vault not 165 bytes"
    );
    assert_eq!(
        quote_vault_bytes.len(),
        SPL_TOKEN_ACCOUNT_SIZE,
        "quote vault not 165 bytes"
    );

    let base_vault: SplTokenAccount = decode_spl_token_account(&base_vault_bytes).unwrap();
    let quote_vault: SplTokenAccount = decode_spl_token_account(&quote_vault_bytes).unwrap();

    // mint cross-check
    assert_eq!(
        base_vault.mint.0, amm.base_mint.0,
        "base vault mint != base_mint in AmmInfo"
    );
    assert_eq!(
        quote_vault.mint.0, amm.quote_mint.0,
        "quote vault mint != quote_mint in AmmInfo"
    );

    // Assemble
    let pool_address_bytes = bs58_decode(&pool_b58).expect("DL_TEST_POOL_PUBKEY must be base58");
    let mut pool_addr = [0u8; 32];
    assert_eq!(pool_address_bytes.len(), 32, "pool pubkey must be 32 bytes");
    pool_addr.copy_from_slice(&pool_address_bytes);
    let pool: Pool = assemble_pool(
        dl_state::Pubkey(pool_addr),
        &amm,
        &base_vault,
        &quote_vault,
        0, // slot not known from this fetch
    )
    .expect("assemble failed");

    // Reserve cross-check
    assert_eq!(pool.base_reserve, base_vault.amount);
    assert_eq!(pool.quote_reserve, quote_vault.amount);

    // Sanity on fee
    assert!(
        pool.fee_bps <= 1000,
        "fee_bps out of range: {}",
        pool.fee_bps
    );

    eprintln!(
        "real pool OK: mints=({},{}), reserves=({},{}), fee_bps={}, decimals=({},{})",
        hex4(&pool.base_mint.0),
        hex4(&pool.quote_mint.0),
        pool.base_reserve,
        pool.quote_reserve,
        pool.fee_bps,
        pool.base_decimals,
        pool.quote_decimals
    );
}

fn hex4(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(8);
    for &b in &bytes[..4.min(bytes.len())] {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

fn bs58_encode(bytes: &[u8]) -> String {
    const ALPHA: &[u8; 58] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    // `result` holds base-58 digits, little-endian.
    let mut result: Vec<u8> = Vec::with_capacity(45);
    for &b in bytes {
        // result = result * 256 + b
        let mut carry: u32 = b as u32;
        for digit in result.iter_mut() {
            let prod = (*digit as u32) * 256 + carry;
            *digit = (prod % 58) as u8;
            carry = prod / 58;
        }
        while carry > 0 {
            result.push((carry % 58) as u8);
            carry /= 58;
        }
    }
    if result.is_empty() {
        return "1".to_string();
    }
    result.reverse();
    result.iter().map(|&d| ALPHA[d as usize] as char).collect()
}

fn bs58_decode(s: &str) -> Option<Vec<u8>> {
    const ALPHA: &[u8; 128] = &{
        let mut t = [255u8; 128];
        let abc = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
        let mut i = 0;
        while i < abc.len() {
            t[abc[i] as usize] = i as u8;
            i += 1;
        }
        t
    };
    // `result` is a base-256 big-endian integer.
    let mut result: Vec<u8> = Vec::with_capacity(45);
    // Track leading '1' chars (each one is a 0x00 byte in the output).
    let mut leading_ones = 0usize;
    for &c in s.as_bytes() {
        if c == b'1' {
            leading_ones += 1;
            continue;
        }
        let v = ALPHA[c as usize];
        if v == 255 {
            return None;
        }
        // result = result * 58 + v
        let mut carry: u32 = v as u32;
        for byte in result.iter_mut() {
            let prod = (*byte as u32) * 58 + carry;
            *byte = (prod & 0xff) as u8;
            carry = prod >> 8;
        }
        while carry > 0 {
            result.push((carry & 0xff) as u8);
            carry >>= 8;
        }
    }
    // result is little-endian. Reverse to get big-endian, then prepend leading_ones 0x00s.
    result.reverse();
    let mut out = vec![0u8; leading_ones];
    out.extend(result);
    Some(out)
}
