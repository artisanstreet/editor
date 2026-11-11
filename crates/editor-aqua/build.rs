use std::{
    env,
    error::Error,
    path::{Path, PathBuf},
    process::Command,
};

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-env-changed=EDITOR_COMMIT");
    for metadata in ["HEAD", "packed-refs"] {
        watch_git_path(metadata);
    }
    if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watch_git_path(&reference);
    }
    let commit = env::var("EDITOR_COMMIT")
        .ok()
        .filter(|commit| !commit.is_empty())
        .or_else(|| git(&["rev-parse", "--short=10", "HEAD"]))
        .unwrap_or_default();
    println!("cargo:rustc-env=EDITOR_COMMIT={commit}");
    println!(
        "cargo:rustc-env=EDITOR_TARGET={}",
        env::var("TARGET").unwrap_or_default()
    );
    artisan_payload::embed("ARTISAN_AE_PAYLOAD", "ae.gz")?;
    artisan_payload::embed("ARTISAN_AE_LINUX_PAYLOAD", "ae-linux.gz")?;
    embed_notices()?;
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resources()?;
    }
    Ok(())
}

fn embed_notices() -> Result<(), Box<dyn Error>> {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?).join("../..");
    let licenses = root.join("crates/editor-assets/licenses");
    println!("cargo:rerun-if-changed={}", licenses.display());
    let mut files = vec![root.join("LICENSE.md")];
    let mut bundled: Vec<PathBuf> = std::fs::read_dir(&licenses)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<_, _>>()?;
    bundled.sort();
    files.extend(bundled);
    files.push(root.join("vendor/highlighting/NOTICE.txt"));
    files.push(root.join("vendor/mermaid-rs-renderer/LICENSE"));
    let mut notices = Vec::new();
    for file in files {
        println!("cargo:rerun-if-changed={}", file.display());
        let name = file
            .strip_prefix(&root)?
            .display()
            .to_string()
            .replace('\\', "/");
        notices.extend_from_slice(format!("==== {name} ====\n\n").as_bytes());
        notices.extend_from_slice(&std::fs::read(&file)?);
        notices.extend_from_slice(b"\n\n");
    }
    let out = PathBuf::from(env::var("OUT_DIR")?);
    std::fs::write(out.join("notices.gz"), artisan_payload::pack(&notices)?)?;
    Ok(())
}

fn git(arguments: &[&str]) -> Option<String> {
    let output = Command::new("git").args(arguments).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn watch_git_path(name: &str) {
    if let Some(path) = git(&["rev-parse", "--git-path", name]) {
        println!("cargo:rerun-if-changed={path}");
    }
}

fn resource_script(icon: &Path) -> String {
    let path = icon.display().to_string().replace('\\', "/");
    let version = env!("CARGO_PKG_VERSION");
    let numeric = [
        env!("CARGO_PKG_VERSION_MAJOR"),
        env!("CARGO_PKG_VERSION_MINOR"),
        env!("CARGO_PKG_VERSION_PATCH"),
        "0",
    ]
    .join(",");
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
    )
}

fn embed_resources() -> Result<(), Box<dyn Error>> {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let icon = manifest.join("../editor-assets/app/artisan-editor.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    println!("cargo:rerun-if-env-changed=TARGET_WINDRES");
    println!("cargo:rerun-if-env-changed=TARGET_CC");
    let out = PathBuf::from(env::var("OUT_DIR")?);
    let script = out.join("resources.rc");
    std::fs::write(&script, resource_script(&icon))?;
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        embed_resource::compile(&script, embed_resource::NONE).manifest_required()?;
        return Ok(());
    }
    let object = out.join("resources.o");
    let windres =
        env::var("TARGET_WINDRES").unwrap_or_else(|_| "x86_64-w64-mingw32-windres".to_owned());
    let preprocessor = env::var("TARGET_CC").unwrap_or_else(|_| "x86_64-w64-mingw32-cc".to_owned());
    let status = Command::new(&windres)
        .arg(format!("--preprocessor={preprocessor}"))
        .args(["--preprocessor-arg=-E", "--preprocessor-arg=-xc"])
        .arg("--preprocessor-arg=-DRC_INVOKED")
        .arg(&script)
        .args(["-O", "coff", "-o"])
        .arg(&object)
        .status()?;
    if !status.success() {
        return Err(format!("{windres} failed on {}", script.display()).into());
    }
    println!("cargo:rustc-link-arg-bins={}", object.display());
    Ok(())
}
