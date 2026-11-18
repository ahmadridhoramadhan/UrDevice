//! Pairing helpers (R3, design §7.1–§7.3).
//!
//! The network flows live in [`crate::node`]. This module holds the parts that
//! can be tested on their own: the QR invite format, the token proof, the
//! 6-digit code, the lockout counter, and the checks on peer-supplied text.

use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use urdevice_proto::{PROTO_VERSION, PairInvite};

use crate::crypto::{hmac_sha256, hmac_verify};
use crate::identity::DeviceId;
use crate::{CoreError, Result};

/// URI prefix of a pairing QR code.
pub const QR_PREFIX: &str = "urdevice://pair?d=";
/// Token lifetime (R3.2).
pub const TOKEN_TTL: Duration = Duration::from_secs(120);
/// Token length in bytes (R3.1: 128 bits).
pub const TOKEN_LEN: usize = 16;
/// TLS exporter label for the QR proof.
pub const EXPORTER_PAIR: &[u8] = b"EXPORTER-urdevice-pair";
/// TLS exporter label for the 6-digit code.
pub const EXPORTER_SAS: &[u8] = b"EXPORTER-urdevice-sas";
/// Largest QR text accepted.
pub const MAX_QR_LEN: usize = 2048;
/// Most addresses taken from a QR code.
pub const MAX_INVITE_ADDRS: usize = 8;

/// Failures allowed in [`LOCKOUT_WINDOW`] before pairing locks (R3.7).
pub const LOCKOUT_FAILURES: usize = 5;
/// Window for counting failures.
pub const LOCKOUT_WINDOW: Duration = Duration::from_secs(5 * 60);
/// How long pairing stays locked.
pub const LOCKOUT_DURATION: Duration = Duration::from_secs(5 * 60);

/// Build the QR text for an invite.
pub fn invite_uri(invite: &PairInvite) -> String {
    let bytes = urdevice_proto::encode_invite(invite);
    format!(
        "{QR_PREFIX}{}",
        data_encoding::BASE64URL_NOPAD.encode(&bytes)
    )
}

/// A parsed and checked invite.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScannedInvite {
    /// The host's Device ID.
    pub device_id: DeviceId,
    /// Host display name (cleaned).
    pub name: String,
    /// Host platform (cleaned).
    pub platform: String,
    /// Where to connect.
    pub addrs: Vec<SocketAddr>,
    /// The one-time token.
    pub token: [u8; TOKEN_LEN],
    /// Expiry, Unix seconds.
    pub expires_at: u64,
}

/// Parse QR text. Every field is untrusted (R3.3).
pub fn parse_invite_uri(text: &str, now_unix: u64) -> Result<ScannedInvite> {
    let bad = |why: &str| CoreError::InvalidInput(format!("not a UrDevice pairing code ({why})"));
    if text.len() > MAX_QR_LEN {
        return Err(bad("too long"));
    }
    let b64 = text
        .trim()
        .strip_prefix(QR_PREFIX)
        .ok_or_else(|| bad("prefix"))?;
    let bytes = data_encoding::BASE64URL_NOPAD
        .decode(b64.as_bytes())
        .map_err(|_| bad("encoding"))?;
    let inv = urdevice_proto::decode_invite(&bytes, MAX_QR_LEN).map_err(|_| bad("payload"))?;
    if inv.v != PROTO_VERSION {
        return Err(CoreError::Pairing("version".into()));
    }
    let device_id = DeviceId::from_slice(&inv.device_id).ok_or_else(|| bad("device id"))?;
    let token: [u8; TOKEN_LEN] = inv.token.as_slice().try_into().map_err(|_| bad("token"))?;
    let port = u16::try_from(inv.port)
        .ok()
        .filter(|p| *p != 0)
        .ok_or_else(|| bad("port"))?;
    let addrs: Vec<SocketAddr> = inv
        .addrs
        .iter()
        .take(MAX_INVITE_ADDRS)
        .filter_map(|a| a.parse::<IpAddr>().ok())
        .filter(|ip| !ip.is_unspecified() && !ip.is_multicast())
        .map(|ip| SocketAddr::new(ip, port))
        .collect();
    if addrs.is_empty() {
        return Err(bad("no address"));
    }
    if inv.expires_at <= now_unix {
        return Err(CoreError::Pairing("expired".into()));
    }
    Ok(ScannedInvite {
        device_id,
        name: clean_name(&inv.name),
        platform: clean_platform(&inv.platform),
        addrs,
        token,
        expires_at: inv.expires_at,
    })
}

/// The scanner's proof: HMAC-SHA256(token, binding ‖ fp_host ‖ fp_scanner).
pub fn pair_proof(token: &[u8], binding: &[u8], host: DeviceId, scanner: DeviceId) -> [u8; 32] {
    hmac_sha256(token, &[binding, &host.0, &scanner.0])
}

/// Check a proof in constant time.
pub fn verify_pair_proof(
    token: &[u8],
    binding: &[u8],
    host: DeviceId,
    scanner: DeviceId,
    proof: &[u8],
) -> bool {
    hmac_verify(token, &[binding, &host.0, &scanner.0], proof)
}

/// The 6-digit code from the TLS exporter value (design §7.2).
pub fn sas_code(exporter4: &[u8]) -> Result<String> {
    let b: [u8; 4] = exporter4
        .try_into()
        .map_err(|_| CoreError::Crypto("exporter length".into()))?;
    Ok(format!("{:06}", u32::from_be_bytes(b) % 1_000_000))
}

