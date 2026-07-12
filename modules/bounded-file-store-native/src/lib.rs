use std::sync::{Arc, Mutex};

use napi::{
    Error, Result, Task,
    bindgen_prelude::{AsyncTask, Buffer},
};
use napi_derive::napi;

mod windows;

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

struct RootState {
    handle: Mutex<Option<Arc<windows::RootHandle>>>,
}

#[napi]
pub struct NativeBoundedRegularFileStore {
    root: Arc<RootState>,
}

#[napi]
impl NativeBoundedRegularFileStore {
    #[napi(constructor)]
    pub fn new(root_directory: String) -> Result<Self> {
        let root_handle = windows::open_root(&root_directory)?;

        Ok(Self {
            root: Arc::new(RootState {
                handle: Mutex::new(Some(Arc::new(root_handle))),
            }),
        })
    }

    #[napi(ts_return_type = "Promise<Uint8Array>")]
    pub fn read_regular_file(
        &self,
        relative_path: String,
        maximum_bytes: f64,
    ) -> Result<AsyncTask<ReadRegularFileTask>> {
        windows::validate_relative_path(&relative_path)?;
        let maximum_bytes = validate_maximum_bytes(maximum_bytes)?;

        let root_handle = self
            .root
            .handle
            .lock()
            .map_err(|_| native_error("native file store is unavailable"))?
            .as_ref()
            .cloned()
            .ok_or_else(|| native_error("native file store is closed"))?;

        Ok(AsyncTask::new(ReadRegularFileTask {
            root_handle,
            relative_path,
            maximum_bytes,
        }))
    }

    #[napi]
    pub fn close(&self) {
        if let Ok(mut root_handle) = self.root.handle.lock() {
            root_handle.take();
        }
    }
}

pub struct ReadRegularFileTask {
    root_handle: Arc<windows::RootHandle>,
    relative_path: String,
    maximum_bytes: u32,
}

impl Task for ReadRegularFileTask {
    type Output = Vec<u8>;
    type JsValue = Buffer;

    fn compute(&mut self) -> Result<Self::Output> {
        windows::read_regular_file(&self.root_handle, &self.relative_path, self.maximum_bytes)
    }

    fn resolve(&mut self, _: napi::Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(Buffer::from(output))
    }
}

fn native_error(reason: &'static str) -> Error {
    Error::from_reason(reason)
}

fn validate_maximum_bytes(maximum_bytes: f64) -> Result<u32> {
    if !maximum_bytes.is_finite()
        || maximum_bytes <= 0.0
        || maximum_bytes.fract() != 0.0
        || maximum_bytes > u32::MAX as f64
    {
        return Err(native_error("maximum bytes is invalid"));
    }

    Ok(maximum_bytes as u32)
}
