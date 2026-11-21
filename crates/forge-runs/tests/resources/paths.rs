use crate::resources::{
    Host, ResourceError, Written, automount_root, host_path, without_verbatim_prefix,
    written_sources,
};
use std::path::{Path, PathBuf};

fn written(path: &str) -> Written {
    Written {
        path: path.to_owned(),
        from_home: false,
    }
}

fn wsl() -> Host {
    Host {
        home: Some(PathBuf::from("/home/ada")),
        windows_drives: Some(PathBuf::from("/mnt")),
        wsl_distribution: Some("Ubuntu".to_owned()),
    }
}

fn plain() -> Host {
    Host {
        home: Some(PathBuf::from("/home/ada")),
        windows_drives: None,
        wsl_distribution: None,
    }
}

fn resolved(path: &str, host: &Host) -> PathBuf {
    host_path(&written(path), Path::new("/work/project"), host).unwrap()
}

#[test]
fn windows_paths_reach_the_mounted_drive_inside_wsl() {
    for (path, expected) in [
        (
            "D:/game/Saved/final/front.png",
            "/mnt/d/game/Saved/final/front.png",
        ),
        ("d:\\game\\Saved\\front.png", "/mnt/d/game/Saved/front.png"),
        (
            "file:///C:/Users/Ada/My%20Shots/a.png",
            "/mnt/c/Users/Ada/My Shots/a.png",
        ),
        (
            "\\\\wsl.localhost\\Ubuntu\\home\\ada\\a.png",
            "/home/ada/a.png",
        ),
        ("//wsl$/Ubuntu/tmp/shot.png", "/tmp/shot.png"),
        ("file://wsl.localhost/Ubuntu/tmp/shot.png", "/tmp/shot.png"),
    ] {
        assert_eq!(resolved(path, &wsl()), PathBuf::from(expected), "{path}");
    }
}

#[test]
fn a_link_into_another_wsl_distribution_is_refused_not_read_from_this_one() {
    for path in [
        "\\\\wsl.localhost\\Debian\\home\\ada\\a.png",
        "//wsl$/Debian/tmp/shot.png",
        "file://wsl.localhost/Debian/tmp/shot.png",
    ] {
        assert!(
            matches!(
                host_path(&written(path), Path::new("/work/project"), &wsl()),
                Err(ResourceError::OtherDistribution { .. })
            ),
            "{path}"
        );
    }
    let unnamed = Host {
        wsl_distribution: None,
        ..wsl()
    };
    assert!(matches!(
        host_path(
            &written("//wsl$/Ubuntu/tmp/shot.png"),
            Path::new("/work/project"),
            &unnamed
        ),
        Err(ResourceError::OtherDistribution { .. })
    ));
    assert_eq!(
        resolved("//WSL.localhost/ubuntu/tmp/shot.png", &wsl()),
        PathBuf::from("/tmp/shot.png")
    );
}

#[test]
fn drive_mounts_follow_the_configured_automount_root() {
    assert_eq!(automount_root(""), PathBuf::from("/mnt"));
    assert_eq!(
        automount_root("[boot]\nsystemd=true\n[automount]\nmountFsTab=false\n"),
        PathBuf::from("/mnt")
    );
    let root = automount_root("[interop]\nroot = /wrong\n[automount]\nroot = /windows/\n");
    assert_eq!(root, PathBuf::from("/windows/"));
    let host = Host {
        home: None,
        windows_drives: Some(root),
        wsl_distribution: None,
    };
    assert_eq!(
        resolved("E:/a.png", &host),
        PathBuf::from("/windows/e/a.png")
    );
}

#[test]
fn native_paths_resolve_against_the_thread_folder_and_home() {
    for host in [wsl(), plain()] {
        assert_eq!(
            resolved("docs/assets/after.png", &host),
            PathBuf::from("/work/project/docs/assets/after.png")
        );
        assert_eq!(
            resolved("/tmp/shot.png", &host),
            PathBuf::from("/tmp/shot.png")
        );
        assert_eq!(
            resolved("file:///tmp/with%20space.png", &host),
            PathBuf::from("/tmp/with space.png")
        );
        let home = Written {
            path: "shots/a.png".to_owned(),
            from_home: true,
        };
        assert_eq!(
            host_path(&home, Path::new("/work/project"), &host).unwrap(),
            PathBuf::from("/home/ada/shots/a.png")
        );
    }
    assert_eq!(
        resolved("D:/game/front.png", &plain()),
        if cfg!(windows) {
            PathBuf::from("D:/game/front.png")
        } else {
            PathBuf::from("/work/project/D:/game/front.png")
        }
    );
    let homeless = Host::default();
    let home = Written {
        path: "a.png".to_owned(),
        from_home: true,
    };
    assert!(matches!(
        host_path(&home, Path::new("/work"), &homeless),
        Err(ResourceError::NoHome)
    ));
}

#[test]
fn a_reply_names_every_local_file_it_shows_once_as_the_editor_asks_for_it() {
    let reply = r#"Here is the crew.

```artisan-image
{ "src": " D:/game/front.png ", "alt": "Front" }
```

![Side view](docs/side.png "Side") and ![remote](https://example.com/a.png).

```artisan-image
{ "images": ["~/shots/a.png", { "src": "docs/side.png", "caption": "again" }, "data:image/png;base64,AAAA"] }
```

```artisan-image-compare
{ "before": { "src": "file:///tmp/before.png" }, "after": { "src": "/tmp/after.png", "label": "After" } }
```

```artisan-audio
{ "tracks": [{ "src": "out/notify.wav", "name": "Notification" }] }
```

```json
{ "src": "not-shown.png" }
```

```artisan-image
{ "src": "broken.png"
```
"#;
    let mut expected = [
        ("D:/game/front.png", false),
        ("docs/side.png", false),
        ("shots/a.png", true),
        ("file:///tmp/before.png", false),
        ("/tmp/after.png", false),
        ("out/notify.wav", false),
    ]
    .map(|(path, from_home)| Written {
        path: path.to_owned(),
        from_home,
    });
    expected.sort();
    let mut found = written_sources(reply);
    found.sort();
    assert_eq!(found, expected);
}

#[test]
fn windows_canonical_paths_compare_in_the_form_permission_rules_are_written_in() {
    assert_eq!(
        without_verbatim_prefix(Path::new(r"\\?\C:\Users\ada\project\.env")),
        PathBuf::from(r"C:\Users\ada\project\.env")
    );
    assert_eq!(
        without_verbatim_prefix(Path::new(r"\\?\UNC\server\share\project\key.pem")),
        PathBuf::from(r"\\server\share\project\key.pem")
    );
    assert_eq!(
        without_verbatim_prefix(Path::new(r"\\?\GLOBALROOT\Device\Null")),
        PathBuf::from(r"\\?\GLOBALROOT\Device\Null")
    );
    assert_eq!(
        without_verbatim_prefix(Path::new("/home/ada/project/.env")),
        PathBuf::from("/home/ada/project/.env")
    );
}
