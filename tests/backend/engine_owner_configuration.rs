use std::path::PathBuf;
use std::time::Duration;

use super::{EngineBounds, EngineLimits, EngineOwnerConfig, EngineOwnerConfigError};

// ---------------------------------------------------------------------------
// Helpers: valid inputs
// ---------------------------------------------------------------------------

fn absolute_engine_path() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from("C:\\tmp\\engine.exe")
    } else {
        PathBuf::from("/tmp/engine")
    }
}

fn secret_absolute_path() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from("C:\\tmp\\secret-sentinel-xyz-987.exe")
    } else {
        PathBuf::from("/tmp/secret-sentinel-xyz-987")
    }
}

fn valid_limits() -> EngineLimits {
    EngineLimits {
        readiness: Duration::from_millis(100),
        health: Duration::from_millis(100),
        prompt: Duration::from_millis(100),
        sse: Duration::from_millis(100),
        close: Duration::from_millis(100),
    }
}

fn valid_bounds() -> EngineBounds {
    EngineBounds {
        max_json_body: 1024,
        max_sse_line: 1024,
        max_sse_event: 1024,
        max_readiness_line: 1024,
        max_headers: 32,
        max_buf_bytes: 8192,
        stderr_cap_bytes: 4096,
        sink_capacity: 16,
        control_capacity: 16,
    }
}

// ---------------------------------------------------------------------------
// Path validation
// ---------------------------------------------------------------------------

#[test]
fn empty_path_rejected() {
    let err = EngineOwnerConfig::new(PathBuf::from(""), valid_limits(), valid_bounds())
        .expect_err("empty path must be rejected");
    assert_eq!(err, EngineOwnerConfigError::InvalidExecutable);
    assert_eq!(format!("{err}"), "engine executable path must be absolute");
}

#[test]
fn relative_path_rejected() {
    let err = EngineOwnerConfig::new(
        PathBuf::from("relative/engine"),
        valid_limits(),
        valid_bounds(),
    )
    .expect_err("relative path must be rejected");
    assert_eq!(err, EngineOwnerConfigError::InvalidExecutable);
    assert_eq!(format!("{err}"), "engine executable path must be absolute");
}

#[test]
fn absolute_path_accepted_without_filesystem_check() {
    let path = absolute_engine_path();
    assert!(path.is_absolute());
    let cfg = EngineOwnerConfig::new(path.clone(), valid_limits(), valid_bounds())
        .expect("absolute path must be accepted");
    assert_eq!(cfg.engine_executable().as_os_str(), path.as_os_str());
}

// ---------------------------------------------------------------------------
// Duration representability — each of five Duration::MAX failures
// ---------------------------------------------------------------------------

#[test]
fn readiness_max_rejected() {
    let mut limits = valid_limits();
    limits.readiness = Duration::MAX;
    let err = EngineOwnerConfig::new(absolute_engine_path(), limits, valid_bounds())
        .expect_err("readiness MAX must be rejected");
    assert_eq!(
        err,
        EngineOwnerConfigError::UnrepresentableDuration { field: "readiness" }
    );
}

#[test]
fn health_max_rejected() {
    let mut limits = valid_limits();
    limits.health = Duration::MAX;
    let err = EngineOwnerConfig::new(absolute_engine_path(), limits, valid_bounds())
        .expect_err("health MAX must be rejected");
    assert_eq!(
        err,
        EngineOwnerConfigError::UnrepresentableDuration { field: "health" }
    );
}

#[test]
fn prompt_max_rejected() {
    let mut limits = valid_limits();
    limits.prompt = Duration::MAX;
    let err = EngineOwnerConfig::new(absolute_engine_path(), limits, valid_bounds())
        .expect_err("prompt MAX must be rejected");
    assert_eq!(
        err,
        EngineOwnerConfigError::UnrepresentableDuration { field: "prompt" }
    );
}

#[test]
fn sse_max_rejected() {
    let mut limits = valid_limits();
    limits.sse = Duration::MAX;
    let err = EngineOwnerConfig::new(absolute_engine_path(), limits, valid_bounds())
        .expect_err("sse MAX must be rejected");
    assert_eq!(
        err,
        EngineOwnerConfigError::UnrepresentableDuration { field: "sse" }
    );
}

