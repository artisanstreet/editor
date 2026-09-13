use std::path::{Path, PathBuf};

use zeroize::Zeroizing;

mod certificates;
pub mod hosts;
mod keychain;
mod storage;
mod validation;

use certificates::{generate_material, validate_cert_sans};
use keychain::{RECONNECT_CAPABILITY_FILENAME, RECONNECT_LOCK_FILENAME};

pub use keychain::{
    RECONNECT_LOCK_TIMEOUT, ReconnectAttempt, ReconnectBinding, ReconnectCapabilityState,
    ReconnectCapabilityStore, ReconnectSessionLease,
};

use storage::{
    CreatedFile, MAX_CAPABILITY_BYTES, MAX_CAPABILITY_READ_BYTES, MAX_CERTIFICATE_READ_BYTES,
    acquire_lock, check_ancestors_all, cleanup_created, ensure_credentials_dir, install_atomic,
    read_private_material, validate_home,
};

// The credential-boundary seam keeps its historical `crate::credentials::*`
// path even though the implementation now lives in the storage child module.
#[allow(unused_imports)]
pub(crate) use storage::{ensure_private_directory, validate_private_directory};

use validation::{
    CredentialManifest, classify_capability_length, classify_certificate_length,
    validate_existing_bundle, validate_existing_identity_bundle,
};

#[derive(Clone, Eq, PartialEq)]
pub enum ForgeCredentialError {
    InvalidHome(PathBuf),
    UnsafePath(PathBuf),
    Io {
        context: &'static str,
        path: PathBuf,
    },
    ManifestMalformed,
    ManifestSchema,
    ManifestVersion,
    ManifestTraversal,
    ManifestUnknownField,
    ManifestDuplicateField,
    PartialBundle,
    InvalidCapability {
        path: PathBuf,
    },
    InvalidCertificate,
    KeyMismatch,
    WindowsAcl,
    Provisioning,
    CapabilityBusy,
    ReconnectRecordMissing,
    ReconnectRecordMalformed,
    ReconnectCapabilityUnavailable,
    ReconnectBindingMismatch,
    ReconnectStaleWriter,
    ReconnectGenerationOverflow,
    ReconnectInvalidBinding,
    ReconnectAttemptComplete,
    ReconnectRecordExists,
    IdentityBundleMissing,
}

impl std::fmt::Display for ForgeCredentialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidHome(path) => write!(f, "invalid Artisan home: {}", path.display()),
            Self::UnsafePath(path) => write!(
                f,
                "refusing unsafe filesystem operation on {}",
                path.display()
            ),
            Self::Io { context, path } => write!(f, "{context} at {}: [REDACTED]", path.display()),
            Self::ManifestMalformed => write!(f, "invalid credential manifest: malformed"),
            Self::ManifestSchema => write!(f, "invalid credential manifest: schema"),
            Self::ManifestVersion => write!(f, "invalid credential manifest: version"),
            Self::ManifestTraversal => write!(f, "invalid credential manifest: traversal"),
            Self::ManifestUnknownField => {
                write!(f, "invalid credential manifest: unknown field")
            }
            Self::ManifestDuplicateField => {
                write!(f, "invalid credential manifest: duplicate field")
            }
            Self::PartialBundle => write!(f, "partial credential bundle"),
            Self::InvalidCapability { path } => write!(
                f,
                "capability at {} has invalid length (expected 32)",
                path.display()
            ),
            Self::InvalidCertificate => write!(f, "invalid certificate"),
            Self::KeyMismatch => write!(f, "private key does not match certificate"),
            Self::WindowsAcl => write!(f, "Windows ACL error"),
            Self::Provisioning => write!(f, "provisioning failed"),
            Self::CapabilityBusy => write!(f, "reconnect capability store is busy"),
            Self::ReconnectRecordMissing => write!(f, "reconnect capability record is missing"),
            Self::ReconnectRecordMalformed => write!(f, "invalid reconnect capability record"),
            Self::ReconnectCapabilityUnavailable => {
                write!(f, "reconnect capability is unavailable")
            }
            Self::ReconnectBindingMismatch => write!(f, "reconnect binding mismatch"),
            Self::ReconnectStaleWriter => write!(f, "reconnect capability writer is stale"),
            Self::ReconnectGenerationOverflow => {
                write!(f, "reconnect capability generation overflow")
            }
            Self::ReconnectInvalidBinding => write!(f, "invalid reconnect binding"),
            Self::ReconnectAttemptComplete => write!(f, "reconnect attempt is already complete"),
            Self::ReconnectRecordExists => write!(f, "reconnect capability record already exists"),
            Self::IdentityBundleMissing => write!(f, "client identity bundle is missing"),
        }
    }
}

