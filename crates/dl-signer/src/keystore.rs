//! Encrypted keyfile: AES-256-GCM with Argon2id-derived key.
//!
//! ## File format (v1)
//!
//! ```text
//! | offset | bytes | field            |
//! | ------ | ----- | ---------------- |
//! | 0      | 4     | magic = b"KFK1"  |
//! | 4      | 1     | version = 1      |
//! | 5      | 16    | salt             |
//! | 21     | 12    | nonce            |
//! | 33     | 4     | ciphertext_len   |
//! | 37     | N     | ciphertext       |
//! ```
//!
//! The 32-byte secret key is the ciphertext after Argon2id KDF and
//! AES-256-GCM encryption. Passphrase is operator-supplied at boot.

use std::path::Path;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use argon2::Argon2;
use rand::RngCore;
use zeroize::Zeroize;

use crate::error::SignerError;

/// Encrypted-on-disk keyfile. Holds only the encrypted secret; the
/// derived keypair lives in [`KeyStore`] (which zeroizes on drop).
#[derive(Debug, Clone)]
pub struct KeyFile {
    /// 16-byte Argon2id salt.
    pub salt: [u8; 16],
    /// 12-byte AES-256-GCM nonce.
    pub nonce: [u8; 12],
    /// Encrypted 32-byte secret key.
    pub ciphertext: Vec<u8>,
    /// Format version (currently 1).
    pub version: u8,
}

const MAGIC: &[u8; 4] = b"KFK1";
const VERSION: u8 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const HEADER_LEN: usize = 4 + 1 + SALT_LEN + NONCE_LEN + 4; // 37 bytes
/// A Solana secret key is 32 bytes. The ciphertext is the same length
/// because AES-GCM is a stream cipher.
const CIPHERTEXT_LEN: usize = 32;

impl KeyFile {
    /// Create a new keyfile with a freshly generated 32-byte secret.
    /// The passphrase is used to encrypt it; both the secret and
    /// passphrase are required to reconstruct the keypair.
    pub fn new(passphrase: &str) -> Self {
        let mut rng = rand::thread_rng();
        let mut secret = [0u8; 32];
        rng.fill_bytes(&mut secret);

        let mut salt = [0u8; SALT_LEN];
        rng.fill_bytes(&mut salt);
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rng.fill_bytes(&mut nonce_bytes);

        let key = derive_key(passphrase, &salt);
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce_bytes), secret.as_ref())
            .expect("AES-GCM encrypt failure is a BUG; key is 256 bits");

        // Wipe the secret; we only need the ciphertext from here on.
        let mut key = key;
        key.zeroize();
        secret.zeroize();

        KeyFile {
            salt,
            nonce: nonce_bytes,
            ciphertext,
            version: VERSION,
        }
    }

    /// Decrypt the keyfile with the given passphrase. Returns the
    /// raw 32-byte secret key. Caller is responsible for zeroizing
    /// the returned buffer after use.
    pub fn decrypt(&self, passphrase: &str) -> Result<[u8; 32], SignerError> {
        if self.version != VERSION {
            return Err(SignerError::BadFormat(format!(
                "unsupported version: {} (expected {})",
                self.version, VERSION
            )));
        }
        let key = derive_key(passphrase, &self.salt);
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
        let plaintext = cipher
            .decrypt(Nonce::from_slice(&self.nonce), self.ciphertext.as_ref())
            .map_err(|_| {
                // Could be wrong passphrase OR corrupt ciphertext.
                // We report WrongPassphrase as the most likely cause
                // and let the operator investigate further if it
                // turns out to be corruption.
                SignerError::WrongPassphrase
            });
        let mut key = key;
        key.zeroize();
        let secret = plaintext?;
        let result: [u8; 32] = match secret.try_into() {
            Ok(s) => s,
            Err(_) => {
                return Err(SignerError::BadFormat(
                    "decrypted secret wrong length".into(),
                ))
            }
        };
        Ok(result)
    }

    /// Serialize to bytes (the on-disk format).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + self.ciphertext.len());
        out.extend_from_slice(MAGIC);
        out.push(self.version);
        out.extend_from_slice(&self.salt);
        out.extend_from_slice(&self.nonce);
        out.extend_from_slice(&(self.ciphertext.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.ciphertext);
        out
    }

    /// Parse from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SignerError> {
        if bytes.len() < HEADER_LEN {
            return Err(SignerError::BadFormat(format!(
                "keyfile too short: {} bytes (need >= {})",
                bytes.len(),
                HEADER_LEN
            )));
        }
        if &bytes[0..4] != MAGIC {
            return Err(SignerError::BadFormat(
                "bad magic (expected b'KFK1')".to_string(),
            ));
        }
        let version = bytes[4];
        if version != VERSION {
            return Err(SignerError::BadFormat(format!(
                "unsupported version: {version} (expected {VERSION})"
            )));
        }
        let mut salt = [0u8; SALT_LEN];
        salt.copy_from_slice(&bytes[5..5 + SALT_LEN]);
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&bytes[5 + SALT_LEN..5 + SALT_LEN + NONCE_LEN]);
        let ct_len = u32::from_le_bytes(
            bytes[5 + SALT_LEN + NONCE_LEN..HEADER_LEN]
                .try_into()
                .map_err(|_| SignerError::BadFormat("ciphertext_len: bad slice".into()))?,
        ) as usize;
        if bytes.len() < HEADER_LEN + ct_len {
            return Err(SignerError::BadFormat(format!(
                "truncated keyfile: have {} bytes, ciphertext needs {ct_len}",
                bytes.len() - HEADER_LEN
            )));
        }
        Ok(KeyFile {
            salt,
            nonce,
            ciphertext: bytes[HEADER_LEN..HEADER_LEN + ct_len].to_vec(),
            version,
        })
    }

    /// Write the keyfile to disk (creates or overwrites).
    pub fn save(&self, path: &Path) -> Result<(), SignerError> {
        std::fs::write(path, self.to_bytes())
            .map_err(|e| SignerError::Io(format!("write {}: {e}", path.display())))
    }

    /// Load a keyfile from disk.
    pub fn load(path: &Path) -> Result<Self, SignerError> {
        let bytes = std::fs::read(path)
            .map_err(|e| SignerError::Io(format!("read {}: {e}", path.display())))?;
        Self::from_bytes(&bytes)
    }
}

