use artisan_editor_cli::instance::{
    NativeInstanceConfig, NativeListenerConfig, NativeRunConfig, NativeRunConfigInput,
};
use std::{
    fs,
    num::NonZeroU32,
    path::{Path, PathBuf},
};

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

fn sample_config(home: &Path) -> NativeInstanceConfig {
    NativeInstanceConfig::new(
        home.join("data").join("artisan.sqlite"),
        home.join("custody").join("lock"),
        home.join("readiness").join("ready"),
        home.join("credentials").join("manifest.json"),
        sample_listener(),
        sample_native_run(),
    )
    .unwrap()
}

fn sample_native_run() -> NativeRunConfig {
    NativeRunConfig::new(NativeRunConfigInput {
        claim_lease_ms: 501,
        poll_interval_ms: 502,
        retry_backoff_ms: 503,
        shutdown_budget_ms: 504,
        queue_capacity: 505,
        max_command_retries: 506,
        prompt_delivery: "queue".to_owned(),
        stream_after: 0,
    })
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
    let encoded: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        encoded["native_run"],
        serde_json::json!({
            "claim_lease_ms": 501,
            "poll_interval_ms": 502,
            "retry_backoff_ms": 503,
            "shutdown_budget_ms": 504,
            "queue_capacity": 505,
            "max_command_retries": 506,
            "prompt_delivery": "queue",
            "stream_after": 0,
        })
    );
    assert_eq!(loaded.native_run().claim_lease_ms(), 501);
    assert_eq!(loaded.native_run().poll_interval_ms(), 502);
    assert_eq!(loaded.native_run().retry_backoff_ms(), 503);
    assert_eq!(loaded.native_run().shutdown_budget_ms(), 504);
    assert_eq!(loaded.native_run().queue_capacity().get(), 505);
    assert_eq!(loaded.native_run().max_command_retries().get(), 506);
    assert_eq!(loaded.native_run().prompt_delivery(), "queue");
    assert_eq!(loaded.native_run().stream_after(), 0);
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn native_rejects_unknown_and_version() {
    let home = temp_dir("unknown");
    let path = home.join("instance-v2.json");
    fs::write(
        &path,
        br#"{"schema":"artisan-instance-v2","version":2,"database_path":"/tmp/a","custody_path":"/tmp/b","readiness_path":"/tmp/c","credentials_manifest":"/tmp/d","listener":{"admission_timeout_ms":1,"handshake_timeout_ms":1,"request_timeout_ms":1,"drain_timeout_ms":1,"admission_capacity":1,"requests_per_connection":1},"native_run":{"claim_lease_ms":1,"poll_interval_ms":1,"retry_backoff_ms":1,"shutdown_budget_ms":1,"queue_capacity":1,"max_command_retries":1,"prompt_delivery":"queue","stream_after":0},"extra":"field"}"#,
    )
    .unwrap();
    assert!(NativeInstanceConfig::load(&path).is_err());
    fs::write(
        &path,
        br#"{"schema":"artisan-instance-v2","version":1,"database_path":"/tmp/a","custody_path":"/tmp/b","readiness_path":"/tmp/c","credentials_manifest":"/tmp/d","listener":{"admission_timeout_ms":1,"handshake_timeout_ms":1,"request_timeout_ms":1,"drain_timeout_ms":1,"admission_capacity":1,"requests_per_connection":1},"native_run":{"claim_lease_ms":1,"poll_interval_ms":1,"retry_backoff_ms":1,"shutdown_budget_ms":1,"queue_capacity":1,"max_command_retries":1,"prompt_delivery":"queue","stream_after":0}}"#,
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
            sample_listener(),
            sample_native_run(),
        )
        .is_err()
    );
    let path = home.join("instance-v2.json");
    fs::write(
        &path,
        br#"{"schema":"artisan-instance-v2","version":2,"database_path":"/tmp/a","custody_path":"/tmp/b","readiness_path":"/tmp/c","credentials_manifest":"/tmp/d","listener":{"admission_timeout_ms":1,"handshake_timeout_ms":1,"request_timeout_ms":1,"drain_timeout_ms":1,"admission_capacity":0,"requests_per_connection":1},"native_run":{"claim_lease_ms":1,"poll_interval_ms":1,"retry_backoff_ms":1,"shutdown_budget_ms":1,"queue_capacity":1,"max_command_retries":1,"prompt_delivery":"queue","stream_after":0}}"#,
    )
    .unwrap();
    assert!(NativeInstanceConfig::load(&path).is_err());
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn native_run_policy_rejects_missing_unknown_and_invalid_values() {
    let home = temp_dir("native-run-policy");
    let path = NativeInstanceConfig::native_path(&home);
    let config = sample_config(&home);
    config.write(&path).unwrap();

    let valid: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();

    let mut missing = valid.clone();
    missing.as_object_mut().unwrap().remove("native_run");
    fs::write(&path, serde_json::to_vec(&missing).unwrap()).unwrap();
    assert!(NativeInstanceConfig::load(&path).is_err());

    let mut unknown = valid.clone();
    unknown["native_run"]["extra"] = serde_json::json!(true);
    fs::write(&path, serde_json::to_vec(&unknown).unwrap()).unwrap();
    assert!(NativeInstanceConfig::load(&path).is_err());

    let mut zero_duration = valid.clone();
    zero_duration["native_run"]["claim_lease_ms"] = serde_json::json!(0);
    fs::write(&path, serde_json::to_vec(&zero_duration).unwrap()).unwrap();
    assert!(NativeInstanceConfig::load(&path).is_err());

    let mut duration_overflow = valid.clone();
    duration_overflow["native_run"]["poll_interval_ms"] = serde_json::json!(u64::MAX / 2 + 1);
    fs::write(&path, serde_json::to_vec(&duration_overflow).unwrap()).unwrap();
    assert!(NativeInstanceConfig::load(&path).is_err());

    let mut zero_capacity = valid.clone();
    zero_capacity["native_run"]["queue_capacity"] = serde_json::json!(0);
    fs::write(&path, serde_json::to_vec(&zero_capacity).unwrap()).unwrap();
    assert!(NativeInstanceConfig::load(&path).is_err());

    let mut capacity_overflow = valid.clone();
    capacity_overflow["native_run"]["max_command_retries"] =
        serde_json::json!(u64::from(u32::MAX) + 1);
    fs::write(&path, serde_json::to_vec(&capacity_overflow).unwrap()).unwrap();
    assert!(NativeInstanceConfig::load(&path).is_err());

    let mut empty_prompt = valid.clone();
    empty_prompt["native_run"]["prompt_delivery"] = serde_json::json!("");
    fs::write(&path, serde_json::to_vec(&empty_prompt).unwrap()).unwrap();
    assert!(NativeInstanceConfig::load(&path).is_err());

    let mut control_prompt = valid.clone();
    control_prompt["native_run"]["prompt_delivery"] = serde_json::json!("queue\rnext");
    fs::write(&path, serde_json::to_vec(&control_prompt).unwrap()).unwrap();
    assert!(NativeInstanceConfig::load(&path).is_err());

    let mut long_prompt = valid;
    long_prompt["native_run"]["prompt_delivery"] = serde_json::json!("p".repeat(257));
    fs::write(&path, serde_json::to_vec(&long_prompt).unwrap()).unwrap();
    assert!(NativeInstanceConfig::load(&path).is_err());

    fs::remove_dir_all(home).unwrap();
}

