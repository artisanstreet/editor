#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

fn main() -> anyhow::Result<()> {
    editor_aqua::launch()
}
