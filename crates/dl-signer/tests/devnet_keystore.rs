//! Devnet keystore ed25519 round-trip (sub-plan 0b).
//!
//! Validates that the secret bytes produced by `dl_signer::KeyFile`
//! are interoperable with a standard ed25519 verifier — which is
//! what `solana-sdk::Keypair::try_from(&secret_bytes)` uses in
//! sub-plan 1a when wiring the real signer into the bundle path.
//!
//! The test runs in CI without a live devnet RPC. The optional
//! `DL_DEVNET_KEYFILE` env var lets an operator re-run it against
//! an existing keyfile as a manual sanity check before the first
//! live run.
//!
//! No solana-sdk / solana-client deps — dl-signer is intentionally
//! dep-light. The on-chain transfer validation lives in the
//! runbook (`docs/live-runbook.md` §3a) as a manual step.

use dl_signer::keystore::{KeyFile, KeyStore};

fn test_message() -> &'static [u8] {
    b"damascus-laundry-devnet-keystore-roundtrip-v1"
}

#[test]
fn keyfile_secret_bytes_interop_with_ed25519_dalek() {
    // 1. Generate an in-memory keyfile with a known passphrase.
    let passphrase = "test-passphrase-0b-roundtrip";
    let kf = KeyFile::new(passphrase);
    let secret = kf.decrypt(passphrase).expect("decrypt with right passphrase");

    // 2. The secret must be exactly 32 bytes (ed25519 private key size).
    assert_eq!(secret.len(), 32, "keyfile secret must be 32 bytes (ed25519)");

    // 3. Derive the ed25519 verifying key from the secret.
    let signing_key = ed25519_dalek::SigningKey::from_bytes(
        secret.as_ref().try_into().expect("32-byte secret slice"),
    );
    let verifying_key = signing_key.verifying_key();
    let expected_pubkey: [u8; 32] = verifying_key.to_bytes();

    // 4. Confirm dl-signer's pubkey matches ed25519-dalek's.
    let ks = KeyStore::from_secret(secret.clone());
    let ks_pubkey = ks.public_key_for_print();
    assert_eq!(
        ks_pubkey, expected_pubkey,
        "dl-signer pubkey must equal ed25519-dalek verifying key"
    );

    // 5. Sign a message and verify it.
    use ed25519_dalek::Signer;
    let signature = signing_key.sign(test_message());
    let valid = ed25519_dalek::Verifier::verify(
        &verifying_key,
        test_message(),
        &signature,
    );
    assert!(valid.is_ok(), "signature must verify");
}

#[test]
fn keyfile_from_disk_decrypts_and_signs_if_env_set() {
    // Manual operator sanity check: when DL_DEVNET_KEYFILE points
    // at a real keyfile, this test loads it and re-runs the
    // ed25519 round-trip. Skipped otherwise.
    let path = match std::env::var("DL_DEVNET_KEYFILE") {
        Ok(p) if !p.is_empty() => p,
        _ => {
            eprintln!("DL_DEVNET_KEYFILE not set — skipping disk-roundtrip test");
            return;
        }
    };
    let passphrase = match std::env::var("DL_SIGNER_PASSPHRASE") {
        Ok(p) if !p.is_empty() => p,
        _ => panic!("DL_DEVNET_KEYFILE set but DL_SIGNER_PASSPHRASE missing"),
    };

    let kf = KeyFile::load(std::path::Path::new(&path)).expect("load keyfile from disk");
    let secret = kf.decrypt(&passphrase).expect("decrypt disk keyfile");
    assert_eq!(secret.len(), 32);

    let signing_key = ed25519_dalek::SigningKey::from_bytes(
        secret.as_ref().try_into().expect("32-byte secret"),
    );
    let verifying_key = signing_key.verifying_key();
    let ks = KeyStore::from_secret(secret);
    assert_eq!(ks.public_key_for_print(), verifying_key.to_bytes());

    use ed25519_dalek::{Signer, Verifier};
    let sig = signing_key.sign(test_message());
    Verifier::verify(&verifying_key, test_message(), &sig)
        .expect("disk-keyfile signature must verify");
}

#[test]
fn wrong_passphrase_fails_cleanly() {
    // Already covered by keystore.rs::wrong_passphrase_fails, but
    // re-asserted here for the devnet_keystore story: a bad
    // passphrase must NOT produce a 32-byte secret that "looks"
    // valid to ed25519. It should decrypt-error before any signing.
    let kf = KeyFile::new("right-passphrase");
    let err = kf.decrypt("wrong-passphrase");
    assert!(err.is_err(), "wrong passphrase must fail to decrypt");
}

#[test]
fn keyfile_persist_roundtrip_via_bytes_and_back() {
    // Validates that KeyFile::to_bytes → from_bytes → decrypt gives
    // back the same secret. Required for sub-plan 1a's
    // `KeyStore::from_keyfile` path which deserializes from a
    // serialized buffer (e.g. read from disk at process boot).
    let passphrase = "persist-test-passphrase";
    let kf = KeyFile::new(passphrase);
    let bytes = kf.to_bytes();
    let kf2 = KeyFile::from_bytes(&bytes).expect("from_bytes");
    let secret1 = kf.decrypt(passphrase).expect("decrypt original");
    let secret2 = kf2.decrypt(passphrase).expect("decrypt round-tripped");
    assert_eq!(secret1, secret2, "persist round-trip must preserve secret");
}