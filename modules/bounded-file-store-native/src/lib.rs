use napi_derive::napi;

#[napi(object)]
pub struct NativeBuildDescriptor {
    pub architecture: String,
    pub operating_system: String,
    pub target: String,
}

#[napi]
pub fn get_native_build_descriptor() -> NativeBuildDescriptor {
    NativeBuildDescriptor {
        architecture: std::env::consts::ARCH.to_owned(),
        operating_system: std::env::consts::OS.to_owned(),
        target: env!("ARTISAN_NATIVE_TARGET").to_owned(),
    }
}