impl std::fmt::Debug for ForgeCredentialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidHome(path) => f
                .debug_tuple("InvalidHome")
                .field(&path.display().to_string())
                .finish(),
            Self::UnsafePath(path) => f
                .debug_tuple("UnsafePath")
                .field(&path.display().to_string())
                .finish(),
            Self::Io { context, path } => f
                .debug_struct("Io")
                .field("context", context)
                .field("path", &path.display().to_string())
                .finish(),
            Self::ManifestMalformed => f.debug_tuple("ManifestMalformed").finish(),
            Self::ManifestSchema => f.debug_tuple("ManifestSchema").finish(),
            Self::ManifestVersion => f.debug_tuple("ManifestVersion").finish(),
            Self::ManifestTraversal => f.debug_tuple("ManifestTraversal").finish(),
            Self::ManifestUnknownField => f.debug_tuple("ManifestUnknownField").finish(),
            Self::ManifestDuplicateField => f.debug_tuple("ManifestDuplicateField").finish(),
            Self::PartialBundle => f.debug_tuple("PartialBundle").finish(),
            Self::InvalidCapability { path } => f
                .debug_struct("InvalidCapability")
                .field("path", &path.display().to_string())
                .finish(),
            Self::InvalidCertificate => f.debug_tuple("InvalidCertificate").finish(),
            Self::KeyMismatch => f.debug_tuple("KeyMismatch").finish(),
            Self::WindowsAcl => f.debug_tuple("WindowsAcl").finish(),
            Self::Provisioning => f.debug_tuple("Provisioning").finish(),
            Self::CapabilityBusy => f.debug_tuple("CapabilityBusy").finish(),
            Self::ReconnectRecordMissing => f.debug_tuple("ReconnectRecordMissing").finish(),
            Self::ReconnectRecordMalformed => f.debug_tuple("ReconnectRecordMalformed").finish(),
            Self::ReconnectCapabilityUnavailable => {
                f.debug_tuple("ReconnectCapabilityUnavailable").finish()
            }
            Self::ReconnectBindingMismatch => f.debug_tuple("ReconnectBindingMismatch").finish(),
            Self::ReconnectStaleWriter => f.debug_tuple("ReconnectStaleWriter").finish(),
            Self::ReconnectGenerationOverflow => {
                f.debug_tuple("ReconnectGenerationOverflow").finish()
            }
            Self::ReconnectInvalidBinding => f.debug_tuple("ReconnectInvalidBinding").finish(),
            Self::ReconnectAttemptComplete => f.debug_tuple("ReconnectAttemptComplete").finish(),
            Self::ReconnectRecordExists => f.debug_tuple("ReconnectRecordExists").finish(),
            Self::IdentityBundleMissing => f.debug_tuple("IdentityBundleMissing").finish(),
        }
    }
}

impl std::error::Error for ForgeCredentialError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForgeCredentialPaths {
    credentials_dir: PathBuf,
    manifest: PathBuf,
    capability: PathBuf,
    certificates: Vec<PathBuf>,
    private_key: PathBuf,
}

impl ForgeCredentialPaths {
    pub fn new(home: &Path) -> Result<Self, ForgeCredentialError> {
        validate_home(home)?;
        let credentials_dir = home.join("credentials");
        Ok(Self {
            credentials_dir: credentials_dir.clone(),
            manifest: credentials_dir.join("manifest.json"),
            capability: credentials_dir.join("bootstrap-capability.bin"),
            certificates: vec![credentials_dir.join("localhost-leaf.der")],
            private_key: credentials_dir.join("localhost-key.pkcs8.der"),
        })
    }

