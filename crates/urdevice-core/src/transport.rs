//! QUIC transport with mutual TLS 1.3 and pinned keys (R4.1, R4.2, design §6.1).
//!
//! Trust never comes from a certificate chain. Each side accepts the other's
//! certificate only when its Device ID is paired, or while a pairing context
//! is open. TLS signatures are still checked against the certificate's key,
//! so the peer must hold the private key.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, SignatureScheme};

use crate::identity::{DeviceId, Identity};
use crate::{CoreError, Result};

/// ALPN protocol id.
pub const ALPN: &[u8] = b"urdevice/1";
/// Server name sent in the TLS handshake. Ignored by the verifiers.
pub const SERVER_NAME: &str = "urdevice";
/// QUIC keep-alive interval while idle. It must stay well under [`IDLE_TIMEOUT`].
pub const KEEP_ALIVE: Duration = Duration::from_secs(10);
/// A session with no traffic for this long is closed.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// Application close codes.
pub mod close {
    /// Normal close.
    pub const NORMAL: u32 = 0;
    /// The user chose Disconnect or Forget.
    pub const USER: u32 = 1;
    /// Unsupported protocol version.
    pub const VERSION: u32 = 2;
    /// Malformed or unexpected message.
    pub const PROTOCOL: u32 = 3;
    /// A second connection replaced this one.
    pub const DUPLICATE: u32 = 4;
    /// Pairing failed or was cancelled.
    pub const PAIRING: u32 = 5;
    /// This device is shutting down.
    pub const SHUTDOWN: u32 = 6;
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn algorithms() -> WebPkiSupportedAlgorithms {
    rustls::crypto::ring::default_provider().signature_verification_algorithms
}

/// What the server side accepts. Shared with the node and updated live.
#[derive(Debug, Default)]
pub struct TrustPolicy {
    state: RwLock<PolicyState>,
}

#[derive(Debug, Default)]
struct PolicyState {
    paired: HashSet<DeviceId>,
    pairing_open: bool,
}

impl TrustPolicy {
    /// Replace the set of paired devices.
    pub fn set_paired(&self, ids: impl IntoIterator<Item = DeviceId>) {
        if let Ok(mut s) = self.state.write() {
            s.paired = ids.into_iter().collect();
        }
    }

    /// Open or close the pairing window (R3.8).
    pub fn set_pairing_open(&self, open: bool) {
        if let Ok(mut s) = self.state.write() {
            s.pairing_open = open;
        }
    }

    /// True while unknown devices may connect to pair.
    pub fn pairing_open(&self) -> bool {
        self.state.read().map(|s| s.pairing_open).unwrap_or(false)
    }

    /// True when `id` is paired.
    pub fn is_paired(&self, id: &DeviceId) -> bool {
        self.state
            .read()
            .map(|s| s.paired.contains(id))
            .unwrap_or(false)
    }

