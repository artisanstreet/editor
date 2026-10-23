//! Mutual TLS with self-signed certificates (feature `tls`).
//!
//! Forge and every Editor make their own certificate once. TLS proves each
//! side holds its key; which certificates are allowed is decided by Forge's
//! `authorized_keys` table and by the Editor's pinned Forge certificate.
//! There is no CA.

use anyhow::{Context, Result};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, UnixTime};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// A certificate and its private key, stored base64.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Identity {
    pub certificate: String,
    pub key: String,
}

impl Identity {
    /// A fresh self-signed identity.
    pub fn generate(name: &str) -> Result<Self> {
        let certified = rcgen::generate_simple_self_signed(vec![name.to_owned()])
            .context("generate a certificate")?;
        Ok(Self {
            certificate: STANDARD.encode(certified.cert.der()),
            key: STANDARD.encode(certified.signing_key.serialize_der()),
        })
    }

    pub fn certificate_der(&self) -> Result<CertificateDer<'static>> {
        Ok(CertificateDer::from(
            STANDARD
                .decode(&self.certificate)
                .context("decode certificate")?,
        ))
    }

    pub fn key_der(&self) -> Result<PrivateKeyDer<'static>> {
        Ok(PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            STANDARD.decode(&self.key).context("decode key")?,
        )))
    }
}

/// The base64 DER of a certificate: how devices are identified.
#[must_use]
pub fn certificate_id(certificate: &CertificateDer<'_>) -> String {
    STANDARD.encode(certificate.as_ref())
}

pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// SHA-256 of a certificate's DER, lowercase hex: what an Editor pins.
#[must_use]
pub fn fingerprint(certificate_der: &[u8]) -> String {
    use sha2::Digest;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = sha2::Sha256::digest(certificate_der);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push(char::from(HEX[usize::from(byte >> 4)]));
        hex.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    hex
}

/// The fingerprint of a base64 DER certificate.
pub fn fingerprint_of(certificate: &str) -> Result<String> {
    Ok(fingerprint(
        &STANDARD.decode(certificate).context("decode certificate")?,
    ))
}

/// Client side: present `identity` and accept only the Forge whose
/// certificate has this fingerprint (see [`fingerprint`]).
pub fn client_config(identity: &Identity, forge_fingerprint: &str) -> Result<rustls::ClientConfig> {
    let provider = provider();
    let pinned = forge_fingerprint.to_ascii_lowercase();
    let verifier = Arc::new(PinnedServer {
        pinned,
        algorithms: provider.signature_verification_algorithms,
    });
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .context("TLS versions")?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(vec![identity.certificate_der()?], identity.key_der()?)
        .context("load this device's certificate")?;
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(config)
}

#[derive(Debug)]
struct PinnedServer {
    pinned: String,
    algorithms: WebPkiSupportedAlgorithms,
}

impl rustls::client::danger::ServerCertVerifier for PinnedServer {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if fingerprint(end_entity.as_ref()) == self.pinned {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(
                "this is not the paired Forge".into(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}