#[test]
fn native_run_policy_accepts_inclusive_boundaries_and_stream_zero() {
    let home = temp_dir("native-run-boundaries");
    let maximum = NativeRunConfig::new(NativeRunConfigInput {
        claim_lease_ms: u64::MAX / 2,
        poll_interval_ms: u64::MAX / 2,
        retry_backoff_ms: u64::MAX / 2,
        shutdown_budget_ms: u64::MAX / 2,
        queue_capacity: u32::MAX,
        max_command_retries: u32::MAX,
        prompt_delivery: "p".repeat(256),
        stream_after: u64::MAX,
    })
    .unwrap();
    let config = NativeInstanceConfig::new(
        home.join("data").join("artisan.sqlite"),
        home.join("custody").join("lock"),
        home.join("readiness").join("ready"),
        home.join("credentials").join("manifest.json"),
        sample_listener(),
        maximum,
    )
    .unwrap();
    let path = NativeInstanceConfig::native_path(&home);
    config.write(&path).unwrap();
    let loaded = NativeInstanceConfig::load(&path).unwrap();
    assert_eq!(loaded.native_run().claim_lease_ms(), u64::MAX / 2);
    assert_eq!(loaded.native_run().queue_capacity().get(), u32::MAX);
    assert_eq!(loaded.native_run().max_command_retries().get(), u32::MAX);
    assert_eq!(loaded.native_run().prompt_delivery().len(), 256);
    assert_eq!(loaded.native_run().stream_after(), u64::MAX);
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn native_run_config_rejects_zero_and_out_of_range_values() {
    for input in [
        NativeRunConfigInput {
            claim_lease_ms: 0,
            poll_interval_ms: 1,
            retry_backoff_ms: 1,
            shutdown_budget_ms: 1,
            queue_capacity: 1,
            max_command_retries: 1,
            prompt_delivery: "queue".to_owned(),
            stream_after: 0,
        },
        NativeRunConfigInput {
            claim_lease_ms: u64::MAX / 2 + 1,
            poll_interval_ms: 1,
            retry_backoff_ms: 1,
            shutdown_budget_ms: 1,
            queue_capacity: 1,
            max_command_retries: 1,
            prompt_delivery: "queue".to_owned(),
            stream_after: 0,
        },
        NativeRunConfigInput {
            claim_lease_ms: 1,
            poll_interval_ms: 1,
            retry_backoff_ms: 1,
            shutdown_budget_ms: 1,
            queue_capacity: 0,
            max_command_retries: 1,
            prompt_delivery: "queue".to_owned(),
            stream_after: 0,
        },
        NativeRunConfigInput {
            claim_lease_ms: 1,
            poll_interval_ms: 1,
            retry_backoff_ms: 1,
            shutdown_budget_ms: 1,
            queue_capacity: 1,
            max_command_retries: 0,
            prompt_delivery: "queue".to_owned(),
            stream_after: 0,
        },
        NativeRunConfigInput {
            claim_lease_ms: 1,
            poll_interval_ms: 1,
            retry_backoff_ms: 1,
            shutdown_budget_ms: 1,
            queue_capacity: 1,
            max_command_retries: 1,
            prompt_delivery: String::new(),
            stream_after: 0,
        },
        NativeRunConfigInput {
            claim_lease_ms: 1,
            poll_interval_ms: 1,
            retry_backoff_ms: 1,
            shutdown_budget_ms: 1,
            queue_capacity: 1,
            max_command_retries: 1,
            prompt_delivery: "queue\nnext".to_owned(),
            stream_after: 0,
        },
        NativeRunConfigInput {
            claim_lease_ms: 1,
            poll_interval_ms: 1,
            retry_backoff_ms: 1,
            shutdown_budget_ms: 1,
            queue_capacity: 1,
            max_command_retries: 1,
            prompt_delivery: "p".repeat(257),
            stream_after: 0,
        },
    ] {
        assert!(NativeRunConfig::new(input).is_err());
    }
}

#[test]
fn native_config_debug_redacts_paths_and_prompt_delivery() {
    let home = temp_dir("debug");
    let canary = "ATTACKER_NATIVE_RUN_PROMPT_CANARY_7F3A";
    let config = NativeInstanceConfig::new(
        home.join("data").join("artisan.sqlite"),
        home.join("custody").join("lock"),
        home.join("readiness").join("ready"),
        home.join("credentials").join("manifest.json"),
        sample_listener(),
        NativeRunConfig::new(NativeRunConfigInput {
            claim_lease_ms: 501,
            poll_interval_ms: 502,
            retry_backoff_ms: 503,
            shutdown_budget_ms: 504,
            queue_capacity: 505,
            max_command_retries: 506,
            prompt_delivery: canary.to_owned(),
            stream_after: 0,
        })
        .unwrap(),
    )
    .unwrap();
    let debug = format!("{config:?}");
    assert!(debug.contains("NativeInstanceConfig"));
    assert!(debug.contains("prompt_delivery_bytes"));
    assert!(!debug.contains(canary));
    for path in [
        config.database_path(),
        config.custody_path(),
        config.readiness_path(),
        config.credentials_manifest(),
    ] {
        let path_text = path.to_string_lossy();
        assert!(!debug.contains(path_text.as_ref()));
    }
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
fn nested_parent_reparse_rejected() {
    let home = temp_dir("nested");
    let nested = home.join("a").join("b").join("c");
    fs::create_dir_all(&nested).unwrap();
    let real = home.join("real");
    fs::create_dir_all(&real).unwrap();
    let link = home.join("a").join("b");
    let _ = fs::remove_dir_all(&link);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let path = nested.join("instance-v2.json");
        let config = sample_config(&home);
        let err = config.write(&path);
        assert!(err.is_err(), "nested symlink must be rejected");
    }
    #[cfg(windows)]
    {
        match std::os::windows::fs::symlink_dir(&real, &link) {
            Ok(()) => {
                let path = nested.join("instance-v2.json");
                let config = sample_config(&home);
                let err = config.write(&path);
                assert!(err.is_err(), "nested reparse must be rejected");
                let _ = fs::remove_file(&link);
            }
            Err(_) => {
                eprintln!("SKIP: nested reparse not supported on this Windows host");
            }
        }
    }
    let _ = fs::remove_file(&link);
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn handle_read_fencing_rejects_symlink_drift() {
    let home = temp_dir("drift");
    let path = NativeInstanceConfig::native_path(&home);
    let config = sample_config(&home);
    config.write(&path).unwrap();
    let backup = path.with_extension("bak");
    fs::rename(&path, &backup).unwrap();
    #[cfg(unix)]
    let created = {
        std::os::unix::fs::symlink(&backup, &path).unwrap();
        true
    };
    #[cfg(windows)]
    let created = {
        if std::os::windows::fs::symlink_file(&backup, &path).is_ok() {
            true
        } else {
            eprintln!("SKIP: symlink drift not supported on this Windows host");
            fs::rename(&backup, &path).unwrap();
            fs::remove_dir_all(home).unwrap();
            return;
        }
    };
    if created {
        assert!(
            NativeInstanceConfig::load(&path).is_err(),
            "handle fencing must reject drift"
        );
        let _ = fs::remove_file(&path);
        fs::rename(&backup, &path).unwrap();
    }
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn atomic_replacement_identity_fenced() {
    let home = temp_dir("atomic");
    let path = NativeInstanceConfig::native_path(&home);
    let config = sample_config(&home);
    config.write(&path).unwrap();
    let config2 = NativeInstanceConfig::new(
        home.join("data2").join("artisan.sqlite"),
        home.join("custody2").join("lock"),
        home.join("readiness2").join("ready"),
        home.join("credentials2").join("manifest.json"),
        sample_listener(),
        sample_native_run(),
    )
    .unwrap();
    config2.write(&path).unwrap();
    let loaded = NativeInstanceConfig::load(&path).unwrap();
    assert_eq!(loaded.database_path(), config2.database_path());
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn redacted_errors_do_not_echo_attacker() {
    let home = temp_dir("redact");
    let path = home.join("instance-v2.json");
    fs::write(&path, b"evil\x00 content").unwrap();
    let err = NativeInstanceConfig::load(&path).unwrap_err();
    assert!(!format!("{err}").contains("evil"));
    assert!(!format!("{err:?}").contains("evil"));
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
