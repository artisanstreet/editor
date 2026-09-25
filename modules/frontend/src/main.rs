// Launch the desktop app without allocating a Windows console window.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

/// mimalloc outperforms the platform allocators for the Editor's many small,
/// short-lived UI allocations.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> std::process::ExitCode {
    artisan_frontend::run()
}
