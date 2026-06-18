//! Pessimistic-by-default simulation core (Phase 5, plan 05-01).
//!
//! Takes a Phase-4 [`crate::net_profit::NetProfit`] for a detected cycle and
//! produces an expected-value estimate via the multiplicative decomposition
//! from the simulation research:
//!
//! ```text
//! E[PnL] = p_detect * p_win * p_land * (gross - costs) - E[failed_costs]
//! ```
//!
//! Every probability is fixed-point, none is 1.0 by default, and `p_win`
//! *decreases* as the opportunity gets richer (winner's curse). Every
//! opportunity is reported with both an optimistic and a conservative bound;
//! callers act only on the conservative one.
//!
//! ## Integer-only invariant
//!
//! This module is value-path. No fractional numeric types anywhere. Note
//! even the doc comments avoid the banned substrings the CI guard scans for
//! (`fixed_point_no_fractional.rs`).
//!
//! ## Probability scale: reuse `dl-core::prob` (1e18), not a parallel ppm type
//!
//! `dl-core::prob` already defines the engine's integer probability scale
//! (`PROB_SCALE_1E18`, `mul_prob`, `bps_to_prob`) and is documented as the
//! Phase-5 probability primitive. To keep one coherent scale across the
//! engine, [`Prob`] wraps a `u128` on that 1e18 scale. Constructors still
//! accept parts-per-million ([`Prob::from_ppm`]) so call sites read in
//! ppm, but the stored value and all combination use the shared 1e18 scale.
//!
//! ## Calibration is deferred to Phase 6
//!
//! The default constants in [`CompetitionParams`], [`LandingParams`], and
//! [`FailedCostModel`] are *structurally* correct and conservatively tuned,
//! but their exact values are placeholders. Phase 6 calibrates them against
//! on-chain ground truth (competitor-landed arbs, macro anchors).

use dl_core::prob::{mul_prob, PROB_SCALE_1E18};

use crate::error::SimError;
use crate::net_profit::NetProfit;

/// Parts-per-million: `1_000_000` ppm == 1.0.
pub const PPM_ONE: u32 = 1_000_000;

/// Nominal Jito auction window in milliseconds (the relayer "speed bump").
/// Anchored from the simulation research / Jito docs. Used by [`p_land`].
pub const JITO_AUCTION_MS: u32 = 200;

/// Nominal Jito parallel-auction tick cadence in milliseconds.
pub const JITO_TICK_MS: u32 = 50;

// ---------------------------------------------------------------------------
// Prob: fixed-point probability on the shared dl-core 1e18 scale
// ---------------------------------------------------------------------------

/// A probability in `[0, 1]`, stored as a `u128` on the `dl-core`
/// `PROB_SCALE_1E18` scale (so `PROB_SCALE_1E18` == 1.0).
///
/// Construct from parts-per-million via [`Prob::from_ppm`] for readable call
/// sites; combine independent probabilities with [`Prob::combine`]; haircut a
/// signed value with [`Prob::apply_to`].
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct Prob(u128);

impl Prob {
    /// Certainty (1.0).
    pub const ONE: Prob = Prob(PROB_SCALE_1E18);
    /// Impossibility (0.0).
    pub const ZERO: Prob = Prob(0);

    /// Build from parts-per-million (`0..=1_000_000`). Rejects out-of-range.
    pub fn from_ppm(ppm: u32) -> Result<Prob, SimError> {
        if ppm > PPM_ONE {
            return Err(SimError::ProbOutOfRange(ppm));
        }
        // ppm / 1e6 == scaled / 1e18  =>  scaled = ppm * 1e12.
        Ok(Prob((ppm as u128) * (PROB_SCALE_1E18 / PPM_ONE as u128)))
    }

    /// Build directly from a raw 1e18-scale value, clamping to `[0, 1]`.
    pub fn from_scaled_clamped(scaled: u128) -> Prob {
        Prob(scaled.min(PROB_SCALE_1E18))
    }

