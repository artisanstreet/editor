//! Build info for `ae --version`: the commit (from `AE_COMMIT`, e.g. set by
//! the release build, else git) and the target triple.

fn main() {
    println!("cargo:rerun-if-env-changed=AE_COMMIT");
    let commit = std::env::var("AE_COMMIT")
        .ok()
        .filter(|commit| !commit.is_empty())
        .or_else(|| {
            let output = std::process::Command::new("git")
                .args(["rev-parse", "--short=10", "HEAD"])
                .output()
                .ok()?;
            output
                .status
                .success()
                .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        });
    println!(
        "cargo:rustc-env=AE_COMMIT={}",
        commit.as_deref().unwrap_or("unknown")
    );
    println!(
        "cargo:rustc-env=AE_TARGET={}",
        std::env::var("TARGET").unwrap_or_default()
    );
}
