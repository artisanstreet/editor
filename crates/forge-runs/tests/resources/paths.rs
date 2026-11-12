use crate::resources::{Host, ResourceError, Written, automount_root, host_path, written_sources};
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
    }
}

fn plain() -> Host {
    Host {
        home: Some(PathBuf::from("/home/ada")),
        windows_drives: None,
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
        PathBuf::from("/work/project/D:/game/front.png")
    );
    let homeless = Host::default();
    let home = Written {
        path: "a.png".to_owned(),
        from_home: true,
    };
    assert!(matches!(
        host_path(&home, Path::new("/work"), &homeless),
        Err(ResourceError::Refused(_))
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
