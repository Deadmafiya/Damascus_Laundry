//! Property tests: the decoder must be **total** on its input domain.
//! That is, for any well-formed or garbage input, it returns either
//! `Ok(_)` or a specific `DecodeError` variant — it never panics, never
//! returns nonsense. This is the bedrock of the "garbage in, structured
//! error out" contract the engine depends on.
//!
//! Run with `cargo test -p dl-state --test decoder_property` (proptest
//! is already in the workspace).

use dl_state::decoder::{
    decode_amm_info, decode_spl_token_account, AMM_INFO_SIZE, SPL_TOKEN_ACCOUNT_SIZE,
};
use proptest::prelude::*;

proptest! {
    /// Any input either decodes or returns a structured error — never
    /// panics. Shorter inputs always error; longer inputs are accepted
    /// (extra bytes ignored).
    #[test]
    fn amm_info_is_total(bytes in proptest::collection::vec(any::<u8>(), 0..2000)) {
        let result = decode_amm_info(&bytes);
        if bytes.len() < AMM_INFO_SIZE {
            // TooShort
            prop_assert!(result.is_err());
        } else if let Ok(info) = result {
            // Either Ok (status in 1..=7) or BadDiscriminator. Either is
            // a valid response to arbitrary input.
            prop_assert!((1..=7).contains(&info.status));
        }
    }

    /// Any input either decodes or returns `TooShort`. Never panics.
    #[test]
    fn spl_token_account_is_total(bytes in proptest::collection::vec(any::<u8>(), 0..500)) {
        let result = decode_spl_token_account(&bytes);
        if bytes.len() < SPL_TOKEN_ACCOUNT_SIZE {
            prop_assert!(result.is_err());
        } else {
            prop_assert!(result.is_ok());
        }
    }

    /// A status byte of 0 always produces an error (the "uninitialized"
    /// sentinel — Raydium's AmmStatus::Uninitialized).
    #[test]
    fn zero_status_is_always_rejected(rest in proptest::collection::vec(any::<u8>(), AMM_INFO_SIZE - 8)) {
        let mut bytes = vec![0u8; AMM_INFO_SIZE];
        bytes.extend_from_slice(&rest);
        prop_assert!(decode_amm_info(&bytes).is_err());
    }

    /// A status byte in 1..=7 with arbitrary tail decodes successfully.
    #[test]
    fn valid_status_with_random_tail_decodes(
        status in 1u64..=7,
        tail in proptest::collection::vec(any::<u8>(), AMM_INFO_SIZE - 8)
    ) {
        let mut bytes = Vec::with_capacity(AMM_INFO_SIZE);
        bytes.extend_from_slice(&status.to_le_bytes());
        bytes.extend_from_slice(&tail);
        let info = decode_amm_info(&bytes).unwrap();
        prop_assert_eq!(info.status, status);
    }
}
