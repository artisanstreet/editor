use std::sync::{Arc, Mutex};

use napi::{
    Error, Result, Task,
    bindgen_prelude::{AsyncTask, Buffer, Uint8Array},
};
use napi_derive::napi;
use zeroize::Zeroize;

mod windows;

#[napi(object)]
pub struct NativeBuildDescriptor {
    pub architecture: String,
    pub operating_system: String,
    pub target: String,
    pub test_hooks_enabled: bool,
}

#[napi(object)]
pub struct NativeReplaceRegularFileOptions {
    pub expected: Uint8Array,
    pub replacement: Uint8Array,
    pub maximum_bytes: f64,
    pub operation_id: String,
    pub path: String,
}

#[napi]
pub fn get_native_build_descriptor() -> NativeBuildDescriptor {
    NativeBuildDescriptor {
        architecture: std::env::consts::ARCH.to_owned(),
        operating_system: std::env::consts::OS.to_owned(),
        target: env!("ARTISAN_NATIVE_TARGET").to_owned(),
        test_hooks_enabled: cfg!(feature = "native-test-hooks"),
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
    pub fn new(root_directory: String, receipt_authentication_key: Uint8Array) -> Result<Self> {
        let mut copied_key = receipt_authentication_key.to_vec();

        if copied_key.len() != 32 {
            copied_key.zeroize();

            return Err(native_error("receipt authentication key is invalid"));
        }

        let mut receipt_authentication_key = [0; 32];

        receipt_authentication_key.copy_from_slice(&copied_key);
        copied_key.zeroize();

        let root_handle = windows::open_root(&root_directory, receipt_authentication_key)?;

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

    #[napi(ts_return_type = "Promise<\"Replaced\" | \"AlreadyReplaced\" | \"Changed\">")]
    pub fn replace_regular_file(
        &self,
        options: NativeReplaceRegularFileOptions,
    ) -> Result<AsyncTask<ReplaceRegularFileTask>> {
        let options = validate_replace_options(options)?;
        let root_handle = self.lease_root()?;

        Ok(AsyncTask::new(ReplaceRegularFileTask {
            root_handle,
            options,
        }))
    }

    #[napi(ts_return_type = "Promise<void>")]
    pub fn finalize_regular_file_replacement(
        &self,
        options: NativeReplaceRegularFileOptions,
    ) -> Result<AsyncTask<FinalizeRegularFileReplacementTask>> {
        let options = validate_replace_options(options)?;
        let root_handle = self.lease_root()?;

        Ok(AsyncTask::new(FinalizeRegularFileReplacementTask {
            root_handle,
            options,
        }))
    }

    #[napi(ts_return_type = "Promise<boolean>")]
    pub fn authorize_root(&self, candidate_root: String) -> Result<AsyncTask<AuthorizeRootTask>> {
        let root_handle = self.lease_root()?;

        Ok(AsyncTask::new(AuthorizeRootTask {
            candidate_root,
            root_handle,
        }))
    }

    #[napi]
    pub fn close(&self) {
        if let Ok(mut root_handle) = self.root.handle.lock() {
            root_handle.take();
        }
    }

    fn lease_root(&self) -> Result<Arc<windows::RootHandle>> {
        self.root
            .handle
            .lock()
            .map_err(|_| native_error("native file store is unavailable"))?
            .as_ref()
            .cloned()
            .ok_or_else(|| native_error("native file store is closed"))
    }
}

pub struct ReadRegularFileTask {
    root_handle: Arc<windows::RootHandle>,
    relative_path: String,
    maximum_bytes: u32,
}

pub struct ReplaceRegularFileTask {
    root_handle: Arc<windows::RootHandle>,
    options: ReplaceRegularFileOptions,
}

impl Task for ReplaceRegularFileTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> Result<Self::Output> {
        Ok(
            windows::replace_regular_file(&self.root_handle, &self.options)?
                .as_str()
                .to_owned(),
        )
    }

    fn resolve(&mut self, _: napi::Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

pub struct FinalizeRegularFileReplacementTask {
    root_handle: Arc<windows::RootHandle>,
    options: ReplaceRegularFileOptions,
}

pub struct AuthorizeRootTask {
    candidate_root: String,
    root_handle: Arc<windows::RootHandle>,
}

impl Task for FinalizeRegularFileReplacementTask {
    type Output = ();
    type JsValue = ();

    fn compute(&mut self) -> Result<Self::Output> {
        windows::finalize_regular_file_replacement(&self.root_handle, &self.options)
    }

    fn resolve(&mut self, _: napi::Env, _: Self::Output) -> Result<Self::JsValue> {
        Ok(())
    }
}

impl Task for AuthorizeRootTask {
    type Output = bool;
    type JsValue = bool;

    fn compute(&mut self) -> Result<Self::Output> {
        windows::authorize_root(&self.root_handle, &self.candidate_root)
    }

    fn resolve(&mut self, _: napi::Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

pub(crate) struct ReplaceRegularFileOptions {
    pub(crate) expected: Vec<u8>,
    pub(crate) replacement: Vec<u8>,
    pub(crate) maximum_bytes: u32,
    pub(crate) operation_id: String,
    pub(crate) path: String,
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

fn validate_replace_options(
    options: NativeReplaceRegularFileOptions,
) -> Result<ReplaceRegularFileOptions> {
    windows::validate_relative_path(&options.path)?;

    if options.operation_id.is_empty()
        || options.operation_id.contains('\0')
        || options.operation_id.len() > 4096
    {
        return Err(native_error("operation id is invalid"));
    }

    let maximum_bytes = validate_maximum_bytes(options.maximum_bytes)?;
    let expected = options.expected.to_vec();
    let replacement = options.replacement.to_vec();

    if expected.len() > maximum_bytes as usize || replacement.len() > maximum_bytes as usize {
        return Err(native_error("replacement bytes exceed maximum"));
    }

    Ok(ReplaceRegularFileOptions {
        expected,
        replacement,
        maximum_bytes,
        operation_id: options.operation_id,
        path: options.path,
    })
}
