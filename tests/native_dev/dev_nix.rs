//! Building with Nix and reaching the Windows runner through WSL interop.

use std::{ffi::OsString, path::Path};

use native_dev::{
    Action, Command,
    nix::{
        Checkout, Stage, Target, parse_build_outputs, payload_attribute, runner_attribute,
        untracked_refusal,
    },
    parse,
    wsl::{EditorInvocation, editor_arguments, windows_path},
};

#[test]
fn stage_outputs_are_the_flake_attributes() {
    assert_eq!(
        payload_attribute(Target::Linux, Stage::Debug),
        "linux-debug"
    );
    assert_eq!(
        payload_attribute(Target::Windows, Stage::Production),
        "windows-production"
    );
    assert_eq!(runner_attribute(Target::Windows), "windows-runner");
    let checkout = Checkout {
        root: "/home/ada/editor".into(),
    };
    assert_eq!(
        checkout.installable("linux-debug"),
        "/home/ada/editor#linux-debug"
    );
}

#[test]
fn build_outputs_are_read_in_installable_order() {
    let json = r#"[
        {"drvPath":"/nix/store/a.drv","outputs":{"out":"/nix/store/a-artisan-linux-debug"}},
        {"drvPath":"/nix/store/b.drv","outputs":{"out":"/nix/store/b-artisan-windows-debug"}}
    ]"#;
    assert_eq!(
        parse_build_outputs(json).expect("outputs"),
        [
            Path::new("/nix/store/a-artisan-linux-debug"),
            Path::new("/nix/store/b-artisan-windows-debug")
        ]
    );
    for invalid in ["", "{}", r#"[{"outputs":{}}]"#] {
        assert!(parse_build_outputs(invalid).is_err(), "{invalid}");
    }
}

#[test]
fn untracked_sources_refuse_the_build() {
    assert!(untracked_refusal("").is_none());
    let refusal = untracked_refusal("modules/cli/rust/new.rs\nnix/new.nix\n")
        .expect("refused")
        .to_string();
    assert!(refusal.contains("modules/cli/rust/new.rs"), "{refusal}");
    assert!(refusal.contains("git add -N"), "{refusal}");
}

#[test]
fn linux_paths_become_windows_paths() {
    assert_eq!(
        windows_path(Path::new("/nix/store/x-artisan-windows-debug"), "Ubuntu")
            .expect("share path"),
        r"\\wsl.localhost\Ubuntu\nix\store\x-artisan-windows-debug"
    );
    assert_eq!(
        windows_path(
            Path::new("/home/ada/.local/share/Artisan Street Dev/host.json"),
            "Ubuntu"
        )
        .expect("share path"),
        r"\\wsl.localhost\Ubuntu\home\ada\.local\share\Artisan Street Dev\host.json"
    );
    assert_eq!(
        windows_path(Path::new("/mnt/c/Users/ada/Verify"), "Ubuntu").expect("drive path"),
        r"C:\Users\ada\Verify"
    );
    assert!(windows_path(Path::new("relative"), "Ubuntu").is_err());
}

/// The Linux runner's call is exactly what the Windows runner parses.
#[test]
fn the_windows_runner_parses_what_the_linux_runner_passes() {
    let root = OsString::from(r"C:\Users\ada\AppData\Local\Artisan Street Verify");
    let arguments = editor_arguments(
        &EditorInvocation {
            command: "run",
            payload: Some(Path::new("/nix/store/x-artisan-windows-debug")),
            invitation: Some(Path::new(
                "/home/ada/.local/share/Artisan Street Dev/host.json",
            )),
            keep: 2,
            root: Some(&root),
            attach: true,
        },
        "Ubuntu",
    )
    .expect("arguments");
    let Action::Editor(editor) = parse(&arguments).expect("parses") else {
        panic!("the editor half");
    };
    assert_eq!(editor.command, Command::Run);
    assert_eq!(
        editor.payload.as_deref(),
        Some(Path::new(
            r"\\wsl.localhost\Ubuntu\nix\store\x-artisan-windows-debug"
        ))
    );
    assert_eq!(
        editor.invitation.as_deref(),
        Some(Path::new(
            r"\\wsl.localhost\Ubuntu\home\ada\.local\share\Artisan Street Dev\host.json"
        ))
    );
    assert_eq!(editor.keep, 2);
    assert_eq!(editor.root.as_deref(), Some(Path::new(&root)));
    assert!(editor.attach);

    let maintenance = editor_arguments(
        &EditorInvocation {
            command: "prune",
            payload: None,
            invitation: None,
            keep: 1,
            root: None,
            attach: false,
        },
        "Ubuntu",
    )
    .expect("arguments");
    assert_eq!(maintenance, ["editor", "prune", "--keep", "1"]);
}
