//! The Forge half hands the Editor half only a current invitation: one that
//! names the Forge process that is running now.

use artisan_editor_cli::credentials::hosts::HostInvitation;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use native_dev::host::invitation_names;

fn invitation(pid: u32) -> Vec<u8> {
    HostInvitation {
        version: 1,
        name: "Ubuntu".to_owned(),
        endpoint: "172.29.34.184:4433".parse().expect("endpoint"),
        incarnation: [7; 16],
        pid,
        certificate: STANDARD.encode([1, 2, 3]),
        bootstrap: STANDARD.encode([9; 32]),
    }
    .encode()
    .expect("valid invitation")
    .to_vec()
}

#[test]
fn only_an_invitation_for_the_running_forge_is_current() {
    let scratch = tempfile::tempdir().expect("scratch");
    let path = scratch.path().join("host.json");
    assert!(!invitation_names(&path, 42), "no invitation yet");
    std::fs::write(&path, invitation(41)).expect("previous incarnation");
    assert!(
        !invitation_names(&path, 42),
        "the previous Forge's invitation"
    );
    std::fs::write(&path, invitation(42)).expect("current incarnation");
    assert!(invitation_names(&path, 42));
    std::fs::write(&path, b"{").expect("corrupt");
    assert!(!invitation_names(&path, 42));
}
