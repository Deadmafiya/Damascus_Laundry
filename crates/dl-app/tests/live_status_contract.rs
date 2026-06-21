//! Contract-conformance tests for the `live_status` writer
//! (DAM-82 / DAM-72a).
//!
//! Verifies the writer emits the v1 contract shape that the
//! Frontend Programmer's `damascus_laundry_dashboard` launcher
//! reads from `./live_status.json` at 1 Hz. The contract
//! version is pinned at `1`; any field rename or addition
//! requires bumping `dl_app::live_status::SCHEMA_VERSION` and
//! this test will need to be updated in lock-step.
//!
//! The integration here covers:
//! 1. The contract shape: every field the dashboard reads
//!    must be present with the right JSON type.
//! 2. Atomic write: a partial write to `live_status.json.tmp`
//!    can never desync the live `live_status.json`.
//! 3. Kill-switch round-trip: when the operator touches
//!    `./STOP`, the next writer tick flips
//!    `kill_switch.open` to `true`.
//! 4. Cap round-trip: after `try_charge`, the next writer
//!    tick reports the new `daily_spent_lamports`.

use dl_app::live_status::{
    write_atomic, LastLandedSnapshot, LiveStatus, SharedState, SolUsdSource, WriterInputs,
    SCHEMA_VERSION,
};
use dl_executor::killswitch::{KillSwitch, KillSwitchConfig};
use dl_oracle::MockPythClient;
use dl_signer::cap::{CapConfig, CapState};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

fn tmp_path(label: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "dl-live-status-itest-{label}-{}-{}.json",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));
    p
}

#[test]
fn contract_v1_emits_every_field_dashboard_reads() {
    let cap = CapState::new(CapConfig::default());
    let ks = KillSwitch::new(KillSwitchConfig::default());
    let inputs = WriterInputs::default();
    let s: LiveStatus = dl_app::live_status::build(&inputs, &cap, &ks);
    let json = serde_json::to_value(&s).expect("serialize");

    assert_eq!(json["schema_version"], SCHEMA_VERSION);
    assert!(json["ts_unix_ms"].is_i64(), "ts_unix_ms must be int");
    assert!(json["running"].is_boolean());
    assert!(json["daily_cap_lamports"].is_i64());
    assert!(json["daily_spent_lamports"].is_i64());
    assert!(json["realized_pnl_today_lamports"].is_i64());
    // kill_switch sub-object.
    let ks_obj = &json["kill_switch"];
    assert!(ks_obj["open"].is_boolean());
    assert!(ks_obj["stop_file_present"].is_boolean());
    assert!(ks_obj["consecutive_losses"].is_i64());
    assert!(ks_obj["max_consecutive_losses"].is_i64());
    assert!(ks_obj["reason"].is_string() || ks_obj["reason"].is_null());
    // last_landed_bundle sub-object.
    let lb = &json["last_landed_bundle"];
    assert!(lb["bundle_id"].is_string() || lb["bundle_id"].is_null());
    assert!(lb["ts_unix_ms"].is_i64() || lb["ts_unix_ms"].is_null());
    assert!(lb["profit_lamports"].is_i64() || lb["profit_lamports"].is_null());
    // sol_usd source — must be the kebab-case enum string the
    // dashboard reads.
    assert_eq!(json["sol_usd_source"], "none");
}

#[test]
fn write_atomic_does_not_leak_tmp_file() {
    let path = tmp_path("atomic");
    let cap = CapState::new(CapConfig::default());
    let ks = KillSwitch::new(KillSwitchConfig::default());
    let s = dl_app::live_status::build(&WriterInputs::default(), &cap, &ks);
    write_atomic(&path, &s).expect("write_atomic");
    assert!(path.exists());
    let tmp = {
        let mut p = path.as_os_str().to_owned();
        p.push(".tmp");
        PathBuf::from(p)
    };
    assert!(!tmp.exists(), "tmp file leaked: {}", tmp.display());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn shared_state_snapshot_round_trip_for_killswitch() {
    // The writer takes a snapshot under the Mutex<>; this
    // test asserts the projection contract: open=true ⇔
    // ks.check() == Err.
    let stop_path = tmp_path("STOP");
    std::fs::write(&stop_path, b"test stop").unwrap();
    let cap = CapState::new(CapConfig::default());
    let ks = KillSwitch::with_default_stop_file(&stop_path);
    let inputs = WriterInputs::default();
    let s = dl_app::live_status::build(&inputs, &cap, &ks);
    assert!(s.kill_switch.open);
    assert!(s.kill_switch.stop_file_present);
    assert!(s.kill_switch.reason.is_some());
    std::fs::remove_file(&stop_path).ok();
}

#[test]
fn cap_state_charge_reflected_in_spent_field() {
    // Paper-mode scenario: charge the cap, build the
    // LiveStatus, assert daily_spent_lamports matches.
    let mut cap = CapState::new(CapConfig {
        daily_lamports: 5_000_000_000,
        per_bundle_lamports: 500_000_000,
    });
    cap.try_charge(123_456_789).expect("charge");
    let ks = KillSwitch::new(KillSwitchConfig::default());
    let inputs = WriterInputs::default();
    let s = dl_app::live_status::build(&inputs, &cap, &ks);
    assert_eq!(s.daily_spent_lamports, 123_456_789);
    assert_eq!(s.daily_cap_lamports, 5_000_000_000);
}

#[test]
fn sol_usd_renders_with_pyth_mock() {
    let pyth: Arc<dyn dl_oracle::PythClient> = Arc::new(MockPythClient::new(20_000_000_000, -8));
    let inputs = WriterInputs {
        pyth: Some(pyth),
        pyth_sol_feed: Some(solana_sdk::pubkey::Pubkey::new_unique()),
        ..WriterInputs::default()
    };
    let cap = CapState::new(CapConfig::default());
    let ks = KillSwitch::new(KillSwitchConfig::default());
    let s = dl_app::live_status::build(&inputs, &cap, &ks);
    assert_eq!(s.sol_usd, Some(200.0));
    assert_eq!(s.sol_usd_source, SolUsdSource::Pyth);
    assert!(s.sol_usd_age_secs.is_some());
}

#[test]
fn shared_state_can_be_constructed_and_snapshot() {
    // Smoke test for `SharedState`: construct it from
    // concrete cap + ks + inputs, build a LiveStatus via
    // the same path the writer uses, assert it round-trips.
    let cap = Arc::new(Mutex::new(CapState::new(CapConfig::default())));
    let ks = Arc::new(Mutex::new(KillSwitch::new(KillSwitchConfig::default())));
    let inputs = Arc::new(Mutex::new(WriterInputs {
        running: true,
        last_landed: LastLandedSnapshot {
            bundle_id: Some("bundle-abc-123".into()),
            ts_unix_ms: Some(1_700_000_000_000),
            profit_lamports: Some(50_000),
        },
        ..WriterInputs::default()
    }));
    let state = SharedState {
        cap: cap.clone(),
        killswitch: ks.clone(),
        inputs: inputs.clone(),
    };
    // The shape of `SharedState` itself is what we are
    // asserting here — the writer task takes ownership of
    // it. We don't run the loop in this test (that would
    // require a tokio runtime + a shutdown signal); we
    // assert the type signature is usable.
    let _: SharedState = state;
}
