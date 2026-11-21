use std::path::Path;

#[cfg(unix)]
pub fn install(from: &Path, to: &Path) {
    let copied = std::process::Command::new("cp")
        .arg(from)
        .arg(to)
        .status()
        .unwrap_or_else(|error| panic!("run cp to install {}: {error}", to.display()));
    assert!(
        copied.success(),
        "cp {} {} failed with {copied}",
        from.display(),
        to.display()
    );
}

#[cfg(not(unix))]
pub fn install(from: &Path, to: &Path) {
    std::fs::copy(from, to)
        .unwrap_or_else(|error| panic!("install {} at {}: {error}", from.display(), to.display()));
}
