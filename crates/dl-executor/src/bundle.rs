//! Bundle structure (1 tip + up to 4 swap legs).
//!
//! The bundle format mirrors what `jito-bundle::send_bundle` accepts:
//! a `Vec<VersionedTransaction>`. In 08-01 we use typed swap-leg
//! metadata (not real transactions) because the paper-mode executor
//! doesn't construct actual Solana transactions. 08-02 wires the
//! real `VersionedTransaction` types.
//!
//! v2.0 (Phase 1): the bundle now carries the **signed**
//! `VersionedTransaction`s that the Jito Block Engine will submit.
//! The bundle shape is fixed by the atomicity ADR:
//!
//!   tx0..tx2  = Jupiter swap legs (3, one per cycle leg)
//!   tx3       = dl-assert instruction (asserts net_pnl ≥ threshold)
//!   tx4       = Jito tip transfer
//!
//! That is exactly 5 txs (Jito's hard cap). 4-leg cycles are out
//! of scope until Phase 2.

use solana_sdk::pubkey::Pubkey;
use solana_sdk::transaction::VersionedTransaction;

use crate::error::ExecutorError;

/// One swap leg in a bundle.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SwapLeg {
    /// Human-readable label (e.g. "Raydium AMM v4 SOL/USDC").
    pub label: String,
    /// Input mint (base58).
    pub input_mint: String,
    /// Output mint.
    pub output_mint: String,
    /// Expected input amount in input-token base units.
    pub expected_in: u64,
    /// Expected output amount in output-token base units.
    pub expected_out: u64,
}

impl SwapLeg {
    pub fn new(
        label: impl Into<String>,
        input_mint: impl Into<String>,
        output_mint: impl Into<String>,
        expected_in: u64,
        expected_out: u64,
    ) -> Self {
        Self {
            label: label.into(),
            input_mint: input_mint.into(),
            output_mint: output_mint.into(),
            expected_in,
            expected_out,
        }
    }
}

/// The tip transaction in a Jito bundle.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TipLeg {
    /// Lamports to tip.
    pub tip_lamports: u64,
    /// Tip receiver pubkey (Jito's tip account, or a validator).
    pub tip_account: String,
}

impl TipLeg {
    pub fn new(tip_lamports: u64, tip_account: impl Into<String>) -> Self {
        Self {
            tip_lamports,
            tip_account: tip_account.into(),
        }
    }
}

/// A complete bundle: 1 tip + 1..=4 swap legs + signed
/// `VersionedTransaction`s for each.
///
/// The signed transactions are the wire-format bytes that Jito
/// accepts. The builder enforces the 5-tx cap. In v2.0 the cap
/// is always 5: 3 swap legs + 1 assert + 1 tip.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Bundle {
    pub tip: TipLeg,
    pub legs: Vec<SwapLeg>,
    /// Signed transactions in submission order. Index 0 = first tx
    /// the validator executes; final index = tip transfer. Length
    /// is `legs.len() + 2` (one assert tx + one tip tx on top of
    /// the swap legs).
    pub signed_transactions: Vec<VersionedTransaction>,
}

impl Bundle {
    /// Total transaction count (1 tip + assert + swaps).
    pub fn tx_count(&self) -> usize {
        self.signed_transactions.len()
    }

    /// Total tip lamports (just the tip leg; the 5% Jito fee is
    /// implicit and tracked separately in the user's cost model).
    pub fn total_tip_lamports(&self) -> u64 {
        self.tip.tip_lamports
    }
}

/// Builder for a Bundle. Enforces the 5-tx Jito cap.
pub struct BundleBuilder {
    legs: Vec<SwapLeg>,
    tip: Option<TipLeg>,
    signed_transactions: Vec<VersionedTransaction>,
}

impl BundleBuilder {
    pub fn new() -> Self {
        Self {
            legs: Vec::new(),
            tip: None,
            signed_transactions: Vec::new(),
        }
    }

    pub fn push_swap(&mut self, leg: SwapLeg) -> &mut Self {
        self.legs.push(leg);
        self
    }

    pub fn set_tip(&mut self, tip: TipLeg) -> &mut Self {
        self.tip = Some(tip);
        self
    }

    /// Replace the entire signed-transactions list. The list must
    /// have exactly `legs.len() + 2` entries (3 swap legs + 1
    /// assert + 1 tip = 5 total for the v2.0 atomicity model).
    pub fn set_signed_transactions(
        &mut self,
        txs: Vec<VersionedTransaction>,
    ) -> &mut Self {
        self.signed_transactions = txs;
        self
    }

    /// Append a single signed transaction (used to add the assert
    /// tx after the swap legs, and the tip tx last).
    pub fn push_signed_transaction(&mut self, tx: VersionedTransaction) -> &mut Self {
        self.signed_transactions.push(tx);
        self
    }

    pub fn len(&self) -> usize {
        self.signed_transactions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.signed_transactions.is_empty()
    }

