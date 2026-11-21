use super::Module;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    GetModuleFileNameW, GetModuleHandleExW,
};
use windows::core::PCWSTR;

pub(super) fn module_of(ip: usize) -> Option<Module> {
    let mut module = HMODULE::default();
    let found = unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(std::ptr::with_exposed_provenance(ip)),
            &raw mut module,
        )
    };
    if found.is_err() || module.is_invalid() {
        return None;
    }
    let mut name = vec![0_u16; 1024];
    let written = unsafe { GetModuleFileNameW(Some(module), &mut name) };
    let written = usize::try_from(written).expect("invariant: a u32 fits in usize");
    Some(Module {
        path: {
            name.truncate(written);
            String::from_utf16_lossy(&name)
        },
        base: module.0.expose_provenance(),
    })
}