/// Failure counter for R3.7.
#[derive(Debug, Default)]
pub struct Lockout {
    failures: VecDeque<Instant>,
    locked_until: Option<Instant>,
}

impl Lockout {
    /// Record a failure. Returns true when this failure locked pairing.
    pub fn fail(&mut self, now: Instant) -> bool {
        self.trim(now);
        self.failures.push_back(now);
        if self.failures.len() >= LOCKOUT_FAILURES && self.locked_until.is_none() {
            self.locked_until = Some(now + LOCKOUT_DURATION);
            self.failures.clear();
            return true;
        }
        false
    }

    /// Time left until pairing opens again.
    pub fn remaining(&mut self, now: Instant) -> Option<Duration> {
        match self.locked_until {
            Some(t) if t > now => Some(t - now),
            Some(_) => {
                self.locked_until = None;
                None
            }
            None => None,
        }
    }

    fn trim(&mut self, now: Instant) {
        while let Some(&t) = self.failures.front() {
            if now.duration_since(t) > LOCKOUT_WINDOW {
                self.failures.pop_front();
            } else {
                break;
            }
        }
    }
}

/// Clean a display name from the network or the user: no control characters,
/// trimmed, at most 64 characters.
pub fn clean_name(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}')
        })
        .take(64)
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "Unknown device".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Clean a platform string: `[a-z0-9-]`, at most 16 characters.
pub fn clean_platform(s: &str) -> String {
    if !s.is_empty()
        && s.len() <= 16
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        s.to_owned()
    } else {
        "unknown".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invite(expires_at: u64) -> PairInvite {
        PairInvite {
            v: PROTO_VERSION,
            device_id: vec![7; 32],
            name: "Laptop\u{0007}".into(),
            platform: "linux".into(),
            addrs: vec!["192.168.1.8".into(), "garbage".into(), "0.0.0.0".into()],
            port: 47811,
            token: vec![1; 16],
            expires_at,
        }
    }

    #[test]
    fn invite_round_trip_and_cleaning() {
        let uri = invite_uri(&invite(200));
        let s = parse_invite_uri(&uri, 100).unwrap();
        assert_eq!(s.device_id, DeviceId([7; 32]));
        assert_eq!(s.name, "Laptop");
        assert_eq!(s.addrs, vec!["192.168.1.8:47811".parse().unwrap()]);
        assert_eq!(s.token, [1; 16]);
    }

    #[test]
    fn invite_rejections() {
        let uri = invite_uri(&invite(200));
        assert!(matches!(
            parse_invite_uri(&uri, 200),
            Err(CoreError::Pairing(_))
        ));
        assert!(parse_invite_uri("https://example.com", 0).is_err());
        assert!(parse_invite_uri(&format!("{QR_PREFIX}!!!"), 0).is_err());
        assert!(parse_invite_uri(&"x".repeat(MAX_QR_LEN + 1), 0).is_err());
        let mut short_token = invite(200);
        short_token.token = vec![1; 4];
        assert!(parse_invite_uri(&invite_uri(&short_token), 0).is_err());
        let mut no_addr = invite(200);
        no_addr.addrs.clear();
        assert!(parse_invite_uri(&invite_uri(&no_addr), 0).is_err());
    }

    #[test]
    fn proof_binds_everything() {
        let (a, b) = (DeviceId([1; 32]), DeviceId([2; 32]));
        let p = pair_proof(b"token", b"binding", a, b);
        assert!(verify_pair_proof(b"token", b"binding", a, b, &p));
        assert!(!verify_pair_proof(b"token2", b"binding", a, b, &p));
        assert!(!verify_pair_proof(b"token", b"other-session", a, b, &p));
        assert!(!verify_pair_proof(b"token", b"binding", b, a, &p));
    }

    #[test]
    fn sas_code_is_six_digits() {
        assert_eq!(sas_code(&[0, 0, 0, 7]).unwrap(), "000007");
        assert_eq!(sas_code(&[0xff; 4]).unwrap(), "967295");
        assert!(sas_code(&[1, 2]).is_err());
    }

    #[test]
    fn lockout_after_five_failures() {
        let mut l = Lockout::default();
        let t0 = Instant::now();
        for i in 0..4 {
            assert!(!l.fail(t0 + Duration::from_secs(i)));
        }
        assert!(l.remaining(t0).is_none());
        assert!(l.fail(t0 + Duration::from_secs(5)));
        assert!(l.remaining(t0 + Duration::from_secs(6)).is_some());
        assert!(
            l.remaining(t0 + LOCKOUT_DURATION + Duration::from_secs(6))
                .is_none()
        );
    }

    #[test]
    fn old_failures_fall_out_of_the_window() {
        let mut l = Lockout::default();
        let t0 = Instant::now();
        for i in 0..4 {
            l.fail(t0 + Duration::from_secs(i));
        }
        assert!(!l.fail(t0 + LOCKOUT_WINDOW + Duration::from_secs(10)));
    }

    #[test]
    fn names_and_platforms() {
        assert_eq!(clean_name("  "), "Unknown device");
        assert_eq!(clean_name(&"a".repeat(100)).len(), 64);
        assert_eq!(clean_name("a\u{202e}b\nc"), "abc");
        assert_eq!(clean_platform("android"), "android");
        assert_eq!(clean_platform("Win<script>"), "unknown");
    }
}
