use dl_paper::{PaperWallet, TradeFill, Side};

fn fill(profit: i64) -> TradeFill {
    TradeFill {
        pair: "SOL/USDC".to_string(),
        side: Side::BaseToQuote,
        input_lamports: 1_000_000,
        output_lamports: 1_000_000,
        profit_lamports: profit,
        tip_lamports: 1_000,
        cycle_hash_hex: "x".to_string(),
    }
}

#[test]
fn empty_wallet_stats() {
    let w = PaperWallet::new(10_000_000_000);
    let s = w.stats();
    assert_eq!(s.current_balance_lamports, 10_000_000_000);
    assert_eq!(s.total_trades, 0);
    assert_eq!(s.wins, 0);
    assert_eq!(s.losses, 0);
    assert_eq!(s.total_pnl_lamports, 0);
    assert_eq!(s.max_drawdown_lamports, 0);
    assert_eq!(s.peak_balance_lamports, 10_000_000_000);
}

#[test]
fn mixed_wins_losses_counted() {
    let mut w = PaperWallet::new(10_000_000_000);
    w.execute(fill(100_000)).unwrap();
    w.execute(fill(-50_000)).unwrap();
    w.execute(fill(200_000)).unwrap();
    w.execute(fill(-10_000)).unwrap();
    let s = w.stats();
    assert_eq!(s.total_trades, 4);
    assert_eq!(s.wins, 2);
    assert_eq!(s.losses, 2);
    assert_eq!(s.total_pnl_lamports, 100_000 - 50_000 + 200_000 - 10_000);
}

#[test]
fn max_drawdown_tracks_peak() {
    let mut w = PaperWallet::new(10_000_000_000);
    w.execute(fill(1_000_000)).unwrap(); // peak 10_001_000_000
    w.execute(fill(-2_000_000)).unwrap(); // down to 9_999_000_000
    let s = w.stats();
    assert!(s.peak_balance_lamports >= 10_001_000_000);
    assert!(s.max_drawdown_lamports >= 2_000_000);
}