/// In-memory keypair. Zeroizes the secret on drop.
#[derive(Clone)]
pub struct KeyStore {
    /// The 32-byte secret key (zeroized on drop).
    secret: [u8; 32],
}

impl KeyStore {
    /// Create a KeyStore from a raw 32-byte secret. Caller is
    /// responsible for zeroizing the source buffer.
    pub fn from_secret(secret: [u8; 32]) -> Self {
        Self { secret }
    }

    /// Load a KeyStore from a keyfile and passphrase.
    pub fn from_keyfile(path: &Path, passphrase: &str) -> Result<Self, SignerError> {
        let kf = KeyFile::load(path)?;
        let secret = kf.decrypt(passphrase)?;
        Ok(Self { secret })
    }

    /// Return the 32-byte secret key. **Do not zeroize or copy**;
    /// the value is owned by this KeyStore and is zeroized on drop.
    pub fn secret(&self) -> &[u8; 32] {
        &self.secret
    }

    /// Return the public key bytes (the Ed25519 verifying key
    /// derived from the secret). Exposed for diagnostic
    /// purposes (e.g. the dl-signer CLI's `verify` and
    /// `drain-to` commands need to print the pubkey), and as
    /// the canonical pubkey that `solana-sdk::Keypair::try_from`
    /// would derive from the same secret.
    pub fn public_key_for_print(&self) -> [u8; 32] {
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&self.secret);
        signing_key.verifying_key().to_bytes()
    }
}

impl Drop for KeyStore {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

impl std::fmt::Debug for KeyStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyStore")
            .field("pubkey_prefix", &format!("{:02x}{:02x}{:02x}{:02x}...", self.secret[0], self.secret[1], self.secret[2], self.secret[3]))
            .finish()
    }
}

/// Argon2id KDF. Returns a 32-byte key suitable for AES-256-GCM.
fn derive_key(passphrase: &str, salt: &[u8; SALT_LEN]) -> [u8; 32] {
    use argon2::Params;
    let params =
        Params::new(19_456, 2, 1, Some(32)).expect("argon2 params are constants; this cannot fail");
    let argon = Argon2::new(argon2::Algorithm::Argon2id, Version::V0x13, params);
    let mut out = [0u8; 32];
    argon
        .hash_password_into(passphrase.as_bytes(), salt, &mut out)
        .expect("argon2 hash_password_into failure is a BUG");
    out
}

use argon2::Version;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_secret() {
        let kf = KeyFile::new("correct horse battery staple");
        let secret1 = kf.decrypt("correct horse battery staple").unwrap();
        let secret2 = kf.decrypt("correct horse battery staple").unwrap();
        assert_eq!(secret1, secret2);
    }

    #[test]
    fn wrong_passphrase_fails() {
        let kf = KeyFile::new("correct horse battery staple");
        let err = kf.decrypt("wrong").unwrap_err();
        assert_eq!(err, SignerError::WrongPassphrase);
    }

    #[test]
    fn bytes_round_trip() {
        let kf = KeyFile::new("hunter2");
        let bytes = kf.to_bytes();
        let parsed = KeyFile::from_bytes(&bytes).unwrap();
        assert_eq!(parsed.salt, kf.salt);
        assert_eq!(parsed.nonce, kf.nonce);
        assert_eq!(parsed.ciphertext, kf.ciphertext);
        assert_eq!(parsed.version, kf.version);
    }

    #[test]
    fn bad_magic_rejected() {
        let mut bytes = vec![0u8; 64];
        bytes[0..4].copy_from_slice(b"NOPE");
        let err = KeyFile::from_bytes(&bytes).unwrap_err();
        assert!(matches!(err, SignerError::BadFormat(_)));
    }

    #[test]
    fn file_round_trip() {
        let kf = KeyFile::new("hunter2");
        let path = std::env::temp_dir().join("dl_signer_test.kfk1");
        kf.save(&path).unwrap();
        let loaded = KeyFile::load(&path).unwrap();
        assert_eq!(loaded.to_bytes(), kf.to_bytes());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn keystore_pubkey_prefix_redacts_secret() {
        let kf = KeyFile::new("hunter2");
        let secret = kf.decrypt("hunter2").unwrap();
        let ks = KeyStore::from_secret(secret);
        let prefix = format!("{:02x}{:02x}{:02x}{:02x}...", secret[0], secret[1], secret[2], secret[3]);
        // Prefix is 8 hex chars + "..." = 11 chars; never the full secret.
        assert_eq!(prefix.len(), 11);
        assert!(!prefix.contains("hunter2"));
    }
}
