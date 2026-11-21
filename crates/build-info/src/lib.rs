#![expect(
    clippy::print_stdout,
    reason = "cargo reads build script directives from stdout"
)]

use std::process::Command;

pub fn emit() {
    println!("cargo:rerun-if-env-changed=ARTISAN_COMMIT");
    for metadata in ["HEAD", "packed-refs"] {
        watch_git_path(metadata);
    }
    if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watch_git_path(&reference);
    }
    let commit = match std::env::var("ARTISAN_COMMIT") {
        Ok(commit) if !commit.is_empty() => Some(commit),
        Ok(_) | Err(_) => git(&["rev-parse", "--short=10", "HEAD"]),
    };
    println!(
        "cargo:rustc-env=ARTISAN_COMMIT={}",
        commit.as_deref().unwrap_or("unknown")
    );
    println!(
        "cargo:rustc-env=ARTISAN_TARGET={}",
        std::env::var("TARGET").expect("invariant: cargo sets TARGET for every build script")
    );
}

fn git(arguments: &[&str]) -> Option<String> {
    match Command::new("git").args(arguments).output() {
        Ok(output) if output.status.success() => {
            Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        }
        Ok(_) | Err(_) => None,
    }
}

fn watch_git_path(name: &str) {
    if let Some(path) = git(&["rev-parse", "--git-path", name]) {
        println!("cargo:rerun-if-changed={path}");
    }
}
