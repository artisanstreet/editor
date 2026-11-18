use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rustls::DistinguishedName;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
#[error("generate a self-signed certificate for {name}")]
pub struct GenerateIdentityError {
    pub name: String,
    #[source]
    pub source: rcgen::Error,
}

#[derive(Debug, thiserror::Error)]
pub enum DecodeIdentityError {
    #[error("decode the certificate's base64")]
    Certificate(#[source] base64::DecodeError),
    #[error("decode the private key's base64")]
    Key(#[source] base64::DecodeError),
}

#[derive(Debug, thiserror::Error)]
pub enum TlsConfigError {
    #[error("choose TLS 1.3")]
    Versions(#[source] rustls::Error),
    #[error("decode this device's identity")]
    Decode(#[source] DecodeIdentityError),
    #[error("load this device's certificate and key")]
    LoadIdentity(#[source] rustls::Error),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Identity {
    pub certificate: String,
    pub key: crate::Secret,
}

impl Identity {
    pub fn generate(name: &str) -> Result<Self, GenerateIdentityError> {
        let certified =
            rcgen::generate_simple_self_signed(vec![name.to_owned()]).map_err(|source| {
                GenerateIdentityError {
                    name: name.to_owned(),
                    source,
                }
            })?;
        Ok(Self {
            certificate: STANDARD.encode(certified.cert.der()),
            key: crate::Secret::new(STANDARD.encode(certified.signing_key.serialize_der())),
        })
    }

    pub fn certificate_der(&self) -> Result<CertificateDer<'static>, DecodeIdentityError> {
        Ok(CertificateDer::from(
            STANDARD
                .decode(&self.certificate)
                .map_err(DecodeIdentityError::Certificate)?,
        ))
    }

    pub fn key_der(&self) -> Result<PrivateKeyDer<'static>, DecodeIdentityError> {
        Ok(PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            STANDARD
                .decode(self.key.expose())
                .map_err(DecodeIdentityError::Key)?,
        )))
    }

    fn ders(
        &self,
    ) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>), TlsConfigError> {
        let certificate = self.certificate_der().map_err(TlsConfigError::Decode)?;
        let key = self.key_der().map_err(TlsConfigError::Decode)?;
        Ok((vec![certificate], key))
    }
}

#[must_use]
pub fn certificate_id(certificate: &CertificateDer<'_>) -> String {
    STANDARD.encode(certificate.as_ref())
}

#[must_use]
pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

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

#[derive(Debug, thiserror::Error)]
#[error("decode the certificate's base64")]
pub struct DecodeCertificateError(#[source] pub base64::DecodeError);

pub fn fingerprint_of(certificate: &str) -> Result<String, DecodeCertificateError> {
    Ok(fingerprint(
        &STANDARD
            .decode(certificate)
            .map_err(DecodeCertificateError)?,
    ))
}

const ALPN: [&[u8]; 2] = [b"h2", b"http/1.1"];

pub fn client_config(
    identity: &Identity,
    forge_fingerprint: &str,
) -> Result<rustls::ClientConfig, TlsConfigError> {
    let provider = provider();
    let verifier = Arc::new(PinnedServer {
        pinned: forge_fingerprint.to_ascii_lowercase(),
        signatures: HandshakeSignatures(provider.signature_verification_algorithms),
    });
    let (certificates, key) = identity.ders()?;
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(TlsConfigError::Versions)?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(certificates, key)
        .map_err(TlsConfigError::LoadIdentity)?;
    config.alpn_protocols = ALPN.iter().map(|protocol| protocol.to_vec()).collect();
    Ok(config)
}

pub fn server_config(identity: &Identity) -> Result<rustls::ServerConfig, TlsConfigError> {
    let provider = provider();
    let verifier = Arc::new(AnyClientCertificate(HandshakeSignatures(
        provider.signature_verification_algorithms,
    )));
    let (certificates, key) = identity.ders()?;
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(TlsConfigError::Versions)?
        .with_client_cert_verifier(verifier)
        .with_single_cert(certificates, key)
        .map_err(TlsConfigError::LoadIdentity)?;
    config.alpn_protocols = ALPN.iter().map(|protocol| protocol.to_vec()).collect();
    Ok(config)
}

#[derive(Debug)]
struct HandshakeSignatures(WebPkiSupportedAlgorithms);

impl HandshakeSignatures {
    fn tls12(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0)
    }

    fn tls13(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0)
    }

    fn schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.supported_schemes()
    }
}

#[derive(Debug)]
struct PinnedServer {
    pinned: String,
    signatures: HandshakeSignatures,
}

impl ServerCertVerifier for PinnedServer {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if fingerprint(end_entity.as_ref()) == self.pinned {
            Ok(ServerCertVerified::assertion())
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
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.signatures.tls12(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.signatures.tls13(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.signatures.schemes()
    }
}

#[derive(Debug)]
struct AnyClientCertificate(HandshakeSignatures);

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
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.0.tls12(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.0.tls13(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.schemes()
    }
}
