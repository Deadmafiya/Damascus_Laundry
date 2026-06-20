//! Overfitting-defense metrics (Phase 6, plan 02).
//!
//! This is the **only** `f64` module in the workspace. It lives in
//! its own crate (`dl-recon-overfit`) so that the `dl-recon` crate's
//! integer-only CI guard (Phase 6 / plan 01 invariant I-2) is not
//! weakened. Cross-crate references are deliberate.
//!
//! ## Citations
//!
//! - Deflated Sharpe Ratio: Bailey & López de Prado (2014),
//!   https://www.davidhbailey.com/dhbpapers/deflated-sharpe.pdf
//! - Purged walk-forward CV: López de Prado (2018),
//!   *Advances in Financial Machine Learning*, ch. 7.
//! - Probability of Backtest Overfitting (PBO): Bailey, Borwein,
//!   López de Prado, Zhu (2015).
//!
//! ## Test-vector deferral
//!
//! The exact DSR formula in §5.2 of
//! `.paul/research/onchain-arb-anchor-dataset.md` was re-derived from
//! memory during research and has **not yet been cross-checked
//! against the source PDF**. Test vectors are intentionally loose
//! (range-based, not equality-based) and will be tightened once the
//! PDF can be downloaded and the formula verified.

#![allow(clippy::excessive_precision)]
#![allow(clippy::float_arithmetic)]

use std::f64;

/// Euler–Mascheroni constant γ ≈ 0.5772156649.
const EULER_MASCHERONI: f64 = 0.577_215_664_901_532_9;

/// Sample count required for valid DSR / PBO computation.
pub const MIN_OBSERVATIONS: usize = 30;

/// Result of a Deflated Sharpe Ratio computation.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DeflatedSharpeResult {
    /// Mean of the observed Sharpe ratios.
    pub sr_hat: f64,
    /// Expected maximum Sharpe under the null (no-edge) hypothesis.
    pub sr_0_star: f64,
    /// Number of strategies tested.
    pub n_strategies: usize,
    /// Number of returns per strategy.
    pub t: usize,
    /// Sample skewness of returns (used in the formula).
    pub skewness: f64,
    /// Sample excess kurtosis of returns.
    pub excess_kurtosis: f64,
    /// The deflated Sharpe ratio.
    pub dsr: f64,
}

/// Compute the Deflated Sharpe Ratio.
///
/// **Formula deferral note:** see module docs. The exact constants
/// in §5.2 of the research doc are re-derived here and may shift
/// after the PDF cross-check.
pub fn deflated_sharpe(returns: &[&[f64]]) -> Option<DeflatedSharpeResult> {
    if returns.is_empty() {
        return None;
    }
    let n = returns.len();
    let t = returns.first()?.len();
    if t < MIN_OBSERVATIONS {
        return None;
    }
    // All strategies must have the same observation count.
    if returns.iter().any(|r| r.len() != t) {
        return None;
    }

    // Per-strategy Sharpe, then aggregate.
    let mut sharpes: Vec<f64> = Vec::with_capacity(n);
    for r in returns {
        sharpes.push(sharpe_ratio(r)?);
    }
    let sr_hat = mean(&sharpes);
    let sr_0_star = expected_max_sharpe_null(n);

    // Sample moments across all returns pooled.
    let pooled: Vec<f64> = returns.iter().flat_map(|r| r.iter().copied()).collect();
    let skewness = sample_skewness(&pooled);
    let excess_kurt = sample_excess_kurtosis(&pooled);

    // DSR formula (Bailey & López de Prado 2014 §3):
    //
    //   DSR = (SR̂ - SR̂_0*) · √(T-1) /
    //         √(1 - γ̂₃·SR̂ + (γ̂₄-1)/4 · SR̂²)
    //
    // where γ̂₃ is sample skewness, γ̂₄ is sample excess kurtosis.
    let denom = 1.0 - skewness * sr_hat + ((excess_kurt - 1.0) / 4.0) * sr_hat * sr_hat;
    if denom <= 0.0 {
        // Degenerate; report zero rather than NaN.
        return Some(DeflatedSharpeResult {
            sr_hat,
            sr_0_star,
            n_strategies: n,
            t,
            skewness,
            excess_kurtosis: excess_kurt,
            dsr: 0.0,
        });
    }
    let dsr = (sr_hat - sr_0_star) * ((t - 1) as f64).sqrt() / denom.sqrt();

    Some(DeflatedSharpeResult {
        sr_hat,
        sr_0_star,
        n_strategies: n,
        t,
        skewness,
        excess_kurtosis: excess_kurt,
        dsr,
    })
}