    /// The raw 1e18-scale value.
    #[inline]
    pub fn scaled(self) -> u128 {
        self.0
    }

    /// Approximate parts-per-million (floored). For logging/inspection only.
    #[inline]
    pub fn to_ppm(self) -> u32 {
        (self.0 / (PROB_SCALE_1E18 / PPM_ONE as u128)) as u32
    }

    /// Combine two independent probabilities: `self * other`. Result is
    /// always `<= min(self, other)` and `<= 1.0`.
    #[inline]
    pub fn combine(self, other: Prob) -> Prob {
        Prob(mul_prob(self.0, other.0))
    }

    /// Haircut a signed value by this probability: `floor(value * p)`.
    ///
    /// Sign-symmetric: applied to the magnitude, then the sign is
    /// reattached, so flooring is toward zero for both positive and negative
    /// values (a loss is never made *smaller* in magnitude by less than the
    /// probability would imply, and a gain is never rounded up).
    pub fn apply_to(self, value: i128) -> i128 {
        if value == 0 || self.0 == 0 {
            return 0;
        }
        let mag = value.unsigned_abs();
        let scaled = mul_prob(mag, self.0);
        if value < 0 {
            -(scaled as i128)
        } else {
            scaled as i128
        }
    }
}

// ---------------------------------------------------------------------------
// p_win: winner's-curse model (decreases with opportunity richness)
// ---------------------------------------------------------------------------

/// Parameters for the winner's-curse [`p_win`] model.
///
/// Below `richness_threshold_bps`, win probability is `base_win_ppm`. Above
/// it, each additional basis point of richness reduces win probability by
/// `decay_ppm_per_bps` (saturating at 0). This encodes adverse selection:
/// fatter spreads attract more and faster competitors, so the *richer* an
/// opportunity looks, the *less* likely we win it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CompetitionParams {
    /// Base win probability (ppm) for opportunities at or below the threshold.
    pub base_win_ppm: u32,
    /// Richness (in net bps) above which competition intensifies.
    pub richness_threshold_bps: i32,
    /// Win-probability decay (ppm) per basis point of richness above threshold.
    pub decay_ppm_per_bps: u32,
}

impl CompetitionParams {
    /// Conservative placeholder (Phase-6 calibration target).
    ///
    /// 30% base win rate, decay starting at 10 bps, losing 1% (10_000 ppm)
    /// of win probability per extra bp. These are deliberately pessimistic.
    pub fn conservative_default() -> Self {
        Self {
            base_win_ppm: 300_000,
            richness_threshold_bps: 10,
            decay_ppm_per_bps: 10_000,
        }
    }

    /// Optimistic bound: always win (`p_win == 1.0`, no decay).
    pub fn optimistic() -> Self {
        Self {
            base_win_ppm: PPM_ONE,
            richness_threshold_bps: i32::MAX,
            decay_ppm_per_bps: 0,
        }
    }
}

/// Win probability for an opportunity of richness `net_profit_bps`.
///
/// Non-increasing in `net_profit_bps`: richer opportunities have lower (or
/// equal) win probability. Clamped to `[0, 1]`.
pub fn p_win(net_profit_bps: i32, params: &CompetitionParams) -> Prob {
    // Below threshold: flat base.
    if net_profit_bps <= params.richness_threshold_bps {
        // base_win_ppm is constructed to be <= PPM_ONE in all our params;
        // clamp defensively in case a caller hand-builds an out-of-range value.
        return Prob::from_ppm(params.base_win_ppm.min(PPM_ONE)).unwrap_or(Prob::ONE);
    }
    // Above threshold: linear decay, saturating at 0.
    let excess_bps = (net_profit_bps - params.richness_threshold_bps) as u64;
    let decay = excess_bps.saturating_mul(params.decay_ppm_per_bps as u64);
    let win_ppm = (params.base_win_ppm as u64).saturating_sub(decay) as u32;
    Prob::from_ppm(win_ppm.min(PPM_ONE)).unwrap_or(Prob::ZERO)
}

