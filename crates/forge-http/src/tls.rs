//! Forge's side of mutual TLS. See `artisan_api::tls`.

use anyhow::{Context, Result};
use artisan_api::tls::provider;
pub use artisan_api::tls::{Identity, certificate_id, client_config, fingerprint, fingerprint_of};
use rustls::DistinguishedName;
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::pki_types::{CertificateDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use std::sync::Arc;

pub(crate) fn server_config(identity: &Identity) -> Result<rustls::ServerConfig> {
    let provider = provider();
    let verifier = Arc::new(AnyClientCertificate {
        algorithms: provider.signature_verification_algorithms,
    });
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .context("TLS versions")?
        .with_client_cert_verifier(verifier)
        .with_single_cert(vec![identity.certificate_der()?], identity.key_der()?)
        .context("load Forge's certificate")?;
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(config)
}

/// Requires a client certificate and checks the handshake signature;
/// whether the certificate is allowed is checked per request.
#[derive(Debug)]
struct AnyClientCertificate {
    algorithms: WebPkiSupportedAlgorithms,
}

impl ClientCertVerifier for AnyClientCertificate {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        Ok(ClientCertVerified::assertion())
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