    pub fn from_home(home: &Path) -> Result<Self, ForgeCredentialError> {
        Self::new(home)
    }

    pub fn manifest_path(&self) -> &Path {
        &self.manifest
    }

    pub fn manifest(&self) -> &Path {
        &self.manifest
    }

    pub fn capability_path(&self) -> &Path {
        &self.capability
    }

    pub fn capability(&self) -> &Path {
        &self.capability
    }

    pub fn certificate_paths(&self) -> &[PathBuf] {
        &self.certificates
    }

    pub fn certificates(&self) -> &[PathBuf] {
        &self.certificates
    }

    pub fn private_key_path(&self) -> &Path {
        &self.private_key
    }

    pub fn private_key(&self) -> &Path {
        &self.private_key
    }

    pub fn credentials_dir(&self) -> PathBuf {
        self.credentials_dir.clone()
    }

    pub fn lock_path(&self) -> PathBuf {
        self.credentials_dir().join(".provision.lock")
    }

    /// Returns the rotated reconnect-capability record path.
    pub fn reconnect_capability_path(&self) -> PathBuf {
        self.credentials_dir().join(RECONNECT_CAPABILITY_FILENAME)
    }

    /// Returns the whole-session reconnect-capability lock path.
    pub fn reconnect_lock_path(&self) -> PathBuf {
        self.credentials_dir().join(RECONNECT_LOCK_FILENAME)
    }
}

pub struct NativeClientCredentials {
    paths: ForgeCredentialPaths,
    certificate: rustls_pki_types::CertificateDer<'static>,
    capability: artisan_protocol::LocalCapability,
}

impl NativeClientCredentials {
    pub fn paths(&self) -> &ForgeCredentialPaths {
        &self.paths
    }

    pub fn into_parts(
        self,
    ) -> (
        rustls_pki_types::CertificateDer<'static>,
        artisan_protocol::LocalCapability,
    ) {
        (self.certificate, self.capability)
    }
}

/// Existing client identity material validated without provisioning authority.
///
/// The private key is deliberately kept behind the validated credential paths;
/// callers that need it can use the existing private-file boundary owned by
/// the Forge launcher. This value never contains the bootstrap capability.
pub struct NativeClientIdentity {
    paths: ForgeCredentialPaths,
    certificate: rustls_pki_types::CertificateDer<'static>,
}

impl NativeClientIdentity {
    /// Returns the paths whose certificate and private key were validated.
    #[must_use]
    pub fn paths(&self) -> &ForgeCredentialPaths {
        &self.paths
    }

    /// Returns the validated leaf certificate.
    #[must_use]
    pub fn certificate(&self) -> &rustls_pki_types::CertificateDer<'static> {
        &self.certificate
    }

    /// Consumes the identity into its validated paths and leaf certificate.
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        ForgeCredentialPaths,
        rustls_pki_types::CertificateDer<'static>,
    ) {
        (self.paths, self.certificate)
    }
}

pub fn provision_or_load(home: &Path) -> Result<ForgeCredentialPaths, ForgeCredentialError> {
    validate_home(home)?;
    check_ancestors_all(home, true)?;
    let paths = ForgeCredentialPaths::new(home)?;
    let credentials_dir = paths.credentials_dir();
    ensure_credentials_dir(&credentials_dir)?;
    let lock_path = paths.lock_path();
    let _lock = acquire_lock(&lock_path)?;
    if validate_existing_bundle(&paths)? {
        return Ok(paths);
    }
    let material = generate_material()?;
    let mut created: Vec<CreatedFile> = Vec::new();
    let result = (|| -> Result<(), ForgeCredentialError> {
        install_atomic(
            &credentials_dir,
            "bootstrap-capability.bin",
            material.capability.as_ref(),
            &mut created,
        )?;
        install_atomic(
            &credentials_dir,
            "localhost-leaf.der",
            &material.certificate,
            &mut created,
        )?;
        install_atomic(
            &credentials_dir,
            "localhost-key.pkcs8.der",
            material.private_key.as_ref(),
            &mut created,
        )?;
        let manifest = CredentialManifest {
            schema: "artisan-forge-credentials-v1".to_string(),
            version: 1,
            bootstrap_capability: "bootstrap-capability.bin".to_string(),
            certificate_chain: vec!["localhost-leaf.der".to_string()],
            private_key: "localhost-key.pkcs8.der".to_string(),
        };
        let manifest_bytes = serde_json::to_vec_pretty(&manifest)
            .map_err(|_| ForgeCredentialError::ManifestMalformed)?;
        install_atomic(
            &credentials_dir,
            "manifest.json",
            &manifest_bytes,
            &mut created,
        )?;
        Ok(())
    })();
    if let Err(error) = result {
        cleanup_created(created);
        return Err(error);
    }
    match validate_existing_bundle(&paths) {
        Ok(true) => Ok(paths),
        Ok(false) => {
            cleanup_created(created);
            Err(ForgeCredentialError::Provisioning)
        }
        Err(error) => {
            cleanup_created(created);
            Err(error)
        }
    }
}