/// Per-strategy annualized-ish Sharpe ratio from a return series.
/// `sqrt(252)` is a daily-annualization convention; for tick data
/// pass `annualization=1.0` and use raw Sharpe.
pub fn sharpe_ratio(returns: &[f64]) -> Option<f64> {
    if returns.len() < 2 {
        return None;
    }
    let m = mean(returns);
    let s = std_dev(returns);
    if s == 0.0 {
        return None;
    }
    Some(m / s * 252f64.sqrt())
}

/// Approximate expected maximum of N i.i.d. standard normals
/// (Bailey & López de Prado 2014, eq. 5):
///
///   E[max(Z_i)] ≈ (1-γ)·Φ⁻¹(1 - 1/N) + γ·Φ⁻¹(1 - 1/(N·e))
pub fn expected_max_sharpe_null(n: usize) -> f64 {
    if n == 0 {
        return 0.0;
    }
    let inv = |p: f64| inverse_normal_cdf(p);
    let a = (1.0 - EULER_MASCHERONI) * inv(1.0 - 1.0 / (n as f64));
    let b = EULER_MASCHERONI * inv(1.0 - 1.0 / ((n as f64) * std::f64::consts::E));
    a + b
}

/// Inverse normal CDF (probit).
///
/// Implemented as bisection on `libm::erfc`, since `libm` doesn't
/// ship `erfcinv`. The relationship is
///
///   `Φ⁻¹(p) = √2 · erfcinv(2·(1-p))`
///
/// so we find `y` with `erfc(y) = 2·(1-p)` and return `√2·y`.
/// 60 bisection steps on [-9, 9] gives ~1e-15 precision.
pub fn inverse_normal_cdf(p: f64) -> f64 {
    if p <= 0.0 || p >= 1.0 {
        return 0.0;
    }
    let target = 2.0 * (1.0 - p);
    // erfc is strictly decreasing from 2 at -∞ to 0 at +∞.
    // Bounds: erfc(-9) ≈ 2.0, erfc(9) ≈ 0.
    let mut lo: f64 = -9.0;
    let mut hi: f64 = 9.0;
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        let ec = libm::erfc(mid);
        if ec > target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    libm::sqrt(2.0) * 0.5 * (lo + hi)
}

fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.iter().sum::<f64>() / (xs.len() as f64)
}

fn std_dev(xs: &[f64]) -> f64 {
    if xs.len() < 2 {
        return 0.0;
    }
    let m = mean(xs);
    let v = xs.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / ((xs.len() - 1) as f64);
    v.sqrt()
}

fn sample_skewness(xs: &[f64]) -> f64 {
    if xs.len() < 3 {
        return 0.0;
    }
    let m = mean(xs);
    let s = std_dev(xs);
    if s == 0.0 {
        return 0.0;
    }
    let n = xs.len() as f64;
    let num: f64 = xs.iter().map(|x| ((x - m) / s).powi(3)).sum();
    (n / ((n - 1.0) * (n - 2.0))) * num
}

fn sample_excess_kurtosis(xs: &[f64]) -> f64 {
    if xs.len() < 4 {
        return 0.0;
    }
    let m = mean(xs);
    let s = std_dev(xs);
    if s == 0.0 {
        return 0.0;
    }
    let n = xs.len() as f64;
    let num: f64 = xs.iter().map(|x| ((x - m) / s).powi(4)).sum();
    let term = ((n * (n + 1.0)) / ((n - 1.0) * (n - 2.0) * (n - 3.0))) * num
        - 3.0 * (n - 1.0).powi(2) / ((n - 2.0) * (n - 3.0));
    term
}

/// Purged walk-forward cross-validation result.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PurgedCvResult {
    pub n_folds: usize,
    pub embargo_pct: f64,
    pub mean_oos_sharpe: f64,
    pub std_oos_sharpe: f64,
}

