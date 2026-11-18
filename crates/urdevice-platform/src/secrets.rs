//! Desktop secure storage for the core's [`SecretStore`] (R1.2).
//!
//! Secrets go to the OS keystore through `keyring`: Keychain on macOS,
//! Credential Manager (DPAPI) on Windows, and Secret Service on Linux. When
//! the keystore is not available (for example a Linux session without a
//! Secret Service), they go to an encrypted file with 0600 permissions in the
//! app data directory. The file key is derived from the machine ID and the
//! user name, so the file is useless when copied to another machine, but it
//! does not protect against other processes of the same user. The threat
//! model (`docs/security/threat-model.md`) records this.

use std::path::{Path, PathBuf};

use ring::aead::{self, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use urdevice_core::secret::SecretStore;
use urdevice_core::{CoreError, Result};

const SERVICE: &str = "UrDevice";

/// Keystore first, encrypted file as the fallback.
#[derive(Debug)]
pub struct DesktopSecretStore {
    dir: PathBuf,
    use_keyring: bool,
}

impl DesktopSecretStore {
    /// Store secrets for this user. `dir` holds the fallback files.
    pub fn new(dir: &Path) -> Self {
        let use_keyring = std::env::var_os("URDEVICE_NO_KEYRING").is_none();
        Self {
            dir: dir.join("secrets"),
            use_keyring,
        }
    }

    fn entry(name: &str) -> Option<keyring::Entry> {
        keyring::Entry::new(SERVICE, name).ok()
    }

    fn file(&self, name: &str) -> PathBuf {
        let safe: String = name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        self.dir.join(format!("{safe}.bin"))
    }

    fn file_key() -> Result<LessSafeKey> {
        let machine = machine_id();
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_default();
        let digest = urdevice_core::crypto::sha256(
            format!("urdevice-secret-file-v1\0{machine}\0{user}").as_bytes(),
        );
        let key = UnboundKey::new(&aead::CHACHA20_POLY1305, &digest)
            .map_err(|_| CoreError::Secret("file key".into()))?;
        Ok(LessSafeKey::new(key))
    }

    fn read_file(&self, name: &str) -> Result<Option<Vec<u8>>> {
        let path = self.file(name);
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        if data.len() < 12 {
            return Err(CoreError::Secret("secret file is too short".into()));
        }
        let (nonce, sealed) = data.split_at(12);
        let nonce = Nonce::try_assume_unique_for_key(nonce)
            .map_err(|_| CoreError::Secret("nonce".into()))?;
        let mut buf = sealed.to_vec();
        let plain = Self::file_key()?
            .open_in_place(nonce, Aad::from(name.as_bytes()), &mut buf)
            .map_err(|_| {
                CoreError::Secret("secret file can't be decrypted on this machine".into())
            })?;
        Ok(Some(plain.to_vec()))
    }

    fn write_file(&self, name: &str, value: &[u8]) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        restrict(&self.dir, 0o700);
        let mut nonce = [0u8; 12];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| CoreError::Secret("random".into()))?;
        let mut buf = value.to_vec();
        Self::file_key()?
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(name.as_bytes()),
                &mut buf,
            )
            .map_err(|_| CoreError::Secret("seal".into()))?;
        let mut out = nonce.to_vec();
        out.extend_from_slice(&buf);
        let path = self.file(name);
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &out)?;
        restrict(&tmp, 0o600);
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}

impl SecretStore for DesktopSecretStore {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>> {
        if self.use_keyring
            && let Some(e) = Self::entry(name)
        {
            match e.get_secret() {
                Ok(v) => return Ok(Some(v)),
                Err(keyring::Error::NoEntry) => {}
                Err(err) => tracing::debug!(error = %err, "keystore read failed; trying the file"),
            }
        }
        self.read_file(name)
    }

    fn set(&self, name: &str, value: &[u8]) -> Result<()> {
        if self.use_keyring
            && let Some(e) = Self::entry(name)
        {
            match e.set_secret(value) {
                Ok(()) => {
                    // Verify the write, then drop any older fallback copy.
                    if e.get_secret().ok().as_deref() == Some(value) {
                        let _ = std::fs::remove_file(self.file(name));
                        return Ok(());
                    }
                }
                Err(err) => {
                    tracing::warn!(error = %err, "keystore is unavailable; using the encrypted file");
                }
            }
        }
        self.write_file(name, value)
    }

    fn delete(&self, name: &str) -> Result<()> {
        if self.use_keyring
            && let Some(e) = Self::entry(name)
        {
            let _ = e.delete_credential();
        }
        match std::fs::remove_file(self.file(name)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

fn machine_id() -> String {
    for p in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
        if let Ok(s) = std::fs::read_to_string(p) {
            let s = s.trim();
            if !s.is_empty() {
                return s.to_owned();
            }
        }
    }
    gethostname::gethostname().to_string_lossy().into_owned()
}

#[cfg(unix)]
fn restrict(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn restrict(_path: &Path, _mode: u32) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_fallback_round_trip() {
        let dir = std::env::temp_dir().join(format!("urdevice-secrets-{}", std::process::id()));
        let s = DesktopSecretStore {
            dir: dir.join("secrets"),
            use_keyring: false,
        };
        assert_eq!(s.get("identity-key").unwrap(), None);
        s.set("identity-key", b"secret bytes").unwrap();
        assert_eq!(
            s.get("identity-key").unwrap().as_deref(),
            Some(&b"secret bytes"[..])
        );
        let raw = std::fs::read(s.file("identity-key")).unwrap();
        assert!(!raw.windows(6).any(|w| w == b"secret"));
        s.delete("identity-key").unwrap();
        assert_eq!(s.get("identity-key").unwrap(), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