fn local_capability_from_bytes(
    bytes: &[u8],
    path: &Path,
) -> Result<artisan_protocol::LocalCapability, ForgeCredentialError> {
    classify_capability_length(bytes.len(), path)?;
    let mut exact = Zeroizing::new([0_u8; MAX_CAPABILITY_BYTES]);
    exact[..].copy_from_slice(bytes);
    Ok(artisan_protocol::LocalCapability::from_bytes(*exact))
}

pub fn load_client_credentials(
    home: &Path,
) -> Result<NativeClientCredentials, ForgeCredentialError> {
    let paths = provision_or_load(home)?;
    let certificate_path = &paths.certificate_paths()[0];
    let certificate_bytes = read_private_material(
        &paths,
        certificate_path,
        MAX_CERTIFICATE_READ_BYTES,
        ForgeCredentialError::InvalidCertificate,
    )?;
    classify_certificate_length(certificate_bytes.len())?;
    validate_cert_sans(&certificate_bytes)?;
    let certificate = rustls_pki_types::CertificateDer::from(certificate_bytes.as_slice().to_vec());

    let capability_path = paths.capability_path();
    let capability_bytes = read_private_material(
        &paths,
        capability_path,
        MAX_CAPABILITY_READ_BYTES,
        ForgeCredentialError::InvalidCapability {
            path: capability_path.to_path_buf(),
        },
    )?;
    let capability = local_capability_from_bytes(capability_bytes.as_slice(), capability_path)?;

    Ok(NativeClientCredentials {
        paths,
        certificate,
        capability,
    })
}

/// Loads existing client identity material without provisioning or reading
/// bootstrap authority.
pub fn load_existing_client_identity(
    home: &Path,
) -> Result<NativeClientIdentity, ForgeCredentialError> {
    validate_home(home)?;
    check_ancestors_all(home, true)?;
    let paths = ForgeCredentialPaths::new(home)?;
    if !validate_existing_identity_bundle(&paths)? {
        return Err(ForgeCredentialError::IdentityBundleMissing);
    }
    let certificate_path = &paths.certificate_paths()[0];
    let certificate_bytes = read_private_material(
        &paths,
        certificate_path,
        MAX_CERTIFICATE_READ_BYTES,
        ForgeCredentialError::InvalidCertificate,
    )?;
    classify_certificate_length(certificate_bytes.len())?;
    validate_cert_sans(&certificate_bytes)?;
    let certificate = rustls_pki_types::CertificateDer::from(certificate_bytes.as_slice().to_vec());
    Ok(NativeClientIdentity { paths, certificate })
}

pub fn provision_credentials(home: &Path) -> Result<ForgeCredentialPaths, ForgeCredentialError> {
    provision_or_load(home)
}

pub fn ensure_credentials(home: &Path) -> Result<ForgeCredentialPaths, ForgeCredentialError> {
    provision_or_load(home)
}

#[cfg(test)]
mod client_credentials_tests {
    use std::fs;

    use super::certificates::{validate_cert_sans, validate_key_matches_cert};
    use super::storage::{
        FileId, MAX_CAPABILITY_BYTES, MAX_CERTIFICATE_BYTES, MAX_CERTIFICATE_READ_BYTES,
        MAX_MANIFEST_BYTES, MAX_PRIVATE_KEY_BYTES, open_and_read_bounded,
        private_material_identity_chain_matches,
    };
    #[cfg(unix)]
    use super::storage::{
        MAX_MANIFEST_READ_BYTES, read_private_material, validate_private_directory,
    };
    use super::validation::{classify_certificate_length, validate_existing_bundle};
    use super::*;