#[test]
fn close_max_rejected() {
    let mut limits = valid_limits();
    limits.close = Duration::MAX;
    let err = EngineOwnerConfig::new(absolute_engine_path(), limits, valid_bounds())
        .expect_err("close MAX must be rejected");
    assert_eq!(
        err,
        EngineOwnerConfigError::UnrepresentableDuration { field: "close" }
    );
}

#[test]
fn zero_durations_accepted() {
    let limits = EngineLimits {
        readiness: Duration::ZERO,
        health: Duration::ZERO,
        prompt: Duration::ZERO,
        sse: Duration::ZERO,
        close: Duration::ZERO,
    };
    EngineOwnerConfig::new(absolute_engine_path(), limits, valid_bounds())
        .expect("zero durations are representable");
}

#[test]
fn representable_durations_accepted() {
    let limits = EngineLimits {
        readiness: Duration::from_secs(1),
        health: Duration::from_millis(500),
        prompt: Duration::from_secs(10),
        sse: Duration::from_secs(30),
        close: Duration::from_secs(5),
    };
    EngineOwnerConfig::new(absolute_engine_path(), limits, valid_bounds())
        .expect("representable durations must be accepted");
}

// ---------------------------------------------------------------------------
// Bounds: each of nine zero-bound failures independently
// ---------------------------------------------------------------------------

#[test]
fn max_json_body_zero_rejected() {
    let mut bounds = valid_bounds();
    bounds.max_json_body = 0;
    let err = EngineOwnerConfig::new(absolute_engine_path(), valid_limits(), bounds)
        .expect_err("max_json_body 0 must be rejected");
    assert_eq!(
        err,
        EngineOwnerConfigError::ZeroBound {
            field: "max_json_body"
        }
    );
}

#[test]
fn max_sse_line_zero_rejected() {
    let mut bounds = valid_bounds();
    bounds.max_sse_line = 0;
    let err = EngineOwnerConfig::new(absolute_engine_path(), valid_limits(), bounds)
        .expect_err("max_sse_line 0 must be rejected");
    assert_eq!(
        err,
        EngineOwnerConfigError::ZeroBound {
            field: "max_sse_line"
        }
    );
}

#[test]
fn max_sse_event_zero_rejected() {
    let mut bounds = valid_bounds();
    bounds.max_sse_event = 0;
    let err = EngineOwnerConfig::new(absolute_engine_path(), valid_limits(), bounds)
        .expect_err("max_sse_event 0 must be rejected");
    assert_eq!(
        err,
        EngineOwnerConfigError::ZeroBound {
            field: "max_sse_event"
        }
    );
}

#[test]
fn max_readiness_line_zero_rejected() {
    let mut bounds = valid_bounds();
    bounds.max_readiness_line = 0;
    let err = EngineOwnerConfig::new(absolute_engine_path(), valid_limits(), bounds)
        .expect_err("max_readiness_line 0 must be rejected");
    assert_eq!(
        err,
        EngineOwnerConfigError::ZeroBound {
            field: "max_readiness_line"
        }
    );
}

#[test]
fn max_headers_zero_rejected() {
    let mut bounds = valid_bounds();
    bounds.max_headers = 0;
    let err = EngineOwnerConfig::new(absolute_engine_path(), valid_limits(), bounds)
        .expect_err("max_headers 0 must be rejected");
    assert_eq!(
        err,
        EngineOwnerConfigError::ZeroBound {
            field: "max_headers"
        }
    );
}

#[test]
fn max_buf_bytes_zero_rejected() {
    let mut bounds = valid_bounds();
    bounds.max_buf_bytes = 0;
    let err = EngineOwnerConfig::new(absolute_engine_path(), valid_limits(), bounds)
        .expect_err("max_buf_bytes 0 must be rejected");
    assert_eq!(
        err,
        EngineOwnerConfigError::ZeroBound {
            field: "max_buf_bytes"
        }
    );
}

#[test]
fn stderr_cap_bytes_zero_rejected() {
    let mut bounds = valid_bounds();
    bounds.stderr_cap_bytes = 0;
    let err = EngineOwnerConfig::new(absolute_engine_path(), valid_limits(), bounds)
        .expect_err("stderr_cap_bytes 0 must be rejected");
    assert_eq!(
        err,
        EngineOwnerConfigError::ZeroBound {
            field: "stderr_cap_bytes"
        }
    );
}

