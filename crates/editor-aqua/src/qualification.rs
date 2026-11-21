pub mod bootstrap;
pub mod fixture;
pub mod observation;
pub mod preparation;
pub mod sign_in;

#[must_use]
pub(crate) fn build() -> crate::settings::about::BuildInfo {
    crate::settings::about::BuildInfo {
        version: "1.0.0".to_owned(),
        commit: "0123456789".to_owned(),
        profile: "release".to_owned(),
        target: "x86_64-pc-windows-msvc".to_owned(),
        executable: Some(r"C:\Program Files\Artisan Street\Editor\Artisan Editor.exe".into()),
    }
}
