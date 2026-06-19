//! Bundle structure (1 tip + up to 4 swap legs).
//!
//! The bundle format mirrors what `jito-bundle::send_bundle` accepts:
//! a `Vec<VersionedTransaction>`. In 08-01 we use typed swap-leg
//! metadata (not real transactions) because the paper-mode executor
//! doesn't construct actual Solana transactions. 08-02 wires the
//! real `VersionedTransaction` types.

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

/// A complete bundle: 1 tip + 1..=4 swap legs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Bundle {
    pub tip: TipLeg,
    pub legs: Vec<SwapLeg>,
}

impl Bundle {
    /// Total transaction count (1 tip + swaps).
    pub fn tx_count(&self) -> usize {
        1 + self.legs.len()
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
}

impl BundleBuilder {
    pub fn new() -> Self {
        Self {
            legs: Vec::new(),
            tip: None,
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

    pub fn len(&self) -> usize {
        1 + self.legs.len() // 1 (tip) + swaps
    }

    pub fn is_empty(&self) -> bool {
        self.legs.is_empty()
    }

    /// Build the bundle. Errors if: 0 swaps, > 4 swaps, no tip set.
    pub fn build(&self) -> Result<Bundle, ExecutorError> {
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
        Ok(Bundle {
            tip,
            legs: self.legs.clone(),
        })
    }
}

impl Default for BundleBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn swap(label: &str) -> SwapLeg {
        SwapLeg::new(label, "SOL", "USDC", 1_000_000, 100_000_000)
    }

    #[test]
    fn build_valid_bundle() {
        let mut b = BundleBuilder::new();
        b.push_swap(swap("Raydium AMM v4 SOL/USDC"));
        b.push_swap(swap("Orca Whirlpool USDC/USDT"));
        b.push_swap(swap("Meteora DLMM USDT/SOL"));
        b.set_tip(TipLeg::new(
            10_000,
            "JitoTip1111111111111111111111111111111111",
        ));
        let bundle = b.build().unwrap();
        assert_eq!(bundle.tx_count(), 4);
        assert_eq!(bundle.legs.len(), 3);
        assert_eq!(bundle.total_tip_lamports(), 10_000);
    }

    #[test]
    fn bundle_with_4_swaps_is_max_allowed() {
        let mut b = BundleBuilder::new();
        for i in 0..4 {
            b.push_swap(swap(&format!("leg_{i}")));
        }
        b.set_tip(TipLeg::new(10_000, "Jito"));
        let bundle = b.build().unwrap();
        assert_eq!(bundle.tx_count(), 5);
    }

    #[test]
    fn bundle_with_5_swaps_rejected() {
        let mut b = BundleBuilder::new();
        for i in 0..5 {
            b.push_swap(swap(&format!("leg_{i}")));
        }
        b.set_tip(TipLeg::new(10_000, "Jito"));
        let err = b.build().unwrap_err();
        assert!(matches!(err, ExecutorError::BundleAssembly(_)));
    }

    #[test]
    fn bundle_with_no_swaps_rejected() {
        let mut b = BundleBuilder::new();
        b.set_tip(TipLeg::new(10_000, "Jito"));
        let err = b.build().unwrap_err();
        assert!(matches!(err, ExecutorError::BundleAssembly(_)));
    }

    #[test]
    fn bundle_with_no_tip_rejected() {
        let mut b = BundleBuilder::new();
        b.push_swap(swap("leg_0"));
        let err = b.build().unwrap_err();
        assert!(matches!(err, ExecutorError::BundleAssembly(_)));
    }
}