    fn accepts(&self, id: &DeviceId) -> bool {
        self.state
            .read()
            .map(|s| s.paired.contains(id) || s.pairing_open)
            .unwrap_or(false)
    }
}

/// What the client expects from the server's certificate.
#[derive(Clone, Copy, Debug)]
pub enum Expect {
    /// Exactly this Device ID: a paired device or a scanned QR code.
    Device(DeviceId),
    /// Any key. Only for code-confirmation pairing, where the users compare
    /// the code on both screens before anything is stored.
    AnyForCodePairing,
}

#[derive(Debug)]
struct PinnedServerVerifier {
    expect: Expect,
    algs: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for PinnedServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        let id = DeviceId::of_cert(end_entity)
            .map_err(|_| rustls::Error::General("bad certificate".into()))?;
        match self.expect {
            Expect::Device(want) if want == id => Ok(ServerCertVerified::assertion()),
            Expect::Device(_) => Err(rustls::Error::General("unexpected device key".into())),
            Expect::AnyForCodePairing => Ok(ServerCertVerified::assertion()),
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.2 is not used".into()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

#[derive(Debug)]
struct PinnedClientVerifier {
    policy: Arc<TrustPolicy>,
    algs: WebPkiSupportedAlgorithms,
}

impl ClientCertVerifier for PinnedClientVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> std::result::Result<ClientCertVerified, rustls::Error> {
        let id = DeviceId::of_cert(end_entity)
            .map_err(|_| rustls::Error::General("bad certificate".into()))?;
        if self.policy.accepts(&id) {
            Ok(ClientCertVerified::assertion())
        } else {
            tracing::debug!(peer = %id.short_hex(), "refused an unknown device outside pairing");
            Err(rustls::Error::General("unknown device".into()))
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.2 is not used".into()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

fn transport_config() -> Arc<quinn::TransportConfig> {
    let mut t = quinn::TransportConfig::default();
    t.keep_alive_interval(Some(KEEP_ALIVE));
    if let Ok(idle) = quinn::IdleTimeout::try_from(IDLE_TIMEOUT) {
        t.max_idle_timeout(Some(idle));
    }
    Arc::new(t)
}

/// Server config that checks client keys against `policy`.
pub fn server_config(identity: &Identity, policy: Arc<TrustPolicy>) -> Result<quinn::ServerConfig> {
    let verifier = Arc::new(PinnedClientVerifier {
        policy,
        algs: algorithms(),
    });
    let mut tls = rustls::ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(verifier)
        .with_single_cert(identity.cert_chain(), identity.private_key())?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let quic = QuicServerConfig::try_from(tls).map_err(|e| CoreError::Crypto(e.to_string()))?;
    let mut cfg = quinn::ServerConfig::with_crypto(Arc::new(quic));
    cfg.transport_config(transport_config());
    Ok(cfg)
}

/// Client config that pins the server's key.
pub fn client_config(identity: &Identity, expect: Expect) -> Result<quinn::ClientConfig> {
    let verifier = Arc::new(PinnedServerVerifier {
        expect,
        algs: algorithms(),
    });
    let mut tls = rustls::ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(identity.cert_chain(), identity.private_key())?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let quic = QuicClientConfig::try_from(tls).map_err(|e| CoreError::Crypto(e.to_string()))?;
    let mut cfg = quinn::ClientConfig::new(Arc::new(quic));
    cfg.transport_config(transport_config());
    Ok(cfg)
}

/// Bind the QUIC endpoint on `port`, or on any free port when it is taken
/// (design §17.1).
pub fn bind_endpoint(
    identity: &Identity,
    policy: Arc<TrustPolicy>,
    bind_ip: std::net::IpAddr,
    port: u16,
) -> Result<quinn::Endpoint> {
    let cfg = server_config(identity, policy.clone())?;
    match quinn::Endpoint::server(cfg, SocketAddr::new(bind_ip, port)) {
        Ok(ep) => Ok(ep),
        Err(e) if port != 0 => {
            tracing::warn!(port, error = %e, "QUIC port is taken; using a free port");
            let cfg = server_config(identity, policy)?;
            Ok(quinn::Endpoint::server(cfg, SocketAddr::new(bind_ip, 0))?)
        }
        Err(e) => Err(e.into()),
    }
}

/// The Device ID of the peer on an established connection.
pub fn peer_id(conn: &quinn::Connection) -> Result<DeviceId> {
    let ident = conn
        .peer_identity()
        .ok_or_else(|| CoreError::Protocol("no peer certificate".into()))?;
    let certs = ident
        .downcast::<Vec<CertificateDer<'static>>>()
        .map_err(|_| CoreError::Protocol("unexpected peer identity".into()))?;
    let cert = certs
        .first()
        .ok_or_else(|| CoreError::Protocol("empty peer certificate chain".into()))?;
    DeviceId::of_cert(cert)
}

/// A TLS exporter value bound to this connection (RFC 5705 / 8446 §7.5).
pub fn exporter(conn: &quinn::Connection, label: &[u8], len: usize) -> Result<Vec<u8>> {
    let mut out = vec![0; len];
    conn.export_keying_material(&mut out, label, b"")
        .map_err(|_| CoreError::Crypto("exporter failed".into()))?;
    Ok(out)
}
