#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

fn main() -> Result<(), editor_aqua::LaunchError> {
    editor_aqua::launch()
}
