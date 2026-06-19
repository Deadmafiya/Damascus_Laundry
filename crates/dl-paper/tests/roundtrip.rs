use dl_paper::{PaperWallet, Side, TradeFill};

#[test]
fn roundtrip_save_load() {
    let dir = tempdir();
    let path = dir.join("wallet.json");
    let mut w = PaperWallet::new(10_000_000_000);
    w.execute(TradeFill {
        pair: "SOL/USDC".to_string(),
        side: Side::BaseToQuote,
        input_lamports: 1_000_000,
        output_lamports: 1_010_000,
        profit_lamports: 10_000,
        tip_lamports: 5_000,
        cycle_hash_hex: "deadbeef".to_string(),
    }).unwrap();
    w.save(&path).unwrap();
    let loaded = PaperWallet::load(&path).unwrap();
    assert_eq!(loaded.balance_lamports, w.balance_lamports);
    assert_eq!(loaded.trades.len(), 1);
    assert_eq!(loaded.trades[0].profit_lamports, 10_000);
    cleanup(&dir);
}

#[test]
fn execute_updates_balance_and_appends_trade() {
    let mut w = PaperWallet::new(10_000_000_000);
    let id = w.execute(TradeFill {
        pair: "SOL/USDT".to_string(),
        side: Side::QuoteToBase,
        input_lamports: 1_000_000,
        output_lamports: 1_005_000,
        profit_lamports: 5_000,
        tip_lamports: 1_000,
        cycle_hash_hex: "01".to_string(),
    }).unwrap();
    assert_eq!(id, 0);
    assert_eq!(w.balance_lamports, 10_000_000_000 + 5_000);
    assert_eq!(w.trades.len(), 1);
}

#[test]
fn insufficient_funds_returns_error() {
    let mut w = PaperWallet::new(100);
    let r = w.execute(TradeFill {
        pair: "X".to_string(),
        side: Side::BaseToQuote,
        input_lamports: 1_000_000,
        output_lamports: 1_010_000,
        profit_lamports: -50_000,
        tip_lamports: 1_000,
        cycle_hash_hex: "0".to_string(),
    });
    assert!(r.is_err());
}

#[test]
fn atomic_write_does_not_leave_tmp() {
    let dir = tempdir();
    let path = dir.join("wallet.json");
    let w = PaperWallet::new(10_000_000_000);
    w.save(&path).unwrap();
    let tmp = path.with_extension("json.tmp");
    assert!(!tmp.exists(), "tmp file should be cleaned up after rename");
    assert!(path.exists());
    cleanup(&dir);
}

fn tempdir() -> std::path::PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let p = std::env::temp_dir().join(format!("dl-paper-{}-{}-{}", std::process::id(), nanos, line!()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn cleanup(p: &std::path::Path) {
    let _ = std::fs::remove_dir_all(p);
}
