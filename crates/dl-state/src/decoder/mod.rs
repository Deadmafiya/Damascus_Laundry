//! Public decoder API. Re-exports the layouts we currently decode.
//!
//! Adding a new AMM family means adding a new module here, a new
//! variant in `AmmKind`, and dispatching `pub fn decode` on the
//! `AmmKind` tag.

pub mod raydium_amm_v4;

pub use raydium_amm_v4::{
    assemble_pool, decode_amm_info, decode_spl_token_account, AmmInfo, SplTokenAccount,
    AMM_INFO_SIZE, RAYDIUM_AMM_V4_PROGRAM_ID, SPL_TOKEN_ACCOUNT_SIZE,
};
