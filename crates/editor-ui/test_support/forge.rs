//! Shared real-Forge pairing setup for Client integration tests.
//! Included by each test crate, so production UI has no server dependencies.
pub fn pair(
    store: &forge_store::Store,
    runtime: &tokio::runtime::Runtime,
    address: &str,
    directory: &std::path::Path,
) -> (editor_core::EditorFiles, editor_core::HostConfig) {
    let files = editor_core::EditorFiles {
        dir: directory.to_path_buf(),
    };
    let identity = files.identity().unwrap();
    let invite = runtime.block_on(forge_http::invite(store)).unwrap();
    let fingerprint = artisan_api::tls::fingerprint_of(&invite.forge_certificate).unwrap();
    let host = editor_core::runtime::runtime()
        .block_on(editor_core::hosts::pair(
            address,
            &invite.code,
            &fingerprint,
            &identity,
            "test",
            "Forge".into(),
            editor_core::HostKind::Local,
        ))
        .unwrap();
    files
        .save_hosts(&editor_core::hosts::HostsFile {
            hosts: vec![host.clone()],
            last: Some(host.id.clone()),
        })
        .unwrap();
    (files, host)
}
