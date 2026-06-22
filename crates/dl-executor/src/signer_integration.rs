//! Signer integration: bridge `dl_signer::KeyStore` to solana-sdk's
//! `Keypair` so we can sign `VersionedTransaction`s for live submission.
//!
//! `dl_signer` is intentionally dep-light (only aes-gcm, argon2, ed25519-dalek).
//! The solana-sdk dependency lives here in `dl-executor`. Keeping the
//! split means `dl-signer` stays fast to compile and easy to audit —
//! it's the one crate in the workspace that touches raw key bytes.

use solana_sdk::hash::Hash;
use solana_sdk::signature::SeedDerivable;
use solana_sdk::signature::Signature;
use solana_sdk::signer::keypair::Keypair;
use solana_sdk::signer::Signer as SdkSigner;
use solana_sdk::transaction::VersionedTransaction;

use dl_signer::keystore::KeyStore;

use crate::error::ExecutorError;

/// Build a `solana_sdk::Keypair` from the `dl_signer::KeyStore`
/// secret bytes. The Keypair holds a copy; zeroizing the Keypair
/// after use is the caller's responsibility.
pub fn keystore_to_keypair(keystore: &KeyStore) -> Result<Keypair, ExecutorError> {
    let secret: [u8; 32] = *keystore.secret();
    // `Keypair::from_seed` accepts a 32-byte ed25519 seed and
    // derives the pubkey internally. This is the standard way to
    // construct a Keypair from a secret-key-only input.
    Keypair::from_seed(&secret).map_err(|e| {
        ExecutorError::Signer(format!(
            "could not derive Keypair from dl_signer secret: {e}"
        ))
    })
}

/// Sign every transaction in the slice with the given keypair and
/// blockhash. Sets the recent blockhash on each message before
/// signing. Required for a tx to be accepted by the validator.
///
/// `txs` is mutated in place: each tx's signatures are replaced
/// with a single signature from `keypair`.
pub fn sign_transactions(
    keypair: &Keypair,
    txs: &mut [VersionedTransaction],
    recent_blockhash: Hash,
) -> Result<(), ExecutorError> {
    for tx in txs.iter_mut() {
        // Set the blockhash on the underlying message. For Legacy
        // messages this mutates Message.recent_blockhash; for v0
        // messages it mutates VersionedMessage::V0.message.
        match &mut tx.message {
            solana_sdk::message::VersionedMessage::Legacy(m) => {
                m.recent_blockhash = recent_blockhash;
            }
            solana_sdk::message::VersionedMessage::V0(m) => {
                m.recent_blockhash = recent_blockhash;
            }
        }
        let sig: Signature = keypair.sign_message(&tx.message.serialize());
        // Replace signatures with a single-signer array. The
        // signer's position in `tx.message.account_keys` is index 0
        // for the fee-payer (which `keypair` is for our bundles).
        tx.signatures = vec![sig];
    }
    Ok(())
}

/// Convenience: keystore → keypair → sign all txs in one call.
pub fn sign_with_keystore(
    keystore: &KeyStore,
    txs: &mut [VersionedTransaction],
    recent_blockhash: Hash,
) -> Result<(), ExecutorError> {
    let keypair = keystore_to_keypair(keystore)?;
    sign_transactions(&keypair, txs, recent_blockhash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::message::Message;
    use solana_sdk::pubkey::Pubkey;
    use solana_sdk::system_instruction;

    fn fresh_keystore() -> KeyStore {
        let kf = dl_signer::keystore::KeyFile::new("test-passphrase-1a2");
        let secret = kf.decrypt("test-passphrase-1a2").unwrap();
        KeyStore::from_secret(secret)
    }

    fn make_tx(keypair: &Keypair) -> VersionedTransaction {
        let fee_payer = keypair.pubkey();
        let ix = system_instruction::transfer(&fee_payer, &Pubkey::new_unique(), 0);
        let msg = Message::new(&[ix], Some(&fee_payer));
        VersionedTransaction::try_new(
            solana_sdk::message::VersionedMessage::Legacy(msg),
            &[keypair],
        )
        .unwrap()
    }

    #[test]
    fn keystore_to_keypair_matches_dl_signer_pubkey() {
        let ks = fresh_keystore();
        let kp = keystore_to_keypair(&ks).unwrap();
        let kp_bytes: [u8; 32] = kp.pubkey().to_bytes();
        let ks_bytes: [u8; 32] = ks.public_key_for_print();
        assert_eq!(kp_bytes, ks_bytes);
    }

    #[test]
    fn sign_transactions_overwrites_signature() {
        let ks = fresh_keystore();
        let kp = keystore_to_keypair(&ks).unwrap();
        let pubkey = kp.pubkey();

        let mut tx = make_tx(&kp);
        let pre_sig = tx.signatures[0];

        let blockhash = Hash::new_unique();
        sign_transactions(&kp, std::slice::from_mut(&mut tx), blockhash).unwrap();

        let post_sig = tx.signatures[0];
        assert_ne!(pre_sig, post_sig, "signature must change after sign");

        // Verify the signature with the pubkey.
        let msg = tx.message.serialize();
        let valid = post_sig.verify(&pubkey.to_bytes(), &msg);
        assert!(valid, "signature must verify against the signer's pubkey");

        // The blockhash must be the one we set.
        match tx.message {
            solana_sdk::message::VersionedMessage::Legacy(m) => {
                assert_eq!(m.recent_blockhash, blockhash);
            }
            _ => panic!("expected Legacy message"),
        }
    }

    #[test]
    fn sign_with_keystore_helper() {
        let ks = fresh_keystore();
        let kp = keystore_to_keypair(&ks).unwrap();
        let mut tx = make_tx(&kp);
        sign_with_keystore(&ks, std::slice::from_mut(&mut tx), Hash::new_unique())
            .unwrap();
        assert_eq!(tx.signatures.len(), 1);
    }
}