// ---------------------------------------------------------------------------
// p_land: latency model (decreases with total latency)
// ---------------------------------------------------------------------------

/// A latency budget from detection to landing, in milliseconds. Total drives
/// [`p_land`]. Component breakdown mirrors the research decomposition
/// `t_detect + t_decide + t_build + t_network + t_auction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LatencyBudget {
    pub t_detect_ms: u32,
    pub t_decide_ms: u32,
    pub t_build_ms: u32,
    pub t_network_ms: u32,
    pub t_auction_ms: u32,
}

impl LatencyBudget {
    /// Total latency in milliseconds (saturating).
    pub fn total_ms(&self) -> u32 {
        self.t_detect_ms
            .saturating_add(self.t_decide_ms)
            .saturating_add(self.t_build_ms)
            .saturating_add(self.t_network_ms)
            .saturating_add(self.t_auction_ms)
    }

    /// A representative conservative budget: small detect/decide/build/network
    /// plus the full Jito auction window.
    pub fn conservative_default() -> Self {
        Self {
            t_detect_ms: 10,
            t_decide_ms: 5,
            t_build_ms: 5,
            t_network_ms: 30,
            t_auction_ms: JITO_AUCTION_MS,
        }
    }

    /// Optimistic bound: zero latency (you act instantly).
    pub fn optimistic() -> Self {
        Self {
            t_detect_ms: 0,
            t_decide_ms: 0,
            t_build_ms: 0,
            t_network_ms: 0,
            t_auction_ms: 0,
        }
    }
}

/// Parameters for the [`p_land`] latency-to-landing model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LandingParams {
    /// At or under this total latency, landing is certain (`p_land == 1.0`).
    pub grace_ms: u32,
    /// Landing-probability decay (ppm) per millisecond of latency above grace.
    pub decay_ppm_per_ms: u32,
}

impl LandingParams {
    /// Conservative placeholder (Phase-6 calibration target).
    ///
    /// Grace of one Jito tick; beyond that, lose ~0.2% (2_000 ppm) of landing
    /// probability per additional millisecond. With the conservative latency
    /// budget (~250 ms), this lands well under 1.0 — as it should.
    pub fn conservative_default() -> Self {
        Self {
            grace_ms: JITO_TICK_MS,
            decay_ppm_per_ms: 2_000,
        }
    }

    /// Optimistic bound: infinite grace, no decay (always lands).
    pub fn optimistic() -> Self {
        Self {
            grace_ms: u32::MAX,
            decay_ppm_per_ms: 0,
        }
    }
}

/// Landing probability for a given latency budget.
///
/// Non-increasing in total latency: more latency means lower (or equal)
/// landing probability. Clamped to `[0, 1]`. The real "advance replayed
/// state to the projected landing slot and re-score" model is Phase 6 (it
/// needs pool-bearing captures); this haircut is the v1.0 stand-in.
pub fn p_land(budget: &LatencyBudget, params: &LandingParams) -> Prob {
    let total = budget.total_ms();
    if total <= params.grace_ms {
        return Prob::ONE;
    }
    let over_ms = (total - params.grace_ms) as u64;
    let decay = over_ms.saturating_mul(params.decay_ppm_per_ms as u64);
    let land_ppm = (PPM_ONE as u64).saturating_sub(decay) as u32;
    Prob::from_ppm(land_ppm.min(PPM_ONE)).unwrap_or(Prob::ZERO)
}

// ---------------------------------------------------------------------------
// Failed-cost model: the cost of losing
// ---------------------------------------------------------------------------

/// Which submission path the strategy uses. Determines failed-attempt cost
/// accounting (research principle 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SubmitPath {
    /// Priority-fee / "spam" path: every *included* tx (including reverts)
    /// pays the base signature fee, so failed attempts cost real lamports.
    Spam,
    /// Jito bundle path: failed bundles do NOT land, so no tip is paid on a
    /// loss. Failed-attempt cost is ~0 (the won bundle's tip is already in
    /// the Phase-4 cost stack).
    JitoBundle,
}

