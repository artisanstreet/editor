//! The commit the titlebar's build badge names (from `EDITOR_COMMIT`, e.g.
//! set by the release build, else git), and the target triple Settings →
//! About shows.

fn main() {
    println!("cargo:rerun-if-env-changed=EDITOR_COMMIT");
    let commit = std::env::var("EDITOR_COMMIT")
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
        "cargo:rustc-env=EDITOR_COMMIT={}",
        commit.as_deref().unwrap_or("")
    );
    println!(
        "cargo:rustc-env=EDITOR_TARGET={}",
        std::env::var("TARGET").unwrap_or_default()
    );
}