    /// Build the bundle. Errors if:
    /// - 0 swaps,
    /// - > 4 swaps (Jito's hard cap),
    /// - no tip set,
    /// - signed_transactions is non-empty AND its length doesn't
    ///   match legs+2 (the live-mode invariant: 1 assert + 1 tip
    ///   on top of the swap legs). In paper mode, signed_transactions
    ///   is empty and the check is skipped.
    /// - `assert_program_id` is `Some` AND signed_transactions[legs.len()]
    ///   does not call that program (defense-in-depth: ensure the
    ///   assert tx slot is the right program).
    pub fn build(&self, assert_program_id: Option<&Pubkey>) -> Result<Bundle, ExecutorError> {
        if self.legs.is_empty() {
            return Err(ExecutorError::BundleAssembly(
                "bundle has 0 swap legs (need >= 1)".into(),
            ));
        }
        if self.legs.len() > 4 {
            return Err(ExecutorError::BundleAssembly(format!(
                "bundle has {} swap legs (Jito allows max 4 + 1 tip = 5 total)",
                self.legs.len()
            )));
        }
        let tip = self
            .tip
            .clone()
            .ok_or_else(|| ExecutorError::BundleAssembly("bundle has no tip leg".into()))?;
        let expected_txs = self.legs.len() + 2; // legs + assert + tip
        if !self.signed_transactions.is_empty() && self.signed_transactions.len() != expected_txs
        {
            return Err(ExecutorError::BundleAssembly(format!(
                "signed_transactions len {} != legs+2 = {}",
                self.signed_transactions.len(),
                expected_txs
            )));
        }
        // Defense-in-depth: assert tx identity check.
        if let Some(assert_pid) = assert_program_id {
            if !self.signed_transactions.is_empty() {
                let assert_tx = &self.signed_transactions[self.legs.len()];
                let first_pid = assert_tx_first_program_id(assert_tx);
                if first_pid != Some(*assert_pid) {
                    return Err(ExecutorError::BundleAssembly(format!(
                        "signed_transactions[{}] (the assert slot) calls {:?}, expected {:?}",
                        self.legs.len(),
                        first_pid,
                        assert_pid
                    )));
                }
            }
        }
        Ok(Bundle {
            tip,
            legs: self.legs.clone(),
            signed_transactions: self.signed_transactions.clone(),
        })
    }
}

/// Return the program ID of the first instruction in `tx`, or
/// `None` if the tx has no instructions.
fn assert_tx_first_program_id(tx: &solana_sdk::transaction::VersionedTransaction) -> Option<Pubkey> {
    use solana_sdk::instruction::CompiledInstruction;
    use solana_sdk::message::VersionedMessage;
    let first_ix: Option<&CompiledInstruction> = match &tx.message {
        VersionedMessage::Legacy(m) => m.instructions.first(),
        VersionedMessage::V0(m) => m.instructions.first(),
    };
    let first_ix = first_ix?;
    let acct_keys = match &tx.message {
        VersionedMessage::Legacy(m) => &m.account_keys,
        VersionedMessage::V0(m) => &m.account_keys,
    };
    acct_keys.get(first_ix.program_id_index as usize).copied()
}

/// Build a bundle directly from pre-built signed transactions,
/// skipping the leg-by-leg builder. Useful in tests and in the
/// hot path when transactions are constructed externally.
pub fn build_bundle_from_signed(
    legs: Vec<SwapLeg>,
    tip: TipLeg,
    signed_transactions: Vec<VersionedTransaction>,
) -> Result<Bundle, ExecutorError> {
    let mut b = BundleBuilder::new();
    for leg in legs {
        b.push_swap(leg);
    }
    b.set_tip(tip);
    b.set_signed_transactions(signed_transactions);
    b.build(None)
}

