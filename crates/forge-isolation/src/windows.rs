use crate::Outcome;
use std::ffi::c_void;
use windows::Win32::Foundation::{CloseHandle, HANDLE, NTSTATUS};
use windows::Win32::System::Memory::{
    QUOTA_LIMITS_HARDWS_MAX_DISABLE, QUOTA_LIMITS_HARDWS_MIN_ENABLE, SetProcessWorkingSetSizeEx,
};
use windows::Win32::System::Threading::{
    ABOVE_NORMAL_PRIORITY_CLASS, GetCurrentProcess, MEMORY_PRIORITY_BELOW_NORMAL,
    MEMORY_PRIORITY_INFORMATION, OpenProcess, PROCESS_SET_INFORMATION, ProcessMemoryPriority,
    SetPriorityClass, SetProcessInformation,
};

const RESIDENT_FLOOR: usize = 128 << 20;
const RESIDENT_SOFT_CEILING: usize = 2 << 30;
const PROCESS_IO_PRIORITY: i32 = 33;
const IO_PRIORITY_LOW: u32 = 1;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtSetInformationProcess(
        process: HANDLE,
        class: i32,
        information: *const c_void,
        length: u32,
    ) -> NTSTATUS;
}

fn byte_length<T>() -> u32 {
    u32::try_from(size_of::<T>()).unwrap_or(u32::MAX)
}

pub fn protect_forge() -> Vec<Outcome> {
    let forge = unsafe { GetCurrentProcess() };
    vec![
        Outcome::of("Forge is scheduled ahead of engines", unsafe {
            SetPriorityClass(forge, ABOVE_NORMAL_PRIORITY_CLASS)
        }),
        Outcome::of("Forge keeps 128 MiB resident", unsafe {
            SetProcessWorkingSetSizeEx(
                forge,
                RESIDENT_FLOOR,
                RESIDENT_SOFT_CEILING,
                QUOTA_LIMITS_HARDWS_MIN_ENABLE | QUOTA_LIMITS_HARDWS_MAX_DISABLE,
            )
        }),
    ]
}

pub fn demote(process_id: u32) {
    let engine = match unsafe { OpenProcess(PROCESS_SET_INFORMATION, false, process_id) } {
        Ok(engine) => engine,
        Err(error) => {
            tracing::debug!(process_id, %error, "engine keeps default memory and disk priority");
            return;
        }
    };
    let memory = MEMORY_PRIORITY_INFORMATION {
        MemoryPriority: MEMORY_PRIORITY_BELOW_NORMAL,
    };
    let paged_out_first = unsafe {
        SetProcessInformation(
            engine,
            ProcessMemoryPriority,
            (&raw const memory).cast(),
            byte_length::<MEMORY_PRIORITY_INFORMATION>(),
        )
    };
    let disk = IO_PRIORITY_LOW;
    let disk_yields = unsafe {
        NtSetInformationProcess(
            engine,
            PROCESS_IO_PRIORITY,
            (&raw const disk).cast(),
            byte_length::<u32>(),
        )
    }
    .ok();
    if let Err(error) = paged_out_first {
        tracing::debug!(process_id, %error, "engine keeps default memory priority");
    }
    if let Err(error) = disk_yields {
        tracing::debug!(process_id, %error, "engine keeps default disk priority");
    }
    unsafe { CloseHandle(engine) }.ok();
}