    #[test]
    fn load_returns_exact_leaf_capability_and_paths() {
        let home = tempfile::tempdir().expect("temporary credential home");
        let expected_paths = provision_or_load(home.path()).expect("provision credentials");
        let expected_certificate =
            fs::read(expected_paths.certificate_paths()[0].as_path()).expect("read certificate");
        let capability_bytes = Zeroizing::new(
            fs::read(expected_paths.capability_path()).expect("read capability fixture"),
        );
        let mut expected_capability_bytes = Zeroizing::new([0_u8; MAX_CAPABILITY_BYTES]);
        expected_capability_bytes[..].copy_from_slice(capability_bytes.as_slice());
        let expected_capability =
            artisan_protocol::LocalCapability::from_bytes(*expected_capability_bytes);

        let loaded = load_client_credentials(home.path()).expect("load credentials");
        assert_eq!(loaded.paths(), &expected_paths);
        let (certificate, capability) = loaded.into_parts();
        assert_eq!(certificate.as_ref(), expected_certificate.as_slice());
        assert!(capability.constant_time_eq(&expected_capability));
    }

    #[test]
    fn capability_length_is_exact_with_zeroizing_material() {
        let path = Path::new("capability.bin");
        for (length, valid) in [(31, false), (32, true), (33, false)] {
            let material = Zeroizing::new(vec![0xa5_u8; length]);
            let result = local_capability_from_bytes(material.as_slice(), path);
            if valid {
                assert!(result.is_ok());
            } else {
                assert!(matches!(
                    result,
                    Err(ForgeCredentialError::InvalidCapability { .. })
                ));
            }
        }
    }

    #[test]
    fn certificate_length_classifier_is_bounded_without_parsing_fixture_bytes() {
        assert!(matches!(
            classify_certificate_length(0),
            Err(ForgeCredentialError::InvalidCertificate)
        ));
        assert!(classify_certificate_length(MAX_CERTIFICATE_BYTES).is_ok());
        assert!(matches!(
            classify_certificate_length(MAX_CERTIFICATE_BYTES + 1),
            Err(ForgeCredentialError::InvalidCertificate)
        ));
    }

    #[test]
    fn bundle_validation_enforces_manifest_and_private_key_bounds() {
        let manifest_home = tempfile::tempdir().expect("temporary manifest home");
        let manifest_paths =
            provision_or_load(manifest_home.path()).expect("provision manifest fixture");
        fs::write(
            manifest_paths.manifest_path(),
            vec![b'm'; MAX_MANIFEST_BYTES + 1],
        )
        .expect("write oversized manifest fixture");
        assert!(matches!(
            validate_existing_bundle(&manifest_paths),
            Err(ForgeCredentialError::ManifestMalformed)
        ));

        let key_home = tempfile::tempdir().expect("temporary key home");
        let key_paths = provision_or_load(key_home.path()).expect("provision key fixture");
        fs::write(
            key_paths.private_key_path(),
            vec![b'k'; MAX_PRIVATE_KEY_BYTES + 1],
        )
        .expect("write oversized private key fixture");
        assert!(matches!(
            validate_existing_bundle(&key_paths),
            Err(ForgeCredentialError::InvalidCertificate)
        ));
    }

    #[test]
    fn file_identity_decision_rejects_mismatch_at_every_chain_position() {
        let chain = [7_u8; 7];
        assert!(private_material_identity_chain_matches(&chain));
        for position in 0..chain.len() {
            let mut mismatch = chain;
            mismatch[position] = 6;
            assert!(
                !private_material_identity_chain_matches(&mismatch),
                "identity mismatch at chain position {position}"
            );
        }
    }

