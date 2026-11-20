use std::path::{Path, PathBuf};
use std::process::Command;

pub fn isolate(command: &mut Command, sandbox: &Path) {
    command
        .env("HOME", sandbox.join("home"))
        .env("XDG_DATA_HOME", sandbox.join("data"))
        .env("XDG_CONFIG_HOME", sandbox.join("config"));
    if cfg!(windows) {
        command
            .env("USERPROFILE", sandbox.join("home"))
            .env("LOCALAPPDATA", sandbox.join("local"))
            .env("APPDATA", sandbox.join("roaming"));
    }
}

pub fn artisan_street(sandbox: &Path) -> PathBuf {
    if cfg!(windows) {
        sandbox.join("local/Artisan Street")
    } else if cfg!(target_os = "macos") {
        sandbox.join("home/Library/Application Support/Artisan Street")
    } else {
        sandbox.join("data/artisan-street")
    }
}
