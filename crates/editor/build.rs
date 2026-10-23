//! The commit (from `EDITOR_COMMIT`, e.g. set by the release build, else
//! git) and the target triple Settings → About shows; on Windows, the app
//! icon and version information as the executable's resources.
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "a build script fails the build by panicking, with what went wrong"
)]

fn main() {
    println!("cargo:rerun-if-env-changed=EDITOR_COMMIT");
    for metadata in ["HEAD", "packed-refs"] {
        watch_git_path(metadata);
    }
    if let Ok(output) = std::process::Command::new("git")
        .args(["symbolic-ref", "-q", "HEAD"])
        .output()
        && output.status.success()
    {
        watch_git_path(String::from_utf8_lossy(&output.stdout).trim());
    }
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
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resources();
    }
}

/// Links `assets/artisan-editor.ico` as icon resource 1 and the version
/// information. GPUI loads icon 1 for the window class, so it is the taskbar
/// and Alt+Tab icon too, and the Start menu shortcut points at the
/// executable's first icon. Task Manager names the process by its
/// `FileDescription`.
fn embed_resources() {
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let icon = manifest.join("assets/artisan-editor.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    println!("cargo:rerun-if-env-changed=TARGET_WINDRES");
    println!("cargo:rerun-if-env-changed=TARGET_CC");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let script = out.join("resources.rc");
    let object = out.join("resources.o");
    let path = icon.display().to_string().replace('\\', "/");
    let version = env!("CARGO_PKG_VERSION");
    let numeric = [
        env!("CARGO_PKG_VERSION_MAJOR"),
        env!("CARGO_PKG_VERSION_MINOR"),
        env!("CARGO_PKG_VERSION_PATCH"),
        "0",
    ]
    .join(",");
    std::fs::write(
        &script,
        format!(
            "1 ICON \"{path}\"\n\
             1 VERSIONINFO\n\
             FILEVERSION {numeric}\n\
             PRODUCTVERSION {numeric}\n\
             BEGIN\n\
             BLOCK \"StringFileInfo\"\n\
             BEGIN\n\
             BLOCK \"040904B0\"\n\
             BEGIN\n\
             VALUE \"CompanyName\", \"Artisan Street\"\n\
             VALUE \"FileDescription\", \"Artisan Editor\"\n\
             VALUE \"FileVersion\", \"{version}\"\n\
             VALUE \"OriginalFilename\", \"Artisan Editor.exe\"\n\
             VALUE \"ProductName\", \"Artisan Editor\"\n\
             VALUE \"ProductVersion\", \"{version}\"\n\
             END\n\
             END\n\
             BLOCK \"VarFileInfo\"\n\
             BEGIN\n\
             VALUE \"Translation\", 0x409, 1200\n\
             END\n\
             END\n"
        ),
    )
    .expect("write resources.rc");
    let windres =
        std::env::var("TARGET_WINDRES").unwrap_or_else(|_| "x86_64-w64-mingw32-windres".to_owned());
    // windres preprocesses with `gcc`, which the cross toolchain names by
    // its target prefix; hand it the target compiler directly.
    let preprocessor =
        std::env::var("TARGET_CC").unwrap_or_else(|_| "x86_64-w64-mingw32-cc".to_owned());
    let status = std::process::Command::new(&windres)
        .arg(format!("--preprocessor={preprocessor}"))
        .args(["--preprocessor-arg=-E", "--preprocessor-arg=-xc"])
        .arg("--preprocessor-arg=-DRC_INVOKED")
        .arg(&script)
        .arg("-O")
        .arg("coff")
        .arg("-o")
        .arg(&object)
        .status()
        .unwrap_or_else(|error| panic!("run {windres} for the app resources: {error}"));
    assert!(status.success(), "{windres} failed on {}", script.display());
    println!("cargo:rustc-link-arg-bins={}", object.display());
}

fn watch_git_path(name: &str) {
    if let Ok(output) = std::process::Command::new("git")
        .args(["rev-parse", "--git-path", name])
        .output()
        && output.status.success()
    {
        println!(
            "cargo:rerun-if-changed={}",
            String::from_utf8_lossy(&output.stdout).trim()
        );
    }
}