/// Models the expected cost of the failed attempts that accompany each win.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FailedCostModel {
    /// Expected number of failed attempts per successful win.
    pub attempts_per_win: u32,
    /// Lamports lost per failed attempt (spam path: base sig fee, ~5_000).
    pub per_attempt_lamports: u64,
    /// Submission path (governs whether failed attempts cost anything).
    pub path: SubmitPath,
}

impl FailedCostModel {
    /// Conservative placeholder for the spam path (Phase-6 calibration target):
    /// ~24 failed attempts per win (matching the ~96%-fail macro anchor) at the
    /// base signature fee each.
    pub fn conservative_spam() -> Self {
        Self {
            attempts_per_win: 24,
            per_attempt_lamports: 5_000,
            path: SubmitPath::Spam,
        }
    }

    /// Conservative Jito-bundle default: failed bundles don't land, so the
    /// expected failed cost is zero.
    pub fn jito_bundle() -> Self {
        Self {
            attempts_per_win: 0,
            per_attempt_lamports: 0,
            path: SubmitPath::JitoBundle,
        }
    }

    /// Optimistic bound: no failed-attempt cost.
    pub fn optimistic() -> Self {
        Self {
            attempts_per_win: 0,
            per_attempt_lamports: 0,
            path: SubmitPath::JitoBundle,
        }
    }

