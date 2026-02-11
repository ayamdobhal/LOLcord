use anyhow::Result;
use argon2::Argon2;
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit},
};
use rand::RngCore;

/// Fixed salt for key derivation (not secret — just ensures consistent keys for same password+room).
const SALT: &[u8; 16] = b"voicechat_e2e_01";

/// Derive a 256-bit encryption key from a room password using Argon2id.
pub fn derive_key(password: &str) -> [u8; 32] {
    let mut key = [0u8; 32];
    Argon2::default()
        .hash_password_into(password.as_bytes(), SALT, &mut key)
        .expect("argon2 key derivation failed");
    key
}

/// Encrypt data using XChaCha20-Poly1305.
/// Returns: [24-byte nonce][ciphertext + 16-byte auth tag]
pub fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let mut nonce_bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = XNonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| anyhow::anyhow!("encryption failed: {e}"))?;

    let mut out = Vec::with_capacity(24 + ciphertext.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypt data. Input: [24-byte nonce][ciphertext + auth tag]
pub fn decrypt(key: &[u8; 32], data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 24 + 16 {
        anyhow::bail!("encrypted data too short");
    }
    let cipher = XChaCha20Poly1305::new(key.into());
    let nonce = XNonce::from_slice(&data[..24]);
    let ciphertext = &data[24..];

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| anyhow::anyhow!("decryption failed: {e}"))
}