impl Default for BundleBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::message::{Message, VersionedMessage};
    use solana_sdk::pubkey::Pubkey;
    use solana_sdk::signer::keypair::Keypair;
    use solana_sdk::signer::Signer;
    use solana_sdk::system_instruction;

    /// Build a placeholder signed VersionedTransaction. Not
    /// intended for submission; only valid enough to satisfy
    /// `Bundle::build`'s signed-transactions length check.
    fn dummy_tx() -> VersionedTransaction {
        let kp = Keypair::new();
        let ix = system_instruction::transfer(&kp.pubkey(), &Pubkey::new_unique(), 0);
        let msg = Message::new(&[ix], Some(&kp.pubkey()));
        VersionedTransaction::try_new(VersionedMessage::Legacy(msg), &[&kp]).unwrap()
    }

    fn dummy_txs(n: usize) -> Vec<VersionedTransaction> {
        (0..n).map(|_| dummy_tx()).collect()
    }

    fn swap(label: &str) -> SwapLeg {
        SwapLeg::new(label, "SOL", "USDC", 1_000_000, 100_000_000)
    }

    fn build_3_leg_bundle(txs: Vec<VersionedTransaction>) -> Bundle {
        let mut b = BundleBuilder::new();
        b.push_swap(swap("Raydium AMM v4 SOL/USDC"));
        b.push_swap(swap("Orca Whirlpool USDC/USDT"));
        b.push_swap(swap("Meteora DLMM USDT/SOL"));
        b.set_tip(TipLeg::new(
            10_000,
            "JitoTip1111111111111111111111111111111111",
        ));
        b.set_signed_transactions(txs);
        b.build(None).unwrap()
    }

    #[test]
    fn build_valid_bundle() {
        let bundle = build_3_leg_bundle(dummy_txs(5));
        assert_eq!(bundle.tx_count(), 5);
        assert_eq!(bundle.legs.len(), 3);
        assert_eq!(bundle.total_tip_lamports(), 10_000);
        assert_eq!(bundle.signed_transactions.len(), 5);
    }

    #[test]
    fn bundle_with_4_swaps_is_max_allowed() {
        let mut b = BundleBuilder::new();
        for i in 0..4 {
            b.push_swap(swap(&format!("leg_{i}")));
        }
        b.set_tip(TipLeg::new(10_000, "Jito"));
        b.set_signed_transactions(dummy_txs(6)); // 4 swaps + assert + tip
        let bundle = b.build(None).unwrap();
        assert_eq!(bundle.tx_count(), 6);
    }

    #[test]
    fn bundle_with_5_swaps_rejected() {
        let mut b = BundleBuilder::new();
        for i in 0..5 {
            b.push_swap(swap(&format!("leg_{i}")));
        }
        b.set_tip(TipLeg::new(10_000, "Jito"));
        let err = b.build(None).unwrap_err();
        assert!(matches!(err, ExecutorError::BundleAssembly(_)));
    }

    #[test]
    fn bundle_with_no_swaps_rejected() {
        let mut b = BundleBuilder::new();
        b.set_tip(TipLeg::new(10_000, "Jito"));
        let err = b.build(None).unwrap_err();
        assert!(matches!(err, ExecutorError::BundleAssembly(_)));
    }

    #[test]
    fn bundle_with_no_tip_rejected() {
        let mut b = BundleBuilder::new();
        b.push_swap(swap("leg_0"));
        b.set_signed_transactions(dummy_txs(3)); // 1 swap + assert + tip
        let err = b.build(None).unwrap_err();
        assert!(matches!(err, ExecutorError::BundleAssembly(_)));
    }

    #[test]
    fn bundle_rejects_mismatched_signed_tx_count() {
        // 3 swap legs ⇒ need 5 signed txs (3 + assert + tip).
        let mut b = BundleBuilder::new();
        b.push_swap(swap("leg_0"));
        b.push_swap(swap("leg_1"));
        b.push_swap(swap("leg_2"));
        b.set_tip(TipLeg::new(10_000, "Jito"));
        b.set_signed_transactions(dummy_txs(3)); // too few
        let err = b.build(None).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("signed_transactions len 3 != legs+2 = 5"));
    }

    #[test]
    fn bundle_rejects_too_many_signed_tx() {
        let mut b = BundleBuilder::new();
        b.push_swap(swap("leg_0"));
        b.push_swap(swap("leg_1"));
        b.push_swap(swap("leg_2"));
        b.set_tip(TipLeg::new(10_000, "Jito"));
        b.set_signed_transactions(dummy_txs(6)); // 1 too many
        let err = b.build(None).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("signed_transactions len 6 != legs+2 = 5"));
    }

    #[test]
    fn bundle_rejects_wrong_assert_program_id() {
        use solana_sdk::instruction::Instruction;
        use solana_sdk::message::Message;
        use solana_sdk::signer::Signer;
        let kp = Keypair::new();
        // 3 swaps + 1 assert (with the WRONG program) + 1 tip.
        let mut b = BundleBuilder::new();
        b.push_swap(swap("leg_0"));
        b.push_swap(swap("leg_1"));
        b.push_swap(swap("leg_2"));
        b.set_tip(TipLeg::new(10_000, "Jito"));
        let wrong_assert_program = Pubkey::new_unique();
        let right_assert_program = Pubkey::new_unique();
        // Build 5 txs: 3 dummy swaps + 1 bad-assert tx + 1 tip.
        let mut txs = vec![dummy_tx(), dummy_tx(), dummy_tx()];
        let bad_ix = Instruction {
            program_id: wrong_assert_program,
            accounts: vec![],
            data: vec![0],
        };
        let mut msg = Message::new(&[bad_ix], Some(&kp.pubkey()));
        msg.recent_blockhash = solana_sdk::hash::Hash::new_unique();
        use solana_sdk::message::VersionedMessage;
        let bad_tx = VersionedTransaction {
            signatures: vec![solana_sdk::signature::Signature::default()],
            message: VersionedMessage::Legacy(msg),
        };
        txs.push(bad_tx); // index 3 = the assert slot
        txs.push(dummy_tx()); // index 4 = the tip slot
        b.set_signed_transactions(txs);
        let err = b.build(Some(&right_assert_program)).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("assert slot"), "got: {msg}");
    }

    #[test]
    fn build_bundle_from_signed_helper() {
        let legs = vec![swap("leg_0"), swap("leg_1"), swap("leg_2")];
        let tip = TipLeg::new(10_000, "Jito");
        let txs = dummy_txs(5);
        let bundle = build_bundle_from_signed(legs, tip, txs).unwrap();
        assert_eq!(bundle.tx_count(), 5);
    }
}