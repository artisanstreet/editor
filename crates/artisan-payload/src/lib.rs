pub mod licences;

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use std::io::Write as _;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum WritePayloadError {
    #[error("this build carries nothing to write to {}", path.display())]
    Absent { path: PathBuf },
    #[error("create {}", path.display())]
    Create {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("unpack into {}", path.display())]
    Unpack {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("make {} runnable", path.display())]
    MakeRunnable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
#[error("compress the program")]
pub struct PackError(#[source] std::io::Error);

#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    #[error("read the program {variable} names, {}", path.display())]
    ReadProgram {
        variable: String,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("pack the program {variable} names, {}", path.display())]
    Pack {
        variable: String,
        path: PathBuf,
        #[source]
        source: PackError,
    },
    #[error("OUT_DIR is not set")]
    OutDirUnset,
    #[error("write the payload {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Clone, Copy)]
pub struct Payload<'a>(&'a [u8]);

impl std::fmt::Debug for Payload<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Payload")
            .field("packed_bytes", &self.0.len())
            .finish()
    }
}

impl<'a> Payload<'a> {
    #[must_use]
    pub const fn new(packed: &'a [u8]) -> Self {
        Self(packed)
    }

    #[must_use]
    pub const fn present(&self) -> bool {
        !self.0.is_empty()
    }

    pub fn write(&self, path: &Path) -> Result<(), WritePayloadError> {
        if !self.present() {
            return Err(WritePayloadError::Absent {
                path: path.to_owned(),
            });
        }
        let mut file = std::fs::File::create(path).map_err(|source| WritePayloadError::Create {
            path: path.to_owned(),
            source,
        })?;
        std::io::copy(&mut GzDecoder::new(self.0), &mut file)
            .and_then(|_| file.flush())
            .map_err(|source| WritePayloadError::Unpack {
                path: path.to_owned(),
                source,
            })
    }

    pub fn write_program(&self, path: &Path) -> Result<(), WritePayloadError> {
        self.write(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).map_err(
                |source| WritePayloadError::MakeRunnable {
                    path: path.to_owned(),
                    source,
                },
            )?;
        }
        Ok(())
    }
}

#[macro_export]
macro_rules! payload {
    ($name:literal) => {
        $crate::Payload::new(include_bytes!(concat!(env!("OUT_DIR"), "/", $name)))
    };
}

fn pack(program: &[u8]) -> Result<Vec<u8>, PackError> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(program).map_err(PackError)?;
    encoder.finish().map_err(PackError)
}

#[expect(
    clippy::print_stdout,
    reason = "Cargo reads build-script directives from stdout"
)]
fn cargo_directive(directive: std::fmt::Arguments<'_>) {
    println!("cargo:{directive}");
}

pub fn embed(variable: &str, name: &str) -> Result<(), EmbedError> {
    cargo_directive(format_args!("rerun-if-env-changed={variable}"));
    let packed = match std::env::var_os(variable).filter(|path| !path.is_empty()) {
        Some(path) => {
            let path = PathBuf::from(path);
            cargo_directive(format_args!("rerun-if-changed={}", path.display()));
            let program = std::fs::read(&path).map_err(|source| EmbedError::ReadProgram {
                variable: variable.to_owned(),
                path: path.clone(),
                source,
            })?;
            pack(&program).map_err(|source| EmbedError::Pack {
                variable: variable.to_owned(),
                path,
                source,
            })?
        }
        None => Vec::new(),
    };
    let path =
        PathBuf::from(std::env::var_os("OUT_DIR").ok_or(EmbedError::OutDirUnset)?).join(name);
    std::fs::write(&path, packed).map_err(|source| EmbedError::Write { path, source })
}

#[derive(Debug, thiserror::Error)]
pub enum EmbedNoticesError {
    #[error("the notice {} is outside {}", file.display(), root.display())]
    Outside { file: PathBuf, root: PathBuf },
    #[error("read the notice {}", file.display())]
    Read {
        file: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("pack the notices")]
    Pack(#[source] PackError),
    #[error("OUT_DIR is not set")]
    OutDirUnset,
    #[error("write the notices {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub fn embed_notices(
    name: &str,
    root: &Path,
    files: &[PathBuf],
) -> Result<String, EmbedNoticesError> {
    let mut notices = String::new();
    for file in files {
        cargo_directive(format_args!("rerun-if-changed={}", file.display()));
        let label = file
            .strip_prefix(root)
            .map_err(|_outside| EmbedNoticesError::Outside {
                file: file.clone(),
                root: root.to_owned(),
            })?
            .display()
            .to_string()
            .replace('\\', "/");
        let text = std::fs::read_to_string(file).map_err(|source| EmbedNoticesError::Read {
            file: file.clone(),
            source,
        })?;
        notices.push_str("==== ");
        notices.push_str(&label);
        notices.push_str(" ====\n\n");
        notices.push_str(&text);
        notices.push_str("\n\n");
    }
    let path = PathBuf::from(std::env::var_os("OUT_DIR").ok_or(EmbedNoticesError::OutDirUnset)?)
        .join(name);
    let packed = pack(notices.as_bytes()).map_err(EmbedNoticesError::Pack)?;
    std::fs::write(&path, packed).map_err(|source| EmbedNoticesError::Write { path, source })?;
    Ok(notices)
}

#[cfg(test)]
#[path = "../tests/payload/roundtrip.rs"]
mod tests;
