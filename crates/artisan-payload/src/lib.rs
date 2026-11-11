use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use std::io::{Result, Write as _};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy)]
pub struct Payload<'a>(&'a [u8]);

impl<'a> Payload<'a> {
    #[must_use]
    pub const fn new(packed: &'a [u8]) -> Self {
        Self(packed)
    }

    #[must_use]
    pub const fn present(&self) -> bool {
        !self.0.is_empty()
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        let mut file = std::fs::File::create(path)?;
        std::io::copy(&mut GzDecoder::new(self.0), &mut file)?;
        file.flush()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
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

pub fn pack(program: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(program)?;
    encoder.finish()
}

pub fn embed(variable: &str, name: &str) -> Result<()> {
    println!("cargo:rerun-if-env-changed={variable}");
    let packed = match std::env::var_os(variable).filter(|path| !path.is_empty()) {
        Some(path) => {
            println!("cargo:rerun-if-changed={}", Path::new(&path).display());
            pack(&std::fs::read(&path)?)?
        }
        None => Vec::new(),
    };
    let out = std::env::var_os("OUT_DIR")
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "OUT_DIR is not set"))?;
    std::fs::write(PathBuf::from(out).join(name), packed)
}

#[cfg(test)]
#[path = "../tests/payload/roundtrip.rs"]
mod tests;