    #[test]
    fn bounded_reader_rejects_directories() {
        let root = tempfile::tempdir().expect("temporary reader home");
        let directory = root.path().join("directory");
        fs::create_dir(&directory).expect("create directory fixture");
        assert!(matches!(
            open_and_read_bounded(
                &directory,
                MAX_CERTIFICATE_READ_BYTES,
                ForgeCredentialError::InvalidCertificate,
                FileId::default(),
            ),
            Err(ForgeCredentialError::UnsafePath(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn bounded_reader_rejects_symlink_targets_and_unsafe_ancestors() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("temporary reader home");
        let real_file = root.path().join("real-file");
        fs::write(&real_file, b"safe").expect("write real file");
        let symlink_file = root.path().join("symlink-file");
        symlink(&real_file, &symlink_file).expect("create symlink fixture");
        assert!(matches!(
            open_and_read_bounded(
                &symlink_file,
                MAX_CERTIFICATE_READ_BYTES,
                ForgeCredentialError::InvalidCertificate,
                FileId::default(),
            ),
            Err(ForgeCredentialError::UnsafePath(_))
        ));

        let real_directory = root.path().join("real-directory");
        fs::create_dir(&real_directory).expect("create real directory");
        let nested_file = real_directory.join("nested-file");
        fs::write(&nested_file, b"safe").expect("write nested file");
        let symlink_directory = root.path().join("symlink-directory");
        symlink(&real_directory, &symlink_directory).expect("create ancestor symlink");
        let substituted_path = symlink_directory.join("nested-file");
        assert!(matches!(
            open_and_read_bounded(
                &substituted_path,
                MAX_CERTIFICATE_READ_BYTES,
                ForgeCredentialError::InvalidCertificate,
                FileId::default(),
            ),
            Err(ForgeCredentialError::UnsafePath(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn existing_private_modes_fail_closed_without_repair() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().expect("temporary mode home");
        let paths = ForgeCredentialPaths::new(root.path()).expect("temporary mode paths");
        let credentials = paths.credentials_dir();
        fs::create_dir(&credentials).expect("create credentials directory");
        fs::set_permissions(&credentials, fs::Permissions::from_mode(0o755))
            .expect("set unsafe directory mode");
        assert!(matches!(
            validate_private_directory(&credentials),
            Err(ForgeCredentialError::WindowsAcl)
        ));
        assert_eq!(
            fs::symlink_metadata(&credentials)
                .expect("inspect directory mode")
                .permissions()
                .mode()
                & 0o777,
            0o755
        );

        fs::set_permissions(&credentials, fs::Permissions::from_mode(0o700))
            .expect("restore directory mode");
        let file = paths.manifest_path().to_path_buf();
        fs::write(&file, b"material").expect("write material");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644))
            .expect("set unsafe file mode");
        assert!(matches!(
            read_private_material(
                &paths,
                &file,
                MAX_MANIFEST_READ_BYTES,
                ForgeCredentialError::ManifestMalformed,
            ),
            Err(ForgeCredentialError::WindowsAcl)
        ));
        assert_eq!(
            fs::symlink_metadata(&file)
                .expect("inspect file mode")
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
    }

    #[test]
    fn credential_material_errors_redact_all_injected_canaries() {
        const CAPABILITY_CANARY: &[u8] = b"bootstrap-capability-bytes-canary";
        const CERTIFICATE_CANARY: &[u8] = b"certificate-bytes-canary";
        const KEY_CANARY: &[u8] = b"private-key-bytes-canary";
        let Err(capability_error) = local_capability_from_bytes(
            Zeroizing::new(CAPABILITY_CANARY.to_vec()).as_slice(),
            Path::new("capability.bin"),
        ) else {
            panic!("canary capability must fail length validation");
        };
        let certificate_error =
            validate_cert_sans(CERTIFICATE_CANARY).expect_err("canary certificate must fail");
        let key_error = validate_key_matches_cert(KEY_CANARY, CERTIFICATE_CANARY)
            .expect_err("canary key must fail parsing");

        for error in [capability_error, certificate_error, key_error] {
            let display = error.to_string();
            let debug = format!("{error:?}");
            for canary in [
                "bootstrap-capability-bytes-canary",
                "certificate-bytes-canary",
                "private-key-bytes-canary",
            ] {
                assert!(!display.contains(canary));
                assert!(!debug.contains(canary));
            }
        }
    }
}
