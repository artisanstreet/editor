//! Atomic readiness receipt publication, validation, and identity-bound
//! removal.

use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use artisan_transport::PinnedIdentity;
use thiserror::Error;

use super::{READY_SCHEMA, configure_no_reparse_open, is_reparse_point, is_required_loopback};
use crate::file_identity_policy::{FileIdentity, read_file_identity, same_file_identity};

/// Typed readiness receipt failures. All variants are payload-free with
/// respect to credential bytes; only the explicit readiness path and safe
/// endpoint metadata can appear in diagnostics.
#[derive(Debug, Error)]
pub enum ReadinessError {
    /// The listener address was not the required IPv4 loopback endpoint.
    #[error("Forge listener did not bind the required IPv4 loopback endpoint")]
    InvalidEndpoint { address: SocketAddr },

    /// The readiness parent could not be inspected.
    #[error("failed to inspect readiness parent at {path}")]
    InspectParent {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// The readiness parent was absent.
    #[error("readiness parent is missing at {path}")]
    ParentMissing { path: PathBuf },

    /// The readiness parent was not a directory.
    #[error("readiness parent is not a directory at {path}")]
    ParentNotDirectory { path: PathBuf },

    /// The readiness parent was a symbolic link.
    #[error("readiness parent is a symbolic link at {path}")]
    ParentSymlink { path: PathBuf },

    /// The readiness parent was a Windows reparse point.
    #[error("readiness parent is a reparse point at {path}")]
    ParentReparsePoint { path: PathBuf },

    /// The readiness target already existed.
    #[error("readiness target already exists at {path}")]
    TargetExists { path: PathBuf },

    /// The readiness target was a symbolic link.
    #[error("readiness target is a symbolic link at {path}")]
    TargetSymlink { path: PathBuf },

    /// The readiness target was a Windows reparse point.
    #[error("readiness target is a reparse point at {path}")]
    TargetReparsePoint { path: PathBuf },

    /// The readiness target could not be inspected.
    #[error("failed to inspect readiness target at {path}")]
    InspectTarget {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// Creation of a same-directory private temporary file failed.
    #[error("failed to create private readiness temporary file at {path}")]
    CreateTemporary {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// Writing or synchronizing the private temporary file failed.
    #[error("failed to write readiness temporary file at {path}")]
    WriteTemporary {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// Atomic no-overwrite installation failed.
    #[error("failed to install readiness receipt at {path}")]
    Install {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// A cleanup operation on a private temporary file failed.
    #[error("failed to remove readiness temporary file at {path}")]
    TemporaryCleanup {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// The primary readiness operation and its private-temporary cleanup
    /// both failed. Neither typed failure is discarded.
    #[error("readiness operation failed ({primary}); cleanup also failed ({cleanup})")]
    Cleanup {
        #[source]
        primary: Box<Self>,
        cleanup: Box<Self>,
    },

    /// The target changed after this runtime installed its receipt.
    #[error("readiness target changed before cleanup at {path}")]
    TargetReplaced { path: PathBuf },

    /// Removing this runtime's receipt failed.
    #[error("failed to remove readiness receipt at {path}")]
    Remove {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Publishes one compact JSON readiness receipt through a same-directory
/// private file and an atomic hard-link installation. Hard-link installation
/// fails if the target appeared after the initial inspection, so it cannot
/// overwrite an existing target.
pub(super) struct ReadinessReceipt {
    path: PathBuf,
    identity: FileIdentity,
    contents: Vec<u8>,
    temporary: PathBuf,
}

impl ReadinessReceipt {
    pub(super) fn publish(
        path: &Path,
        address: SocketAddr,
        identity: PinnedIdentity,
    ) -> Result<Self, ReadinessError> {
        if !is_required_loopback(address) {
            return Err(ReadinessError::InvalidEndpoint { address });
        }
        let parent = path.parent().ok_or_else(|| ReadinessError::ParentMissing {
            path: path.to_path_buf(),
        })?;
        validate_readiness_parent_chain(parent)?;
        inspect_readiness_target(path)?;

        let body = readiness_json(address, identity);
        let (temporary, temporary_identity) = write_readiness_temporary(parent, &body)?;

        if let Err(source) = fs::hard_link(&temporary, path) {
            let error = if source.kind() == io::ErrorKind::AlreadyExists {
                ReadinessError::TargetExists {
                    path: path.to_path_buf(),
                }
            } else {
                ReadinessError::Install {
                    path: path.to_path_buf(),
                    source,
                }
            };
            return Err(with_temporary_identity_cleanup(
                &temporary,
                temporary_identity,
                error,
            ));
        }
        // The hard link is now installed. Construct the cleanup owner before
        // opening or inspecting the target so every later failure has a
        // recorded identity-bound owner to clean up.
        let receipt = Self {
            path: path.to_path_buf(),
            identity: temporary_identity,
            contents: body.into_bytes(),
            temporary,
        };
        match receipt.validate_published_target() {
            Ok(()) => Ok(receipt),
            Err(primary) => Err(receipt.cleanup_after_error(primary)),
        }
    }

    pub(super) fn remove(self) -> Result<(), ReadinessError> {
        // Both owners are attempted independently. A target replacement must
        // never prevent removal of this run's private temporary hard link.
        let target_failure = remove_owned_target(&self.path, self.identity, &self.contents).err();
        let temporary_failure = remove_owned_temporary(&self.temporary, self.identity).err();

        match (target_failure, temporary_failure) {
            (None, None) => Ok(()),
            (Some(failure), None) | (None, Some(failure)) => Err(failure),
            (Some(primary), Some(cleanup)) => Err(ReadinessError::Cleanup {
                primary: Box::new(primary),
                cleanup: Box::new(cleanup),
            }),
        }
    }

    fn validate_published_target(&self) -> Result<(), ReadinessError> {
        let Some(mut target) = open_readiness_target(&self.path)? else {
            return Err(ReadinessError::TargetReplaced {
                path: self.path.clone(),
            });
        };
        let target_identity =
            read_file_identity(&target).map_err(|source| ReadinessError::InspectTarget {
                path: self.path.clone(),
                source,
            })?;
        if !same_file_identity(target_identity, self.identity)
            || !readiness_contents_match(&mut target, &self.contents).map_err(|source| {
                ReadinessError::InspectTarget {
                    path: self.path.clone(),
                    source,
                }
            })?
        {
            return Err(ReadinessError::TargetReplaced {
                path: self.path.clone(),
            });
        }
        Ok(())
    }

    fn cleanup_after_error(self, primary: ReadinessError) -> ReadinessError {
        match self.remove() {
            Ok(()) => primary,
            Err(cleanup) => ReadinessError::Cleanup {
                primary: Box::new(primary),
                cleanup: Box::new(cleanup),
            },
        }
    }
}

/// Opens an existing readiness entry only after checking its complete parent
/// chain and final path shape. The path is checked again after opening so the
/// returned handle is paired with the exact entry that was inspected.
fn open_readiness_target(path: &Path) -> Result<Option<File>, ReadinessError> {
    let parent = path.parent().ok_or_else(|| ReadinessError::ParentMissing {
        path: path.to_path_buf(),
    })?;
    validate_readiness_parent_chain(parent)?;

    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ReadinessError::InspectTarget {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if readiness_target_is_replaced(&metadata) {
        return Err(ReadinessError::TargetReplaced {
            path: path.to_path_buf(),
        });
    }

    let file = match open_readiness_file(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ReadinessError::InspectTarget {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let handle_metadata = file
        .metadata()
        .map_err(|source| ReadinessError::InspectTarget {
            path: path.to_path_buf(),
            source,
        })?;
    if readiness_target_is_replaced(&handle_metadata) {
        return Err(ReadinessError::TargetReplaced {
            path: path.to_path_buf(),
        });
    }

    match fs::symlink_metadata(path) {
        Ok(metadata) if readiness_target_is_replaced(&metadata) => {
            Err(ReadinessError::TargetReplaced {
                path: path.to_path_buf(),
            })
        }
        Ok(_) => Ok(Some(file)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ReadinessError::InspectTarget {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn remove_owned_target(
    path: &Path,
    expected_identity: FileIdentity,
    expected_contents: &[u8],
) -> Result<(), ReadinessError> {
    let Some(mut target) = open_readiness_target(path)? else {
        return Ok(());
    };
    let target_identity =
        read_file_identity(&target).map_err(|source| ReadinessError::InspectTarget {
            path: path.to_path_buf(),
            source,
        })?;
    if !same_file_identity(target_identity, expected_identity)
        || !readiness_contents_match(&mut target, expected_contents).map_err(|source| {
            ReadinessError::InspectTarget {
                path: path.to_path_buf(),
                source,
            }
        })?
    {
        return Err(ReadinessError::TargetReplaced {
            path: path.to_path_buf(),
        });
    }

    let Some(mut final_target) = open_readiness_target(path)? else {
        return Ok(());
    };
    let final_identity =
        read_file_identity(&final_target).map_err(|source| ReadinessError::InspectTarget {
            path: path.to_path_buf(),
            source,
        })?;
    if !same_file_identity(final_identity, expected_identity)
        || !readiness_contents_match(&mut final_target, expected_contents).map_err(|source| {
            ReadinessError::InspectTarget {
                path: path.to_path_buf(),
                source,
            }
        })?
    {
        return Err(ReadinessError::TargetReplaced {
            path: path.to_path_buf(),
        });
    }
    drop(final_target);
    drop(target);

    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(ReadinessError::Remove {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn remove_owned_temporary(
    path: &Path,
    expected_identity: FileIdentity,
) -> Result<(), ReadinessError> {
    let Some(file) = open_readiness_target(path)? else {
        return Ok(());
    };
    let identity =
        read_file_identity(&file).map_err(|source| ReadinessError::TemporaryCleanup {
            path: path.to_path_buf(),
            source,
        })?;
    if !same_file_identity(identity, expected_identity) {
        return Err(ReadinessError::TargetReplaced {
            path: path.to_path_buf(),
        });
    }

    let Some(final_file) = open_readiness_target(path)? else {
        return Ok(());
    };
    let final_identity =
        read_file_identity(&final_file).map_err(|source| ReadinessError::TemporaryCleanup {
            path: path.to_path_buf(),
            source,
        })?;
    if !same_file_identity(final_identity, expected_identity) {
        return Err(ReadinessError::TargetReplaced {
            path: path.to_path_buf(),
        });
    }
    drop(final_file);
    drop(file);

    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(ReadinessError::TemporaryCleanup {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn open_readiness_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    configure_no_reparse_open(&mut options);
    options.open(path)
}

fn write_readiness_temporary(
    parent: &Path,
    body: &str,
) -> Result<(PathBuf, FileIdentity), ReadinessError> {
    let (temporary, mut file) = create_readiness_temporary(parent)?;
    if let Err(source) = file.write_all(body.as_bytes()) {
        let temporary_path = temporary.clone();
        return Err(with_open_temporary_cleanup(
            temporary,
            file,
            ReadinessError::WriteTemporary {
                path: temporary_path,
                source,
            },
        ));
    }
    if let Err(source) = file.flush().and_then(|()| file.sync_all()) {
        let temporary_path = temporary.clone();
        return Err(with_open_temporary_cleanup(
            temporary,
            file,
            ReadinessError::WriteTemporary {
                path: temporary_path,
                source,
            },
        ));
    }
    let temporary_identity = match read_file_identity(&file) {
        Ok(identity) => identity,
        Err(source) => {
            let temporary_path = temporary.clone();
            return Err(with_open_temporary_cleanup(
                temporary,
                file,
                ReadinessError::WriteTemporary {
                    path: temporary_path,
                    source,
                },
            ));
        }
    };
    drop(file);
    Ok((temporary, temporary_identity))
}

fn create_readiness_temporary(parent: &Path) -> Result<(PathBuf, File), ReadinessError> {
    for _ in 0..32 {
        let candidate = temporary_path(parent);
        match create_private_temporary(&candidate) {
            Ok(file) => return Ok((candidate, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(source) => {
                return Err(ReadinessError::CreateTemporary {
                    path: candidate,
                    source,
                });
            }
        }
    }
    Err(ReadinessError::CreateTemporary {
        path: parent.to_path_buf(),
        source: io::Error::new(
            io::ErrorKind::AlreadyExists,
            "readiness temporary-name space is exhausted",
        ),
    })
}

fn readiness_contents_match(file: &mut File, expected: &[u8]) -> io::Result<bool> {
    let expected_length = u64::try_from(expected.len()).unwrap_or(u64::MAX);
    let limit = expected_length.saturating_add(1);
    let mut contents = Vec::with_capacity(expected.len().saturating_add(1));
    let mut limited = (&mut *file).take(limit);
    limited.read_to_end(&mut contents)?;
    Ok(contents == expected)
}

fn readiness_target_is_replaced(metadata: &Metadata) -> bool {
    metadata.file_type().is_symlink() || is_reparse_point(metadata) || !metadata.is_file()
}

fn with_open_temporary_cleanup(
    path: PathBuf,
    file: File,
    primary: ReadinessError,
) -> ReadinessError {
    let identity = match read_file_identity(&file) {
        Ok(identity) => identity,
        Err(source) => {
            drop(file);
            return ReadinessError::Cleanup {
                primary: Box::new(primary),
                cleanup: Box::new(ReadinessError::TemporaryCleanup { path, source }),
            };
        }
    };
    drop(file);
    with_temporary_identity_cleanup(&path, identity, primary)
}

fn with_temporary_identity_cleanup(
    path: &Path,
    identity: FileIdentity,
    primary: ReadinessError,
) -> ReadinessError {
    match remove_owned_temporary(path, identity) {
        Ok(()) => primary,
        Err(cleanup) => ReadinessError::Cleanup {
            primary: Box::new(primary),
            cleanup: Box::new(cleanup),
        },
    }
}

fn readiness_json(address: SocketAddr, identity: PinnedIdentity) -> String {
    format!(
        "{{\"schema\":\"{READY_SCHEMA}\",\"endpoint\":\"{address}\",\"certificate_sha256\":\"{}\",\"pid\":{}}}\n",
        identity.to_hex(),
        std::process::id(),
    )
}

fn validate_readiness_parent_chain(parent: &Path) -> Result<(), ReadinessError> {
    let mut current = parent;
    loop {
        let metadata = match fs::symlink_metadata(current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(ReadinessError::ParentMissing {
                    path: current.to_path_buf(),
                });
            }
            Err(source) => {
                return Err(ReadinessError::InspectParent {
                    path: current.to_path_buf(),
                    source,
                });
            }
        };
        if metadata.file_type().is_symlink() {
            return Err(ReadinessError::ParentSymlink {
                path: current.to_path_buf(),
            });
        }
        if is_reparse_point(&metadata) {
            return Err(ReadinessError::ParentReparsePoint {
                path: current.to_path_buf(),
            });
        }
        if !metadata.is_dir() {
            return Err(ReadinessError::ParentNotDirectory {
                path: current.to_path_buf(),
            });
        }
        let Some(next) = current.parent() else {
            break;
        };
        if next == current || next.as_os_str().is_empty() {
            break;
        }
        current = next;
    }
    Ok(())
}

fn inspect_readiness_target(path: &Path) -> Result<(), ReadinessError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(ReadinessError::TargetSymlink {
            path: path.to_path_buf(),
        }),
        Ok(metadata) if is_reparse_point(&metadata) => Err(ReadinessError::TargetReparsePoint {
            path: path.to_path_buf(),
        }),
        Ok(_) => Err(ReadinessError::TargetExists {
            path: path.to_path_buf(),
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(ReadinessError::InspectTarget {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn create_private_temporary(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }
    options.open(path)
}

static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn temporary_path(parent: &Path) -> PathBuf {
    let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(
        ".artisan-forge-ready-{}-{sequence}.tmp",
        std::process::id()
    ))
}