#[test]
fn sink_capacity_zero_rejected() {
    let mut bounds = valid_bounds();
    bounds.sink_capacity = 0;
    let err = EngineOwnerConfig::new(absolute_engine_path(), valid_limits(), bounds)
        .expect_err("sink_capacity 0 must be rejected");
    assert_eq!(
        err,
        EngineOwnerConfigError::ZeroBound {
            field: "sink_capacity"
        }
    );
}

#[test]
fn control_capacity_zero_rejected() {
    let mut bounds = valid_bounds();
    bounds.control_capacity = 0;
    let err = EngineOwnerConfig::new(absolute_engine_path(), valid_limits(), bounds)
        .expect_err("control_capacity 0 must be rejected");
    assert_eq!(
        err,
        EngineOwnerConfigError::ZeroBound {
            field: "control_capacity"
        }
    );
}

// ---------------------------------------------------------------------------
// Parser buffer boundary 8191 / 8192
// ---------------------------------------------------------------------------

#[test]
fn max_buf_bytes_8191_rejected() {
    let mut bounds = valid_bounds();
    bounds.max_buf_bytes = 8191;
    let err = EngineOwnerConfig::new(absolute_engine_path(), valid_limits(), bounds)
        .expect_err("8191 must be rejected");
    assert_eq!(err, EngineOwnerConfigError::BufferTooSmall);
    assert_eq!(
        format!("{err}"),
        "http read buffer must be at least 8192 bytes"
    );
}

#[test]
fn max_buf_bytes_8192_accepted() {
    let mut bounds = valid_bounds();
    bounds.max_buf_bytes = 8192;
    EngineOwnerConfig::new(absolute_engine_path(), valid_limits(), bounds)
        .expect("8192 is the library minimum and must be accepted");
}

// ---------------------------------------------------------------------------
// Semaphore capacities: MAX_PERMITS and +1 without allocation
// ---------------------------------------------------------------------------

#[test]
fn sink_capacity_max_permits_accepted() {
    let mut bounds = valid_bounds();
    bounds.sink_capacity = tokio::sync::Semaphore::MAX_PERMITS;
    EngineOwnerConfig::new(absolute_engine_path(), valid_limits(), bounds)
        .expect("MAX_PERMITS must be accepted");
}

#[test]
fn sink_capacity_plus_one_rejected() {
    let mut bounds = valid_bounds();
    bounds.sink_capacity = tokio::sync::Semaphore::MAX_PERMITS + 1;
    let err = EngineOwnerConfig::new(absolute_engine_path(), valid_limits(), bounds)
        .expect_err("MAX_PERMITS + 1 must be rejected");
    assert_eq!(
        err,
        EngineOwnerConfigError::CapacityTooLarge {
            field: "sink_capacity"
        }
    );
}

#[test]
fn control_capacity_max_permits_accepted() {
    let mut bounds = valid_bounds();
    bounds.control_capacity = tokio::sync::Semaphore::MAX_PERMITS;
    EngineOwnerConfig::new(absolute_engine_path(), valid_limits(), bounds)
        .expect("MAX_PERMITS must be accepted");
}

#[test]
fn control_capacity_plus_one_rejected() {
    let mut bounds = valid_bounds();
    bounds.control_capacity = tokio::sync::Semaphore::MAX_PERMITS + 1;
    let err = EngineOwnerConfig::new(absolute_engine_path(), valid_limits(), bounds)
        .expect_err("MAX_PERMITS + 1 must be rejected");
    assert_eq!(
        err,
        EngineOwnerConfigError::CapacityTooLarge {
            field: "control_capacity"
        }
    );
}

// ---------------------------------------------------------------------------
// Exact preservation via immutable accessors
// ---------------------------------------------------------------------------

