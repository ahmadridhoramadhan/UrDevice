//! Small wrappers over `ring` used by pairing and the trust store.

use ring::rand::{SecureRandom, SystemRandom};
use ring::{digest, hmac};

use crate::{CoreError, Result};

/// `len` bytes from the OS CSPRNG.
pub fn random_bytes(len: usize) -> Result<Vec<u8>> {
    let mut out = vec![0; len];
    SystemRandom::new()
        .fill(&mut out)
        .map_err(|_| CoreError::Crypto("random generator failed".into()))?;
    Ok(out)
}

/// A random u64 from the OS CSPRNG.
pub fn random_u64() -> Result<u64> {
    let b = random_bytes(8)?;
    let mut a = [0; 8];
    a.copy_from_slice(&b);
    Ok(u64::from_le_bytes(a))
}

/// SHA-256 of `data`.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let d = digest::digest(&digest::SHA256, data);
    let mut out = [0; 32];
    out.copy_from_slice(d.as_ref());
    out
}

/// HMAC-SHA256 over the concatenation of `parts`.
pub fn hmac_sha256(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let key = hmac::Key::new(hmac::HMAC_SHA256, key);
    let mut ctx = hmac::Context::with_key(&key);
    for p in parts {
        ctx.update(p);
    }
    let tag = ctx.sign();
    let mut out = [0; 32];
    out.copy_from_slice(tag.as_ref());
    out
}

/// Check an HMAC-SHA256 tag in constant time.
pub fn hmac_verify(key: &[u8], parts: &[&[u8]], tag: &[u8]) -> bool {
    let key = hmac::Key::new(hmac::HMAC_SHA256, key);
    let msg: Vec<u8> = parts.concat();
    hmac::verify(&key, &msg, tag).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_round_trip() {
        let tag = hmac_sha256(b"k", &[b"a", b"b"]);
        assert!(hmac_verify(b"k", &[b"ab"], &tag));
        assert!(!hmac_verify(b"k2", &[b"ab"], &tag));
        assert!(!hmac_verify(b"k", &[b"ab"], &tag[..31]));
    }

    #[test]
    fn random_is_not_constant() {
        assert_ne!(random_bytes(16).unwrap(), random_bytes(16).unwrap());
    }
}
