//! Device identity (R1, design §4.1).
//!
//! Each device has one Ed25519 key, created on first start and kept in the
//! [`SecretStore`]. The self-signed certificate is rebuilt from the key on every
//! start: peers pin the key (the Device ID), not the certificate.

use std::fmt;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

use crate::crypto::sha256;
use crate::secret::{IDENTITY_KEY, SecretStore};
use crate::{CoreError, Result};

/// SHA-256 of a device's SubjectPublicKeyInfo.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DeviceId(pub [u8; 32]);

impl DeviceId {
    /// Build from a slice, which must be exactly 32 bytes.
    pub fn from_slice(b: &[u8]) -> Option<Self> {
        <[u8; 32]>::try_from(b).ok().map(Self)
    }

    /// Parse the lower-case hex form.
    pub fn from_hex(s: &str) -> Option<Self> {
        let b = data_encoding::HEXLOWER_PERMISSIVE
            .decode(s.as_bytes())
            .ok()?;
        Self::from_slice(&b)
    }

    /// First 8 bytes, used by discovery (design §5).
    pub fn short(&self) -> [u8; 8] {
        let mut s = [0; 8];
        s.copy_from_slice(&self.0[..8]);
        s
    }

    /// Lower-case hex of all 32 bytes. Used as a stable key in the UI and FFI.
    pub fn hex(&self) -> String {
        data_encoding::HEXLOWER.encode(&self.0)
    }

    /// Lower-case hex of the short ID, for logs.
    pub fn short_hex(&self) -> String {
        data_encoding::HEXLOWER.encode(&self.short())
    }

    /// Readable fingerprint: base32 of the first 20 bytes as 8 groups of 4 (R1.4).
    pub fn fingerprint(&self) -> String {
        let b32 = data_encoding::BASE32_NOPAD.encode(&self.0[..20]);
        b32.as_bytes()
            .chunks(4)
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect::<Vec<_>>()
            .join("-")
    }

    /// The Device ID of the certificate's key.
    pub fn of_cert(cert: &CertificateDer<'_>) -> Result<Self> {
        let ee = webpki::EndEntityCert::try_from(cert)
            .map_err(|e| CoreError::Crypto(format!("bad certificate: {e}")))?;
        Ok(Self(sha256(ee.subject_public_key_info().as_ref())))
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceId({})", self.short_hex())
    }
}

/// The local device's key, certificate, and Device ID.
pub struct Identity {
    key_pkcs8: Vec<u8>,
    cert: CertificateDer<'static>,
    id: DeviceId,
}

impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never print the key.
        f.debug_struct("Identity")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Identity {
    /// Load the identity from secure storage, or create one. The flag is true
    /// when a new key was created.
    pub fn load_or_create(store: &dyn SecretStore) -> Result<(Self, bool)> {
        if let Some(key) = store.get(IDENTITY_KEY)? {
            match Self::from_pkcs8(key) {
                Ok(id) => return Ok((id, false)),
                Err(e) => {
                    tracing::warn!(error = %e, "stored identity key is unusable; creating a new one")
                }
            }
        }
        let id = Self::generate()?;
        store.set(IDENTITY_KEY, &id.key_pkcs8)?;
        Ok((id, true))
    }

    /// Create a new key, replacing the stored one (R1.5).
    pub fn reset(store: &dyn SecretStore) -> Result<Self> {
        let id = Self::generate()?;
        store.set(IDENTITY_KEY, &id.key_pkcs8)?;
        Ok(id)
    }

    /// A fresh identity that is not stored anywhere.
    pub fn generate() -> Result<Self> {
        let kp = rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519)?;
        Self::from_pkcs8(kp.serialize_der())
    }

    fn from_pkcs8(key_pkcs8: Vec<u8>) -> Result<Self> {
        let der = PrivatePkcs8KeyDer::from(key_pkcs8.as_slice());
        let kp = rcgen::KeyPair::from_pkcs8_der_and_sign_algo(&der, &rcgen::PKCS_ED25519)?;
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new())?;
        let mut dn = rcgen::DistinguishedName::new();
        dn.push(rcgen::DnType::CommonName, "UrDevice");
        params.distinguished_name = dn;
        params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        params.not_after = rcgen::date_time_ymd(2046, 1, 1);
        let cert = params.self_signed(&kp)?;
        let cert = CertificateDer::from(cert.der().to_vec());
        let id = DeviceId::of_cert(&cert)?;
        Ok(Self {
            key_pkcs8,
            cert,
            id,
        })
    }

    /// The Device ID.
    pub fn id(&self) -> DeviceId {
        self.id
    }

    /// The certificate chain for TLS.
    pub fn cert_chain(&self) -> Vec<CertificateDer<'static>> {
        vec![self.cert.clone()]
    }

    /// The private key for TLS. Stays in memory; never logged or sent.
    pub fn private_key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key_pkcs8.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret::MemorySecretStore;

    #[test]
    fn identity_is_stable_across_loads() {
        let store = MemorySecretStore::new();
        let (a, created) = Identity::load_or_create(&store).unwrap();
        assert!(created);
        let (b, created) = Identity::load_or_create(&store).unwrap();
        assert!(!created);
        assert_eq!(a.id(), b.id());
        // The certificate is rebuilt, but it still carries the same key.
        assert_eq!(DeviceId::of_cert(&b.cert_chain()[0]).unwrap(), a.id());
    }

    #[test]
    fn reset_creates_a_new_id() {
        let store = MemorySecretStore::new();
        let (a, _) = Identity::load_or_create(&store).unwrap();
        let b = Identity::reset(&store).unwrap();
        assert_ne!(a.id(), b.id());
        let (c, _) = Identity::load_or_create(&store).unwrap();
        assert_eq!(b.id(), c.id());
    }

    #[test]
    fn fingerprint_has_eight_groups_of_four() {
        let id = Identity::generate().unwrap().id();
        let fp = id.fingerprint();
        let groups: Vec<_> = fp.split('-').collect();
        assert_eq!(groups.len(), 8);
        assert!(groups.iter().all(|g| g.len() == 4));
        assert_eq!(DeviceId::from_hex(&id.hex()), Some(id));
    }

    #[test]
    fn debug_does_not_print_the_key() {
        let id = Identity::generate().unwrap();
        let text = format!("{id:?}");
        assert!(!text.contains("key_pkcs8"));
    }
}