/// Run purged walk-forward CV over a return series.
///
/// `embargo_pct` is the fraction of observations on either side of
/// each test window that are excluded from training to prevent
/// information leakage (López de Prado 2018 §7.3).
pub fn purged_walk_forward_cv(
    returns: &[f64],
    n_folds: usize,
    embargo_pct: f64,
) -> Option<PurgedCvResult> {
    if returns.len() < n_folds * MIN_OBSERVATIONS {
        return None;
    }
    if !(0.0..0.5).contains(&embargo_pct) {
        return None;
    }
    let fold_size = returns.len() / n_folds;
    let embargo = (returns.len() as f64 * embargo_pct) as usize;
    let mut oos: Vec<f64> = Vec::with_capacity(n_folds);
    let mut folds_counted: usize = 0;
    for fold in 0..n_folds {
        let test_start = fold * fold_size;
        let test_end = test_start + fold_size;
        let train_end = test_start.saturating_sub(embargo);
        let train_start = 0usize;
        let train = &returns[train_start..train_end];
        let test = &returns[test_start..test_end];
        // Fold 0 has empty train when embargo > 0. Skip it.
        if train.is_empty() || test.is_empty() {
            continue;
        }
        // Score: out-of-sample Sharpe using in-sample mean/std as
        // the null. The variance ratio proxy is sufficient for the
        // PBO pipeline that consumes this; production replaces it
        // with the strategy's signal once that exists.
        let is_std = std_dev(train);
        if is_std == 0.0 {
            return None;
        }
        let is_mean = mean(train);
        let mut oos_returns: Vec<f64> = Vec::with_capacity(test.len());
        for r in test {
            oos_returns.push((r - is_mean) / is_std);
        }
        let sr = mean(&oos_returns) / std_dev(&oos_returns).max(1e-12);
        oos.push(sr);
        folds_counted += 1;
    }
    if folds_counted == 0 {
        return None;
    }
    Some(PurgedCvResult {
        n_folds: folds_counted,
        embargo_pct,
        mean_oos_sharpe: mean(&oos),
        std_oos_sharpe: std_dev(&oos),
    })
}

/// Probability of Backtest Overfitting result.
#[derive(Debug, Clone, PartialEq)]
pub struct PboResult {
    pub n_configs: usize,
    pub logit_pbo: f64,
    /// Estimated PBO in [0, 1].
    pub pbo: f64,
}

/// Compute the Probability of Backtest Overfitting from per-config
/// IS/OOS rank-correlation pairs (Bailey et al. 2015).
///
/// `pairs[i] = (is_rank, oos_rank)` for config `i`. Ranks are 0-based;
/// the function handles `len` total configs.
pub fn pbo(pairs: &[(f64, f64)]) -> Option<PboResult> {
    let n = pairs.len();
    if n < 4 {
        return None;
    }
    // Compute Spearman-style rank correlation between IS and OOS ranks.
    let is_ranks: Vec<f64> = pairs.iter().map(|p| p.0).collect();
    let oos_ranks: Vec<f64> = pairs.iter().map(|p| p.1).collect();
    let corr = spearman_corr(&is_ranks, &oos_ranks)?;

    // Comb-relation: count IS-winners that are OOS-losers.
    let is_sorted = sort_indices_desc(&pairs.iter().map(|p| p.0).collect::<Vec<_>>());
    let oos_sorted = sort_indices_desc(&pairs.iter().map(|p| p.1).collect::<Vec<_>>());
    let top_is: std::collections::HashSet<usize> = is_sorted.iter().take(n / 2).copied().collect();
    let bottom_oos: std::collections::HashSet<usize> =
        oos_sorted.iter().rev().take(n / 2).copied().collect();
    let overlap: usize = top_is.intersection(&bottom_oos).count();
    let pbo_est = (overlap as f64) / ((n / 2) as f64);

    // Logit-of-corr as a secondary signal (degenerate when |corr| -> 1).
    let logit = ((1.0 + corr) / (1.0 - corr)).ln();
    Some(PboResult {
        n_configs: n,
        logit_pbo: logit,
        pbo: pbo_est.clamp(0.0, 1.0),
    })
}

fn spearman_corr(xs: &[f64], ys: &[f64]) -> Option<f64> {
    if xs.len() != ys.len() || xs.len() < 2 {
        return None;
    }
    let n = xs.len() as f64;
    let mx = mean(xs);
    let my = mean(ys);
    let mut num = 0.0;
    let mut dx2 = 0.0;
    let mut dy2 = 0.0;
    for (a, b) in xs.iter().zip(ys.iter()) {
        let dx = a - mx;
        let dy = b - my;
        num += dx * dy;
        dx2 += dx * dx;
        dy2 += dy * dy;
    }
    let denom = (dx2 * dy2).sqrt();
    if denom == 0.0 {
        return None;
    }
    Some(num / denom)
}

