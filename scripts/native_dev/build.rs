//! Records the compile target so staged payloads can name the target their
//! binaries were built for (the runner is always built alongside them).

fn main() {
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_owned());
    println!("cargo:rustc-env=ARTISAN_NATIVE_DEV_TARGET={target}");
    println!("cargo:rerun-if-changed=build.rs");
}
