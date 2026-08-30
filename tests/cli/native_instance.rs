use artisan_editor_cli::instance::{NativeInstanceConfig, NativeListenerConfig};
use std::{fs, num::NonZeroU32, path::PathBuf};

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "artisan-native-int-{}-{}-{}",
        label,
        std::process::id(),
        line!()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn sample_listener() -> NativeListenerConfig {
    NativeListenerConfig::new(
        1000,
        2000,
        3000,
        4000,
        NonZeroU32::new(10).unwrap(),
        NonZeroU32::new(20).unwrap(),
    )
}

fn sample_config(home: &PathBuf) -> NativeInstanceConfig {
    NativeInstanceConfig::new(
        home.join("data").join("artisan.sqlite"),
        home.join("custody").join("lock"),
        home.join("readiness").join("ready"),
        home.join("credentials").join("manifest.json"),
        sample_listener(),
    )
    .unwrap()
}

#[test]
fn native_exact_round_trip() {
    let home = temp_dir("roundtrip");
    let config = sample_config(&home);
    let path = NativeInstanceConfig::native_path(&home);
    config.write(&path).unwrap();
    let loaded = NativeInstanceConfig::load(&path).unwrap();
    assert_eq!(config, loaded);
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn native_rejects_unknown_and_version() {
    let home = temp_dir("unknown");
    let path = home.join("instance-v2.json");
    fs::write(
        &path,
        br#"{"schema":"artisan-instance-v2","version":2,"database_path":"/tmp/a","custody_path":"/tmp/b","readiness_path":"/tmp/c","credentials_manifest":"/tmp/d","listener":{"admission_timeout_ms":1,"handshake_timeout_ms":1,"request_timeout_ms":1,"drain_timeout_ms":1,"admission_capacity":1,"requests_per_connection":1},"extra":"field"}"#,
    )
    .unwrap();
    assert!(NativeInstanceConfig::load(&path).is_err());
    fs::write(
        &path,
        br#"{"schema":"artisan-instance-v2","version":1,"database_path":"/tmp/a","custody_path":"/tmp/b","readiness_path":"/tmp/c","credentials_manifest":"/tmp/d","listener":{"admission_timeout_ms":1,"handshake_timeout_ms":1,"request_timeout_ms":1,"drain_timeout_ms":1,"admission_capacity":1,"requests_per_connection":1}}"#,
    )
    .unwrap();
    assert!(NativeInstanceConfig::load(&path).is_err());
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn native_rejects_relative_and_zero() {
    let home = temp_dir("relative");
    assert!(
        NativeInstanceConfig::new(
            PathBuf::from("relative"),
            PathBuf::from("/tmp/b"),
            PathBuf::from("/tmp/c"),
            PathBuf::from("/tmp/d"),
            sample_listener()
        )
        .is_err()
    );
    let path = home.join("instance-v2.json");
    fs::write(
        &path,
        br#"{"schema":"artisan-instance-v2","version":2,"database_path":"/tmp/a","custody_path":"/tmp/b","readiness_path":"/tmp/c","credentials_manifest":"/tmp/d","listener":{"admission_timeout_ms":1,"handshake_timeout_ms":1,"request_timeout_ms":1,"drain_timeout_ms":1,"admission_capacity":0,"requests_per_connection":1}}"#,
    )
    .unwrap();
    assert!(NativeInstanceConfig::load(&path).is_err());
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn native_listener_values_unchanged() {
    let l = NativeListenerConfig::new(
        11,
        22,
        33,
        44,
        NonZeroU32::new(5).unwrap(),
        NonZeroU32::new(6).unwrap(),
    );
    assert_eq!(l.admission_timeout_ms(), 11);
    assert_eq!(l.handshake_timeout_ms(), 22);
    assert_eq!(l.request_timeout_ms(), 33);
    assert_eq!(l.drain_timeout_ms(), 44);
    assert_eq!(l.admission_capacity().get(), 5);
    assert_eq!(l.requests_per_connection().get(), 6);
}

#[test]
fn nested_parent_symlink_rejected() {
    let home = temp_dir("nested");
    let nested = home.join("a").join("b").join("c");
    fs::create_dir_all(&nested).unwrap();
    let real = home.join("real");
    fs::create_dir_all(&real).unwrap();
    let link = home.join("a").join("b");
    let _ = fs::remove_dir_all(&link);
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real, &link).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&real, &link).unwrap_or_else(|_| return);
    let path = nested.join("instance-v2.json");
    let config = sample_config(&home);
    let err = config.write(&path);
    if cfg!(unix) {
        assert!(err.is_err());
    }
    let _ = fs::remove_file(&link);
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn handle_read_fencing_rejects_drift() {
    let home = temp_dir("drift");
    let path = NativeInstanceConfig::native_path(&home);
    let config = sample_config(&home);
    config.write(&path).unwrap();
    let backup = path.with_extension("bak");
    fs::rename(&path, &backup).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&backup, &path).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&backup, &path).unwrap_or_else(|_| {
        fs::write(&path, b"bad").unwrap();
        return;
    });
    assert!(NativeInstanceConfig::load(&path).is_err());
    let _ = fs::remove_file(&path);
    fs::rename(&backup, &path).unwrap();
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn atomic_replacement_identity_fenced() {
    let home = temp_dir("atomic");
    let path = NativeInstanceConfig::native_path(&home);
    let config = sample_config(&home);
    config.write(&path).unwrap();
    let first = fs::read(&path).unwrap();
    let config2 = NativeInstanceConfig::new(
        home.join("data2").join("artisan.sqlite"),
        home.join("custody2").join("lock"),
        home.join("readiness2").join("ready"),
        home.join("credentials2").join("manifest.json"),
        sample_listener(),
    )
    .unwrap();
    config2.write(&path).unwrap();
    let second = fs::read(&path).unwrap();
    assert!(first != second);
    let loaded = NativeInstanceConfig::load(&path).unwrap();
    assert_eq!(loaded.database_path(), config2.database_path());
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn redacted_errors() {
    let err = NativeInstanceConfig::load(&PathBuf::from("/nonexistent/path.json")).unwrap_err();
    let debug = format!("{err:?}");
    let display = format!("{err}");
    assert!(!display.contains("secret"));
    assert!(!debug.contains("secret"));
    // Even with attacker content in file, error must not echo
    let home = temp_dir("redact");
    let path = home.join("instance-v2.json");
    fs::write(&path, b"evil\x00 content").unwrap();
    let err2 = NativeInstanceConfig::load(&path).unwrap_err();
    assert!(!format!("{err2}").contains("evil"));
    assert!(!format!("{err2:?}").contains("evil"));
    fs::remove_dir_all(home).unwrap();
}

#[test]
#[cfg(unix)]
fn unix_modes_native() {
    use std::os::unix::fs::PermissionsExt;
    let home = temp_dir("modes");
    let path = NativeInstanceConfig::native_path(&home);
    let config = sample_config(&home);
    config.write(&path).unwrap();
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    fs::remove_dir_all(home).unwrap();
}