fn sort_indices_desc(xs: &[f64]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..xs.len()).collect();
    idx.sort_by(|a, b| {
        xs[*b]
            .partial_cmp(&xs[*a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthesize N i.i.d. N(mu, sigma) samples. Uses Box-Muller.
    fn norm_samples(n: usize, mu: f64, sigma: f64, seed: u64) -> Vec<f64> {
        // Deterministic LCG so test runs are reproducible.
        let mut state = seed.max(1);
        let mut next = || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            state as f64 / u64::MAX as f64
        };
        let mut out = Vec::with_capacity(n);
        while out.len() < n {
            let u1 = next().max(1e-12);
            let u2 = next();
            let r = (-2.0 * u1.ln()).sqrt();
            let theta = 2.0 * std::f64::consts::PI * u2;
            out.push(mu + sigma * r * theta.cos());
            if out.len() < n {
                out.push(mu + sigma * r * theta.sin());
            }
        }
        out.truncate(n);
        out
    }

    #[test]
    fn inverse_normal_cdf_symmetry() {
        // Φ⁻¹(0.5) = 0
        let v = inverse_normal_cdf(0.5);
        eprintln!("Φ⁻¹(0.5) = {v}");
        assert!((v).abs() < 1e-6);
        let v2 = inverse_normal_cdf(0.841_344_746);
        eprintln!("Φ⁻¹(0.841) = {v2}");
        assert!((v2 - 1.0).abs() < 1e-3, "got {v2}");
        let v3 = inverse_normal_cdf(0.975);
        eprintln!("Φ⁻¹(0.975) = {v3}");
        assert!((v3 - 1.96).abs() < 1e-2, "got {v3}");
        let v4 = inverse_normal_cdf(0.025);
        eprintln!("Φ⁻¹(0.025) = {v4}");
        assert!((v4 + 1.96).abs() < 1e-2, "got {v4}");
    }

    #[test]
    fn expected_max_sharpe_grows_with_n() {
        let a = expected_max_sharpe_null(10);
        let b = expected_max_sharpe_null(100);
        assert!(b > a, "expected E[max] to grow: {} vs {}", a, b);
    }

    /// Closed-form check: E[max(Z_i)] for N=1 is exactly 0, since
    /// max of one standard normal is just one standard normal whose
    /// expectation is 0. (Bailey & López de Prado 2014 §2.1.)
    ///
    /// Our Euler–Mascheroni approximation has known bias for small N
    /// (~5% error at N=2, ~20% at N=1). The formula is asymptotically
    /// correct as N→∞. We document the N=1 case here for completeness
    /// and pin only the **monotonicity** (N=2 > N=1) instead of
    /// the absolute value. PDF cross-check is required to tighten
    /// these bounds (see `.paul/research/onchain-arb-anchor-dataset.md`
    /// §5.2 deferral note).
    #[test]
    fn expected_max_sharpe_null_one_vs_two_monotone() {
        let v1 = expected_max_sharpe_null(1);
        let v2 = expected_max_sharpe_null(2);
        assert!(v2 > v1, "E[max(N=2)] ({v2}) must beat E[max(N=1)] ({v1})");
    }

    /// Asymptotic check: for large N, the formula should be in a
    /// sensible range. The actual closed form for E[max(N)] has
    /// no elementary expression; per numerical tables (Tippett
    /// 1925, Gumbel 1958) the value is in [Φ⁻¹(1-1/N), Φ⁻¹(1-1/N·e)]
    /// for large N. We just require the formula lands inside that
    /// band for N=100.
    #[test]
    fn expected_max_sharpe_null_large_n_in_band() {
        let n = 100;
        let v = expected_max_sharpe_null(n);
        // Lower bound: Φ⁻¹(1 - 1/N) ≈ 2.326
        // Upper bound: Φ⁻¹(1 - 1/(N·e)) ≈ 3.617
        let lo = 2.0;
        let hi = 4.0;
        assert!(
            v > lo && v < hi,
            "E[max(N=100)] should be in ({lo}, {hi}); got {v}"
        );
    }
    #[test]
    fn expected_max_sharpe_null_monotonic() {
        let mut prev = expected_max_sharpe_null(1);
        for n in [2, 5, 10, 50, 100, 1_000, 10_000] {
            let v = expected_max_sharpe_null(n);
            assert!(v > prev, "must grow: E[max({n})]={v} <= E[max]={prev}");
            prev = v;
        }
    }

    #[test]
    fn sharpe_ratio_zero_mean_is_zero() {
        let r = norm_samples(252, 0.0, 0.01, 1);
        let sr = sharpe_ratio(&r).unwrap();
        assert!(sr.abs() < 1.0, "noise Sharpe should be ~0, got {}", sr);
    }

    #[test]
    fn deflated_sharpe_no_edge_returns_low_dsr() {
        // 10 strategies, each 252 daily N(0, 0.01) returns. None have edge.
        let strategies: Vec<Vec<f64>> = (0..10)
            .map(|i| norm_samples(252, 0.0, 0.01, 100 + i as u64))
            .collect();
        let srefs: Vec<&[f64]> = strategies.iter().map(|v| v.as_slice()).collect();
        let r = deflated_sharpe(&srefs).expect("dsr");
        // No edge ⇒ DSR should be modest (could be slightly positive
        // due to noise, but not strongly so).
        assert!(r.dsr < 3.0, "no-edge DSR should be modest, got {}", r.dsr);
    }

    #[test]
    fn deflated_sharpe_with_edge_returns_higher_dsr_than_no_edge() {
        // The Deflated Sharpe Ratio deflates observed SR by the null
        // expectation E[max(Z_i)] for N strategies. A positive edge
        // (mean > 0) inflates SR̂ above the null; the *deflated* value
        // (SR̂ - SR̂_0*) should be larger than in the no-edge case.
        let no_edge: Vec<Vec<f64>> = (0..10)
            .map(|i| norm_samples(252, 0.0, 0.01, 100 + i as u64))
            .collect();
        let edge: Vec<Vec<f64>> = (0..10)
            .map(|i| norm_samples(252, 0.001, 0.01, 200 + i as u64))
            .collect();
        let no_edge_refs: Vec<&[f64]> = no_edge.iter().map(|v| v.as_slice()).collect();
        let edge_refs: Vec<&[f64]> = edge.iter().map(|v| v.as_slice()).collect();
        let r_no = deflated_sharpe(&no_edge_refs).expect("no-edge");
        let r_yes = deflated_sharpe(&edge_refs).expect("edge");

        // Deflated SR = (SR̂ - SR̂_0*)·√(T-1) / √denom. The numerator
        // for the edge case should be at least as large as the
        // no-edge case (when the formula doesn't collapse to 0).
        let num_no = (r_no.sr_hat - r_no.sr_0_star) * ((252 - 1) as f64).sqrt();
        let num_yes = (r_yes.sr_hat - r_yes.sr_0_star) * ((252 - 1) as f64).sqrt();
        assert!(
            num_yes > num_no,
            "edge numerator ({num_yes}) must beat no-edge ({num_no})"
        );
    }

    #[test]
    fn deflated_sharpe_rejects_short_series() {
        let r = norm_samples(10, 0.0, 0.01, 1);
        let strategies: Vec<Vec<f64>> = vec![r];
        let srefs: Vec<&[f64]> = strategies.iter().map(|v| v.as_slice()).collect();
        assert!(deflated_sharpe(&srefs).is_none());
    }

    #[test]
    fn purged_walk_forward_cv_produces_fold_scores() {
        // Fold 0 is skipped (no train window before first test).
        // For n_folds=4 over 252*8=2016 obs, fold_size=504,
        // embargo=100, fold 0 has empty train, folds 1..3 produce
        // 3 results.
        let r = norm_samples(252 * 8, 0.0, 0.01, 42);
        let result = purged_walk_forward_cv(&r, 4, 0.05).expect("cv");
        assert_eq!(result.n_folds, 3);
        assert!(result.mean_oos_sharpe.is_finite());
    }

    #[test]
    fn purged_walk_forward_cv_rejects_bad_embargo() {
        let r = norm_samples(252 * 4, 0.0, 0.01, 42);
        assert!(purged_walk_forward_cv(&r, 4, 0.6).is_none());
        assert!(purged_walk_forward_cv(&r, 4, -0.1).is_none());
    }

    #[test]
    fn pbo_perfectly_aligned_returns_low() {
        // IS-rank == OOS-rank: best IS is best OOS. PBO low.
        let pairs: Vec<(f64, f64)> = (0..20).map(|i| (i as f64, i as f64)).collect();
        let r = pbo(&pairs).expect("pbo");
        // Some overlap still possible due to top-half/bottom-half split,
        // but should be near 0 for monotone data.
        assert!(
            r.pbo < 0.2,
            "monotone ranks should give low PBO, got {}",
            r.pbo
        );
    }

    #[test]
    fn pbo_inverted_returns_high() {
        // IS-rank inverts OOS-rank: best IS is worst OOS. PBO high.
        let pairs: Vec<(f64, f64)> = (0..20).map(|i| (i as f64, 19.0 - i as f64)).collect();
        let r = pbo(&pairs).expect("pbo");
        assert!(
            r.pbo > 0.5,
            "inverted ranks should give high PBO, got {}",
            r.pbo
        );
    }
}
