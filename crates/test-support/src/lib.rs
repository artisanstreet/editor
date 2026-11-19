pub mod executable;
pub mod float;
pub mod poll;
#[cfg(unix)]
pub mod sandbox;
pub mod wait;

#[cfg(any(feature = "tokio", feature = "clock"))]
pub mod clock;
#[cfg(feature = "editor-core")]
pub mod editor;
#[cfg(feature = "http")]
pub mod http;
#[cfg(feature = "items")]
pub mod items;
#[cfg(feature = "logs")]
pub mod logs;
#[cfg(feature = "net")]
pub mod net;
#[cfg(feature = "replay")]
pub mod replay;
#[cfg(feature = "scenario")]
pub mod scenario;
#[cfg(feature = "sse")]
pub mod sse;
#[cfg(feature = "transcript")]
pub mod transcript;

pub const SANDBOX_REQUIRED: &str = "ARTISAN_SANDBOX_REQUIRED";

#[expect(
    clippy::print_stderr,
    reason = "a test the machine can't run says so instead of passing silently"
)]
pub fn skipped(why: std::fmt::Arguments<'_>) {
    eprintln!("SKIPPED: {why}");
}

pub fn sandbox_missing(why: std::fmt::Arguments<'_>) {
    assert!(
        std::env::var_os(SANDBOX_REQUIRED).is_none(),
        "{SANDBOX_REQUIRED} is set, so this test must run, but {why}"
    );
    skipped(why);
}
