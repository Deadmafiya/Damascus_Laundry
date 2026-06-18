//! Public decoder API. Re-exports the layouts we currently decode.
//!
//! Adding a new AMM family means adding a new module here, a new
//! variant in `AmmKind`, and dispatching `pub fn decode` on the
//! `AmmKind` tag.

pub mod meteora_dlmm;
pub mod orca_whirlpool;
pub mod raydium_amm_v4;

pub use meteora_dlmm::{
    decode_lb_pair, encode_lb_pair, DecodeError as MeteoraDecodeError, LbPair, DLMM_ACCOUNT_SIZE,
    METEORA_DLMM_PROGRAM_ID, SCALE_OFFSET,
};
pub use orca_whirlpool::{
    decode_whirlpool, encode_whirlpool, DecodeError as OrcaDecodeError, Whirlpool,
    ORCA_WHIRLPOOL_PROGRAM_ID, Q64_RESOLUTION, WHIRLPOOL_ACCOUNT_SIZE,
};
pub use raydium_amm_v4::{
    assemble_pool, decode_amm_info, decode_spl_token_account, AmmInfo, SplTokenAccount,
    AMM_INFO_SIZE, RAYDIUM_AMM_V4_PROGRAM_ID, SPL_TOKEN_ACCOUNT_SIZE,
};

use crate::pool::AmmKind;
use crate::Pubkey;

/// Discriminate which decoder to use by the **owner program ID**
/// of the account. Solana accounts are owned by their program;
/// the owner is the discriminator.
///
/// Returns `AmmKind` if the program is one we decode; `None` if
/// the program is unknown. Callers then dispatch to the
/// appropriate `decode_*` function.
pub fn identify_amm_by_program(program_id: &Pubkey) -> Option<AmmKind> {
    if *program_id == RAYDIUM_AMM_V4_PROGRAM_ID {
        Some(AmmKind::RaydiumAmmV4)
    } else if *program_id == ORCA_WHIRLPOOL_PROGRAM_ID {
        Some(AmmKind::OrcaWhirlpool)
    } else if *program_id == METEORA_DLMM_PROGRAM_ID {
        Some(AmmKind::MeteoraDlmm)
    } else {
        None
    }
}
