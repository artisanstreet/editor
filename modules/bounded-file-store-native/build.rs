use std::env;

fn main() {
    let target = env::var("TARGET").expect("Cargo did not provide TARGET");

    println!("cargo:rustc-env=ARTISAN_NATIVE_TARGET={target}");
    napi_build::setup();
}
