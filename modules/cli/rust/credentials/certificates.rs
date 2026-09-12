use std::net::{IpAddr, Ipv4Addr};

use rcgen::PublicKeyData;
use zeroize::Zeroizing;

use super::ForgeCredentialError;
use super::storage::MAX_CAPABILITY_BYTES;

pub(super) struct ProvisionalMaterial {
    pub(super) capability: Zeroizing<[u8; 32]>,
    pub(super) private_key: Zeroizing<Vec<u8>>,
    pub(super) certificate: Vec<u8>,
}

impl std::fmt::Debug for ProvisionalMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProvisionalMaterial")
            .field("capability", &"[REDACTED]")
            .field("private_key", &"[REDACTED]")
            .field("certificate", &"[REDACTED]")
            .finish()
    }
}

pub(super) fn validate_cert_sans(cert_der: &[u8]) -> Result<(), ForgeCredentialError> {
    let (_, cert) = x509_parser::parse_x509_certificate(cert_der)
        .map_err(|_| ForgeCredentialError::InvalidCertificate)?;
    let has_san = cert
        .subject_alternative_name()
        .map_err(|_| ForgeCredentialError::InvalidCertificate)?;
    let Some(san) = has_san else {
        return Err(ForgeCredentialError::InvalidCertificate);
    };
    let mut has_dns_localhost = false;
    let mut has_ip_127 = false;
    for name in &san.value.general_names {
        match name {
            x509_parser::extensions::GeneralName::DNSName(dns) => {
                if *dns == "localhost" {
                    has_dns_localhost = true;
                }
            }
            x509_parser::extensions::GeneralName::IPAddress(bytes) => {
                if bytes.len() == 4 && bytes == &[127, 0, 0, 1] {
                    has_ip_127 = true;
                }
                if bytes.len() == 16
                    && bytes[12..] == [127, 0, 0, 1]
                    && bytes[..12] == [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
                {
                    has_ip_127 = true;
                }
            }
            _ => {}
        }
    }
    if !has_dns_localhost || !has_ip_127 {
        return Err(ForgeCredentialError::InvalidCertificate);
    }
    if cert.tbs_certificate.issuer != cert.tbs_certificate.subject {
        return Err(ForgeCredentialError::InvalidCertificate);
    }
    cert.verify_signature(None)
        .map_err(|_| ForgeCredentialError::InvalidCertificate)?;
    Ok(())
}

pub(super) fn validate_key_matches_cert(
    key_der: &[u8],
    cert_der: &[u8],
) -> Result<(), ForgeCredentialError> {
    let key_pair =
        rcgen::KeyPair::try_from(key_der).map_err(|_| ForgeCredentialError::InvalidCertificate)?;
    let cert_spki = {
        let (_, cert) = x509_parser::parse_x509_certificate(cert_der)
            .map_err(|_| ForgeCredentialError::InvalidCertificate)?;
        cert.tbs_certificate.subject_pki.raw.to_vec()
    };
    let key_spki = key_pair.subject_public_key_info();
    if cert_spki != key_spki {
        return Err(ForgeCredentialError::KeyMismatch);
    }
    let _ = rustls_pki_types::PrivateKeyDer::try_from(key_der.to_vec())
        .map_err(|_| ForgeCredentialError::InvalidCertificate)?;
    let _ = rustls_pki_types::CertificateDer::from(cert_der.to_vec());
    Ok(())
}

pub(super) fn generate_material() -> Result<ProvisionalMaterial, ForgeCredentialError> {
    let mut cap = Zeroizing::new([0_u8; MAX_CAPABILITY_BYTES]);
    getrandom::fill(&mut *cap).map_err(|_| ForgeCredentialError::Provisioning)?;
    let key_pair = rcgen::KeyPair::generate().map_err(|_| ForgeCredentialError::Provisioning)?;
    let mut params = rcgen::CertificateParams::new(vec!["localhost".to_string()])
        .map_err(|_| ForgeCredentialError::Provisioning)?;
    params.distinguished_name.push(
        rcgen::DnType::CommonName,
        rcgen::DnValue::Utf8String("localhost".to_string()),
    );
    params.subject_alt_names = vec![
        rcgen::SanType::DnsName(
            "localhost"
                .try_into()
                .map_err(|_| ForgeCredentialError::Provisioning)?,
        ),
        rcgen::SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST)),
    ];
    let cert = params
        .self_signed(&key_pair)
        .map_err(|_| ForgeCredentialError::Provisioning)?;
    let cert_der = cert.der().to_vec();
    let key_der = key_pair.serialize_der();
    validate_cert_sans(&cert_der)?;
    validate_key_matches_cert(&key_der, &cert_der)?;
    Ok(ProvisionalMaterial {
        capability: cap,
        private_key: Zeroizing::new(key_der),
        certificate: cert_der,
    })
}
