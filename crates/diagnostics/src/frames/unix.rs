use super::Module;
use std::ffi::CStr;

pub(super) fn module_of(ip: usize) -> Option<Module> {
    let mut info = libc::Dl_info {
        dli_fname: std::ptr::null(),
        dli_fbase: std::ptr::null_mut(),
        dli_sname: std::ptr::null(),
        dli_saddr: std::ptr::null_mut(),
    };
    let found = unsafe { libc::dladdr(std::ptr::with_exposed_provenance(ip), &raw mut info) };
    if found == 0 || info.dli_fbase.is_null() {
        return None;
    }
    let path = if info.dli_fname.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(info.dli_fname) }
            .to_string_lossy()
            .into_owned()
    };
    Some(Module {
        path,
        base: info.dli_fbase.expose_provenance(),
    })
}
