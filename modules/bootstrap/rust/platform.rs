use std::path::PathBuf;

use crate::error::{BootstrapError, Result};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Platform {
    pub os: &'static str,
    pub arch: &'static str,
}

impl Platform {
    pub fn detect() -> Result<Self> {
        let os = match std::env::consts::OS {
            "windows" => "windows",
            "macos" => "macos",
            "linux" => "linux",
            other => return Err(BootstrapError::UnsupportedPlatform(other.to_owned())),
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            other => return Err(BootstrapError::UnsupportedPlatform(other.to_owned())),
        };
        Ok(Self { os, arch })
    }

    pub fn target(&self) -> String {
        format!("{}-{}", self.os, self.arch)
    }

	pub fn default_install_root() -> PathBuf {
        #[cfg(windows)]
        {
			std::env::var_os("LOCALAPPDATA")
				.map_or_else(std::env::temp_dir, PathBuf::from)
                .join("Artisan")
        }
        #[cfg(not(windows))]
        {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
                .join(".local")
                .join("share")
                .join("artisan")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Platform;

    #[test]
    fn signed_linux_product_target_uses_glibc_name() {
        let platform = Platform {
            os: "linux",
            arch: "x64",
        };
        assert_eq!(platform.target(), "linux-x64");
        assert_eq!(crate::install::platform_libc(), if cfg!(target_env = "musl") {
            "musl"
        } else {
            "glibc"
        });
    }
}
