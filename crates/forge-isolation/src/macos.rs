use std::ffi::c_int;

const IOPOL_TYPE_DISK: c_int = 0;
const IOPOL_SCOPE_PROCESS: c_int = 0;
const IOPOL_UTILITY: c_int = 4;

unsafe extern "C" {
    fn setiopolicy_np(iotype: c_int, scope: c_int, policy: c_int) -> c_int;
}

pub fn yield_disk() {
    unsafe {
        setiopolicy_np(IOPOL_TYPE_DISK, IOPOL_SCOPE_PROCESS, IOPOL_UTILITY);
    }
}