    /// Expected failed-attempt cost in lamports.
    pub fn expected_failed_cost(&self) -> u128 {
        match self.path {
            // Jito: failed bundles never land -> no cost on losses.
            SubmitPath::JitoBundle => 0,
            // Spam: every included attempt (incl. reverts) pays the sig fee.
            SubmitPath::Spam => {
                (self.attempts_per_win as u128) * (self.per_attempt_lamports as u128)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// EV evaluation + dual bounds
// ---------------------------------------------------------------------------

/// One expected-value estimate (under one assumption set).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExpectedValue {
    /// Expected PnL in input-token base units (signed). The decision metric.
    pub e_pnl: i128,
    /// The detection probability used.
    pub p_detect: Prob,
    /// The win probability used (post winner's-curse).
    pub p_win: Prob,
    /// The landing probability used.
    pub p_land: Prob,
    /// The expected failed-attempt cost subtracted (lamports).
    pub expected_failed_cost: u128,
}

/// One full assumption set for an EV evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EvalParams {
    /// Probability we detect the opportunity in time.
    pub p_detect: Prob,
    /// Winner's-curse competition model.
    pub competition: CompetitionParams,
    /// Latency budget feeding `p_land`.
    pub latency: LatencyBudget,
    /// Landing model parameters.
    pub landing: LandingParams,
    /// Failed-attempt cost model.
    pub failed: FailedCostModel,
}

impl EvalParams {
    /// Optimistic bound: detect instantly, always win, always land, no failed
    /// cost. This is the naive backtest — useful only as a ceiling.
    pub fn optimistic() -> Self {
        Self {
            p_detect: Prob::ONE,
            competition: CompetitionParams::optimistic(),
            latency: LatencyBudget::optimistic(),
            landing: LandingParams::optimistic(),
            failed: FailedCostModel::optimistic(),
        }
    }

    /// Conservative default: the full pessimistic haircut stack. Spam path is
    /// the default (it has nonzero failed cost — the more honest baseline).
    pub fn conservative_default() -> Self {
        Self {
            // ~70% chance we even see it in time (Phase-6 calibration target).
            p_detect: Prob::from_ppm(700_000).unwrap_or(Prob::ONE),
            competition: CompetitionParams::conservative_default(),
            latency: LatencyBudget::conservative_default(),
            landing: LandingParams::conservative_default(),
            failed: FailedCostModel::conservative_spam(),
        }
    }
}

/// Both bounds for one opportunity. Callers act only on `conservative`; the
/// gap between the two is itself a risk signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EvalOutcome {
    /// Optimistic ceiling (you detect, win, land at no failed cost).
    pub optimistic: ExpectedValue,
    /// Conservative, pessimistic-by-default estimate. The one to act on.
    pub conservative: ExpectedValue,
}

/// Evaluate one assumption set against a Phase-4 [`NetProfit`].
fn evaluate_one(net: &NetProfit, params: &EvalParams) -> ExpectedValue {
    let pw = p_win(net.net_profit_bps, &params.competition);
    let pl = p_land(&params.latency, &params.landing);
    let haircut = params.p_detect.combine(pw).combine(pl);

    // `net.net_profit` is already gross minus all Phase-4 costs (signed).
    // Haircut by the combined probability, then subtract expected failed cost.
    // A losing cycle (net < 0) stays non-positive: apply_to keeps the sign,
    // and subtracting a non-negative failed cost only lowers it further.
    let haircut_pnl = haircut.apply_to(net.net_profit);
    let failed = params.failed.expected_failed_cost();
    let e_pnl = haircut_pnl - (failed as i128);

    ExpectedValue {
        e_pnl,
        p_detect: params.p_detect,
        p_win: pw,
        p_land: pl,
        expected_failed_cost: failed,
    }
}

/// Evaluate an opportunity under both an optimistic and a conservative
/// assumption set, returning both bounds.
///
/// Guarantees (verified by tests): `conservative.e_pnl <= optimistic.e_pnl`,
/// and for a profitable `NetProfit` with non-degenerate conservative
/// probabilities, `conservative.e_pnl < net.net_profit`. A non-profitable
/// `NetProfit` (or one whose sizer returned `NoTrade`) never yields a
/// positive EV.
pub fn evaluate(
    net: &NetProfit,
    optimistic: &EvalParams,
    conservative: &EvalParams,
) -> EvalOutcome {
    EvalOutcome {
        optimistic: evaluate_one(net, optimistic),
        conservative: evaluate_one(net, conservative),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cost::CostBreakdown;

    // Build a NetProfit directly for unit testing without running the sizer.
    fn net_with(net_profit: i128, bps: i32, input: u128) -> NetProfit {
        let zero_costs = CostBreakdown {
            base_sig_fee_lamports: 0,
            priority_fee_lamports: 0,
            jito_tip_lamports: 0,
            jito_tip_fee_lamports: 0,
            total_lamports: 0,
        };
        NetProfit {
            input_amount: input,
            gross_output: 0,
            total_costs: zero_costs,
            net_profit,
            net_profit_bps: bps,
            profitable: net_profit > 0,
        }
    }

    #[test]
    fn prob_from_ppm_bounds() {
        assert_eq!(Prob::from_ppm(0).unwrap(), Prob::ZERO);
        assert_eq!(Prob::from_ppm(PPM_ONE).unwrap(), Prob::ONE);
        assert!(Prob::from_ppm(PPM_ONE + 1).is_err());
    }

    #[test]
    fn prob_combine_le_inputs() {
        let half = Prob::from_ppm(500_000).unwrap();
        let combined = half.combine(half);
        assert!(combined <= half);
        // 0.5 * 0.5 == 0.25 == 250_000 ppm.
        assert_eq!(combined.to_ppm(), 250_000);
        assert_eq!(Prob::ONE.combine(half), half);
        assert_eq!(Prob::ZERO.combine(half), Prob::ZERO);
    }

    #[test]
    fn prob_apply_to_signs() {
        let half = Prob::from_ppm(500_000).unwrap();
        assert_eq!(half.apply_to(1000), 500);
        assert_eq!(half.apply_to(-1000), -500);
        assert_eq!(half.apply_to(0), 0);
        assert_eq!(Prob::ZERO.apply_to(1000), 0);
        assert_eq!(Prob::ONE.apply_to(-1234), -1234);
    }

    #[test]
    fn p_win_below_threshold_is_base() {
        let p = CompetitionParams::conservative_default();
        let at = p_win(p.richness_threshold_bps, &p);
        let below = p_win(p.richness_threshold_bps - 100, &p);
        assert_eq!(at, below);
        assert_eq!(at.to_ppm(), p.base_win_ppm);
    }

    #[test]
    fn p_win_decreases_with_richness() {
        let p = CompetitionParams::conservative_default();
        let lean = p_win(p.richness_threshold_bps + 1, &p);
        let rich = p_win(p.richness_threshold_bps + 100, &p);
        assert!(rich <= lean, "richer must not have higher win prob");
        assert!(rich < lean, "strictly lower past the threshold");
    }

    #[test]
    fn p_win_saturates_at_zero() {
        let p = CompetitionParams::conservative_default();
        // Huge richness drives decay past base -> 0.
        let p0 = p_win(1_000_000, &p);
        assert_eq!(p0, Prob::ZERO);
    }

    #[test]
    fn p_land_under_grace_is_one() {
        let lp = LandingParams::conservative_default();
        let b = LatencyBudget {
            t_detect_ms: 0,
            t_decide_ms: 0,
            t_build_ms: 0,
            t_network_ms: 0,
            t_auction_ms: lp.grace_ms,
        };
        assert_eq!(p_land(&b, &lp), Prob::ONE);
    }

    #[test]
    fn p_land_decreases_with_latency() {
        let lp = LandingParams::conservative_default();
        let lo = LatencyBudget {
            t_detect_ms: 0,
            t_decide_ms: 0,
            t_build_ms: 0,
            t_network_ms: 0,
            t_auction_ms: lp.grace_ms + 10,
        };
        let hi = LatencyBudget {
            t_detect_ms: 0,
            t_decide_ms: 0,
            t_build_ms: 0,
            t_network_ms: 0,
            t_auction_ms: lp.grace_ms + 200,
        };
        assert!(p_land(&hi, &lp) < p_land(&lo, &lp));
    }

    #[test]
    fn failed_cost_path_asymmetry() {
        assert_eq!(FailedCostModel::jito_bundle().expected_failed_cost(), 0);
        let spam = FailedCostModel::conservative_spam();
        assert_eq!(
            spam.expected_failed_cost(),
            (spam.attempts_per_win as u128) * (spam.per_attempt_lamports as u128)
        );
        assert!(spam.expected_failed_cost() > 0);
    }

    #[test]
    fn conservative_below_raw_and_below_optimistic() {
        // Profitable: 1_000_000 base units net, 5 bps (below richness threshold).
        let net = net_with(1_000_000, 5, 200_000_000);
        let out = evaluate(
            &net,
            &EvalParams::optimistic(),
            &EvalParams::conservative_default(),
        );
        assert_eq!(
            out.optimistic.e_pnl, net.net_profit,
            "optimistic == raw net"
        );
        assert!(
            out.conservative.e_pnl < net.net_profit,
            "conservative haircuts raw net"
        );
        assert!(
            out.conservative.e_pnl <= out.optimistic.e_pnl,
            "conservative <= optimistic"
        );
    }

    #[test]
    fn losing_cycle_never_positive() {
        let net = net_with(-50_000, -3, 200_000_000);
        let out = evaluate(
            &net,
            &EvalParams::optimistic(),
            &EvalParams::conservative_default(),
        );
        assert!(out.optimistic.e_pnl <= 0);
        assert!(out.conservative.e_pnl <= 0);
        // Conservative is more negative (failed costs added).
        assert!(out.conservative.e_pnl <= out.optimistic.e_pnl);
    }

    #[test]
    fn evaluate_is_deterministic() {
        let net = net_with(1_000_000, 5, 200_000_000);
        let a = evaluate(
            &net,
            &EvalParams::optimistic(),
            &EvalParams::conservative_default(),
        );
        let b = evaluate(
            &net,
            &EvalParams::optimistic(),
            &EvalParams::conservative_default(),
        );
        assert_eq!(a, b);
    }
}
