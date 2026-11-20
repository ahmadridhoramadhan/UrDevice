//! Localhost sharing and Expose on LAN (#22, design §11): wire messages for `ServiceId::TUNNEL`.
//!
//! Owned by the tunnel feature. Field numbers and stream kinds are local to
//! this service, so they never clash with another feature. Mirror each
//! message in `proto/features/tunnel.proto`.
//!
//! - The host pushes [`SharedPorts`] to every connected device
//!   (`ToController`) when a session opens and whenever its list or its
//!   Localhost setting changes. The list is empty while the setting is Deny.
//! - The viewer opens one bidirectional stream per TCP connection, kind
//!   [`STREAM_TCP`], with an encoded [`TunnelOpen`] as `meta`, sent
//!   `ToHost` under the activity it was granted for the tunnel.

use prost::Message;

/// Stream kind: one TCP connection to a shared port.
pub const STREAM_TCP: u32 = 1;

/// Most ports a host shares, and most a viewer accepts in a list.
pub const MAX_PORTS: usize = 64;

/// Longest port label, in characters.
pub const MAX_LABEL: usize = 64;

/// Largest encoded [`TunnelOpen`] accepted.
pub const MAX_OPEN: usize = 16;

/// One shared port.
#[derive(Clone, PartialEq, Eq, Message)]
pub struct PortInfo {
    /// TCP port on the host's loopback, 1-65535.
    #[prost(uint32, tag = "1")]
    pub port: u32,
    /// User label, may be empty.
    #[prost(string, tag = "2")]
    pub label: String,
}

/// The ports a host shares (host to viewer).
#[derive(Clone, PartialEq, Eq, Message)]
pub struct SharedPorts {
    #[prost(message, repeated, tag = "1")]
    pub ports: Vec<PortInfo>,
}

/// Stream header `meta`: which shared port the connection is for.
#[derive(Clone, PartialEq, Eq, Message)]
pub struct TunnelOpen {
    #[prost(uint32, tag = "1")]
    pub port: u32,
}

/// Decode a [`TunnelOpen`] from untrusted `meta`. `None` when it is too
/// large, malformed, or names port 0 or a port above 65535.
pub fn decode_open(meta: &[u8]) -> Option<u16> {
    if meta.len() > MAX_OPEN {
        return None;
    }
    let open = TunnelOpen::decode(meta).ok()?;
    u16::try_from(open.port).ok().filter(|p| *p != 0)
}

/// Decode a [`SharedPorts`] from an untrusted body: at most [`MAX_PORTS`]
/// entries, valid port numbers, labels cut to [`MAX_LABEL`] characters with
/// control characters removed, and no duplicate ports. `None` when the body
/// is malformed or lists too many ports.
pub fn decode_ports(body: &[u8]) -> Option<Vec<(u16, String)>> {
    // A full list is far below this; anything bigger is not ours.
    if body.len() > MAX_PORTS * (MAX_LABEL * 4 + 16) {
        return None;
    }
    let list = SharedPorts::decode(body).ok()?;
    if list.ports.len() > MAX_PORTS {
        return None;
    }
    let mut out: Vec<(u16, String)> = Vec::with_capacity(list.ports.len());
    for p in list.ports {
        let Some(port) = u16::try_from(p.port).ok().filter(|p| *p != 0) else {
            continue;
        };
        if out.iter().any(|(q, _)| *q == port) {
            continue;
        }
        out.push((port, clean_label(&p.label)));
    }
    Some(out)
}

/// A label as it may be stored and shown: control characters removed,
/// surrounding space trimmed, at most [`MAX_LABEL`] characters.
pub fn clean_label(label: &str) -> String {
    label
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_LABEL)
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_round_trip_and_limits() {
        let b = TunnelOpen { port: 3000 }.encode_to_vec();
        assert_eq!(decode_open(&b), Some(3000));
        assert_eq!(decode_open(&TunnelOpen { port: 0 }.encode_to_vec()), None);
        assert_eq!(
            decode_open(&TunnelOpen { port: 70000 }.encode_to_vec()),
            None
        );
        assert_eq!(decode_open(&[0xff; 40]), None);
        assert_eq!(decode_open(&[0x08]), None);
    }

    #[test]
    fn ports_are_cleaned() {
        let body = SharedPorts {
            ports: vec![
                PortInfo {
                    port: 3000,
                    label: "  web\u{7}app ".into(),
                },
                PortInfo {
                    port: 3000,
                    label: "dup".into(),
                },
                PortInfo {
                    port: 0,
                    label: String::new(),
                },
                PortInfo {
                    port: 99999,
                    label: String::new(),
                },
                PortInfo {
                    port: 8080,
                    label: "x".repeat(500),
                },
            ],
        }
        .encode_to_vec();
        let out = decode_ports(&body).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], (3000, "webapp".into()));
        assert_eq!(out[1].1.chars().count(), MAX_LABEL);
    }

    #[test]
    fn too_many_ports_is_refused() {
        let body = SharedPorts {
            ports: (1..=(MAX_PORTS as u32 + 1))
                .map(|port| PortInfo {
                    port,
                    label: String::new(),
                })
                .collect(),
        }
        .encode_to_vec();
        assert_eq!(decode_ports(&body), None);
    }

    /// Random and mutated inputs never panic and never break the limits.
    /// (A small seeded generator: the workspace has no proptest yet.)
    #[test]
    fn decoders_never_panic() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let valid = SharedPorts {
            ports: (1..20)
                .map(|port| PortInfo {
                    port,
                    label: "dev".into(),
                })
                .collect(),
        }
        .encode_to_vec();
        for i in 0..20_000 {
            let bytes: Vec<u8> = if i % 2 == 0 {
                let len = (next() % 600) as usize;
                (0..len).map(|_| next() as u8).collect()
            } else {
                let mut b = valid.clone();
                for _ in 0..(next() % 8) {
                    let at = (next() as usize) % b.len();
                    b[at] = next() as u8;
                }
                b
            };
            let _ = decode_open(&bytes);
            if let Some(list) = decode_ports(&bytes) {
                assert!(list.len() <= MAX_PORTS);
                for (port, label) in list {
                    assert_ne!(port, 0);
                    assert!(label.chars().count() <= MAX_LABEL);
                }
            }
        }
    }
}
