//! Noise IK for the BLE channel (design §8.2, R6.3).
//!
//! `Noise_IK_25519_ChaChaPoly_SHA256` with the BLE static keys: each device
//! derives an X25519 key from its identity key, and peers pin each other's
//! public half over the authenticated QUIC session (design §4.1). The desktop
//! (GATT central) is the initiator and knows the phone's key in advance; the
//! phone learns the desktop's key from the first message and accepts it only
//! if it is pinned for a paired device. There is no unencrypted mode: every
//! byte after the handshake is a Noise transport message.

use ring::hkdf;
use snow::resolvers::{CryptoResolver, DefaultResolver};
use snow::{HandshakeState, TransportState};

use crate::{CoreError, Result};

/// The Noise protocol name.
pub const PATTERN: &str = "Noise_IK_25519_ChaChaPoly_SHA256";
/// Bound into the handshake hash, so a key can't be reused for anything else.
pub const PROLOGUE: &[u8] = b"urdevice-ble-v1";
/// X25519 key length.
pub const KEY_LEN: usize = 32;
/// Authentication tag added to every message.
pub const TAG: usize = 16;
/// Largest Noise message.
pub const MAX_MESSAGE: usize = 65_535;
/// Largest plaintext in one transport message.
pub const MAX_PLAINTEXT: usize = MAX_MESSAGE - TAG;

const DERIVE_SALT: &[u8] = b"urdevice-ble-static-v1";

fn crypto(e: snow::Error) -> CoreError {
    CoreError::Crypto(format!("noise: {e}"))
}

fn params() -> Result<snow::params::NoiseParams> {
    PATTERN.parse().map_err(crypto)
}

/// A device's BLE static key pair.
#[derive(Clone)]
pub struct StaticKey {
    secret: [u8; KEY_LEN],
    public: [u8; KEY_LEN],
}

impl std::fmt::Debug for StaticKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the secret half.
        f.debug_struct("StaticKey").finish_non_exhaustive()
    }
}

impl StaticKey {
    /// Derive the key pair from the identity key material, so it changes
    /// whenever the identity is reset (R1.5) and needs no storage of its own.
    pub fn derive(identity_secret: &[u8]) -> Result<Self> {
        let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, DERIVE_SALT).extract(identity_secret);
        let mut secret = [0u8; KEY_LEN];
        prk.expand(&[b"x25519"], hkdf::HKDF_SHA256)
            .and_then(|okm| okm.fill(&mut secret))
            .map_err(|_| CoreError::Crypto("hkdf".into()))?;
        Self::from_secret(secret)
    }

    /// Build from a secret key.
    pub fn from_secret(secret: [u8; KEY_LEN]) -> Result<Self> {
        let mut dh = DefaultResolver
            .resolve_dh(&snow::params::DHChoice::Curve25519)
            .ok_or_else(|| CoreError::Crypto("no x25519".into()))?;
        dh.set(&secret);
        let public = <[u8; KEY_LEN]>::try_from(dh.pubkey())
            .map_err(|_| CoreError::Crypto("bad x25519 key".into()))?;
        Ok(Self { secret, public })
    }

    /// The public half, which peers pin.
    pub fn public(&self) -> [u8; KEY_LEN] {
        self.public
    }
}

/// An encrypted, authenticated channel after the handshake.
pub struct Transport(TransportState);

impl std::fmt::Debug for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transport").finish_non_exhaustive()
    }
}

impl Transport {
    /// Encrypt one message of at most [`MAX_PLAINTEXT`] bytes.
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        if plaintext.len() > MAX_PLAINTEXT {
            return Err(CoreError::InvalidInput("noise message too large".into()));
        }
        let mut out = vec![0u8; plaintext.len() + TAG];
        let n = self.0.write_message(plaintext, &mut out).map_err(crypto)?;
        out.truncate(n);
        Ok(out)
    }

    /// Decrypt and authenticate one message. Any failure is fatal for the
    /// channel: the nonce sequence is lost.
    pub fn decrypt(&mut self, message: &[u8]) -> Result<Vec<u8>> {
        if message.len() < TAG || message.len() > MAX_MESSAGE {
            return Err(CoreError::Protocol("bad noise message length".into()));
        }
        let mut out = vec![0u8; message.len()];
        let n = self.0.read_message(message, &mut out).map_err(crypto)?;
        out.truncate(n);
        Ok(out)
    }
}

/// The central's side: it knows the peripheral's pinned key.
pub struct Initiator(HandshakeState);

impl Initiator {
    /// Start a handshake with the peer whose pinned BLE key is `remote`.
    /// Returns the first message to send.
    pub fn start(local: &StaticKey, remote: &[u8; KEY_LEN]) -> Result<(Self, Vec<u8>)> {
        let mut hs = snow::Builder::new(params()?)
            .local_private_key(&local.secret)
            .and_then(|b| b.remote_public_key(remote))
            .and_then(|b| b.prologue(PROLOGUE))
            .and_then(|b| b.build_initiator())
            .map_err(crypto)?;
        let mut msg = vec![0u8; 256];
        let n = hs.write_message(&[], &mut msg).map_err(crypto)?;
        msg.truncate(n);
        Ok((Self(hs), msg))
    }

