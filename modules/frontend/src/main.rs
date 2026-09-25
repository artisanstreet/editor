// Launch the desktop app without allocating a Windows console window.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    artisan_frontend::run()
}