#[test]
fn exact_inputs_preserved() {
    let limits = EngineLimits {
        readiness: Duration::from_millis(11),
        health: Duration::from_millis(22),
        prompt: Duration::from_millis(33),
        sse: Duration::from_millis(44),
        close: Duration::from_millis(55),
    };
    let bounds = EngineBounds {
        max_json_body: 111,
        max_sse_line: 222,
        max_sse_event: 333,
        max_readiness_line: 444,
        max_headers: 55,
        max_buf_bytes: 9000,
        stderr_cap_bytes: 777,
        sink_capacity: 10,
        control_capacity: 20,
    };
    // Canonical absolute path.
    let path = absolute_engine_path();
    assert!(path.is_absolute());
    let cfg = EngineOwnerConfig::new(path.clone(), limits, bounds)
        .expect("valid inputs must be accepted");
    assert_eq!(cfg.engine_executable().as_os_str(), path.as_os_str());
    assert_eq!(cfg.limits(), &limits);
    assert_eq!(cfg.bounds(), &bounds);
    // Lexical noncanonical absolute path must be preserved verbatim.
    let noncanonical = if cfg!(windows) {
        PathBuf::from("C:\\tmp\\.\\engine.exe")
    } else {
        PathBuf::from("/tmp/./engine")
    };
    assert!(noncanonical.is_absolute());
    let cfg2 = EngineOwnerConfig::new(noncanonical.clone(), limits, bounds)
        .expect("noncanonical absolute path must be accepted");
    assert_eq!(
        cfg2.engine_executable().as_os_str(),
        noncanonical.as_os_str()
    );
    // Conditional non-Unicode absolute path (raw bytes on Unix, wide on Windows).
    #[cfg(unix)]
    {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        let raw = OsString::from_vec(vec![b'/', b't', b'm', b'p', b'/', 0xFF, 0xFE]);
        let non_unicode = PathBuf::from(raw);
        assert!(non_unicode.is_absolute());
        let cfg3 = EngineOwnerConfig::new(non_unicode.clone(), limits, bounds)
            .expect("non-Unicode absolute path must be accepted");
        assert_eq!(
            cfg3.engine_executable().as_os_str(),
            non_unicode.as_os_str()
        );
    }
    #[cfg(windows)]
    {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;
        let wide = [0x43, 0x3A, 0x5C, 0x74, 0x6D, 0x70, 0x5C, 0xD800];
        let raw = OsString::from_wide(&wide);
        let non_unicode = PathBuf::from(raw);
        assert!(non_unicode.is_absolute());
        let cfg3 = EngineOwnerConfig::new(non_unicode.clone(), limits, bounds)
            .expect("non-Unicode absolute path must be accepted");
        assert_eq!(
            cfg3.engine_executable().as_os_str(),
            non_unicode.as_os_str()
        );
    }
}

// ---------------------------------------------------------------------------
// Redaction: secret sentinel absent from Debug and error outputs
// ---------------------------------------------------------------------------

#[test]
fn config_debug_does_not_leak_secret() {
    let secret = secret_absolute_path();
    let cfg = EngineOwnerConfig::new(secret.clone(), valid_limits(), valid_bounds())
        .expect("secret absolute path must be accepted for redaction test");
    let debug = format!("{cfg:?}");
    assert!(
        !debug.contains("secret-sentinel-xyz-987"),
        "config Debug must not leak executable path"
    );
    assert!(debug.contains("<redacted>"));
    // Getter still returns exact bytes intentionally.
    assert_eq!(cfg.engine_executable().as_os_str(), secret.as_os_str());
}

#[test]
fn error_debug_and_display_do_not_leak_secret() {
    let secret = secret_absolute_path();
    let mut bad_bounds = valid_bounds();
    bad_bounds.max_buf_bytes = 8191;
    let err = EngineOwnerConfig::new(secret.clone(), valid_limits(), bad_bounds)
        .expect_err("should fail on buffer size");
    assert_eq!(err, EngineOwnerConfigError::BufferTooSmall);
    let display = format!("{err}");
    let debug = format!("{err:?}");
    assert!(
        !display.contains("secret-sentinel-xyz-987"),
        "error Display must not leak path"
    );
    assert!(
        !debug.contains("secret-sentinel-xyz-987"),
        "error Debug must not leak path"
    );

    let rel_secret = PathBuf::from("secret-sentinel-xyz-987-relative");
    let err2 = EngineOwnerConfig::new(rel_secret, valid_limits(), valid_bounds())
        .expect_err("relative secret path must be rejected");
    assert_eq!(err2, EngineOwnerConfigError::InvalidExecutable);
    let combined = format!("{err2}{err2:?}");
    assert!(
        !combined.contains("secret-sentinel-xyz-987"),
        "invalid-executable error must not leak input string"
    );
}