    /// Read the peripheral's answer and switch to transport mode.
    pub fn finish(mut self, msg: &[u8]) -> Result<Transport> {
        if msg.len() > 256 {
            return Err(CoreError::Protocol("bad noise handshake".into()));
        }
        let mut payload = vec![0u8; 256];
        self.0.read_message(msg, &mut payload).map_err(crypto)?;
        Ok(Transport(self.0.into_transport_mode().map_err(crypto)?))
    }
}

/// The peripheral's side, after the first message was read.
pub struct Responder(HandshakeState);

impl Responder {
    /// Read the central's first message. Returns the handshake and the
    /// central's static key; check that key against the pinned keys before
    /// calling [`Responder::accept`].
    pub fn read(local: &StaticKey, msg: &[u8]) -> Result<(Self, [u8; KEY_LEN])> {
        if msg.len() > 256 {
            return Err(CoreError::Protocol("bad noise handshake".into()));
        }
        let mut hs = snow::Builder::new(params()?)
            .local_private_key(&local.secret)
            .and_then(|b| b.prologue(PROLOGUE))
            .and_then(|b| b.build_responder())
            .map_err(crypto)?;
        let mut payload = vec![0u8; 256];
        hs.read_message(msg, &mut payload).map_err(crypto)?;
        let remote = hs
            .get_remote_static()
            .and_then(|k| <[u8; KEY_LEN]>::try_from(k).ok())
            .ok_or_else(|| CoreError::Protocol("no remote static key".into()))?;
        Ok((Self(hs), remote))
    }

    /// Answer and switch to transport mode.
    pub fn accept(mut self) -> Result<(Vec<u8>, Transport)> {
        let mut msg = vec![0u8; 256];
        let n = self.0.write_message(&[], &mut msg).map_err(crypto)?;
        msg.truncate(n);
        Ok((
            msg,
            Transport(self.0.into_transport_mode().map_err(crypto)?),
        ))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn pair() -> (Transport, Transport) {
        let a = StaticKey::derive(b"desktop identity").unwrap();
        let b = StaticKey::derive(b"phone identity").unwrap();
        let (init, m1) = Initiator::start(&a, &b.public()).unwrap();
        let (resp, seen) = Responder::read(&b, &m1).unwrap();
        assert_eq!(seen, a.public());
        let (m2, tb) = resp.accept().unwrap();
        (init.finish(&m2).unwrap(), tb)
    }

    #[test]
    fn derived_keys_are_stable_and_distinct() {
        let a1 = StaticKey::derive(b"one").unwrap();
        let a2 = StaticKey::derive(b"one").unwrap();
        let b = StaticKey::derive(b"two").unwrap();
        assert_eq!(a1.public(), a2.public());
        assert_ne!(a1.public(), b.public());
        assert!(!format!("{a1:?}").contains(&format!("{:?}", a1.secret)));
    }

    #[test]
    fn handshake_and_both_directions() {
        let (mut a, mut b) = pair();
        let c = a.encrypt(b"hello phone").unwrap();
        assert!(!c.windows(5).any(|w| w == b"hello"));
        assert_eq!(b.decrypt(&c).unwrap(), b"hello phone");
        let c = b.encrypt(b"hello desktop").unwrap();
        assert_eq!(a.decrypt(&c).unwrap(), b"hello desktop");
        let big = vec![7u8; MAX_PLAINTEXT];
        let c = a.encrypt(&big).unwrap();
        assert_eq!(c.len(), MAX_MESSAGE);
        assert_eq!(b.decrypt(&c).unwrap(), big);
        assert!(a.encrypt(&[0; MAX_PLAINTEXT + 1]).is_err());
    }

    #[test]
    fn wrong_responder_key_fails() {
        let a = StaticKey::derive(b"desktop").unwrap();
        let b = StaticKey::derive(b"phone").unwrap();
        let other = StaticKey::derive(b"someone else").unwrap();
        let (_, m1) = Initiator::start(&a, &other.public()).unwrap();
        assert!(Responder::read(&b, &m1).is_err());
    }

    #[test]
    fn tampered_or_replayed_messages_fail() {
        let (mut a, mut b) = pair();
        let mut c = a.encrypt(b"input").unwrap();
        c[0] ^= 1;
        assert!(b.decrypt(&c).is_err());

        let (mut a, mut b) = pair();
        let c = a.encrypt(b"once").unwrap();
        b.decrypt(&c).unwrap();
        assert!(b.decrypt(&c).is_err());
        assert!(b.decrypt(&[0; 3]).is_err());
    }

    #[test]
    fn garbage_handshake_is_refused() {
        let b = StaticKey::derive(b"phone").unwrap();
        assert!(Responder::read(&b, &[0; 10]).is_err());
        assert!(Responder::read(&b, &[0; 300]).is_err());
        let a = StaticKey::derive(b"desktop").unwrap();
        let (init, _) = Initiator::start(&a, &b.public()).unwrap();
        assert!(init.finish(&[1; 48]).is_err());
    }
}
