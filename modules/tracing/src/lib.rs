//! Local, bounded flight recorder. Without `enabled`, only erasing macros
//! exist: arguments are neither evaluated nor compiled, and no runtime,
//! serializer, writer, environment handling, or watchdog is included.
#![forbid(unsafe_code)]

#[cfg(feature = "enabled")]
mod event;
#[cfg(feature = "enabled")]
mod export;
#[cfg(feature = "enabled")]
mod recorder;
#[cfg(feature = "enabled")]
mod watchdog;
#[cfg(feature = "enabled")]
pub use recorder::{
    Session, Span, completed, incident, instant, is_recording, name_thread, save, span, start,
};
#[cfg(feature = "enabled")]
#[doc(hidden)]
pub use serde_json::json;
#[cfg(feature = "enabled")]
pub use watchdog::Heartbeat;

/// Async span, valid across executor threads and concurrent operations.
#[cfg(feature = "enabled")]
#[macro_export]
macro_rules! span {
    ($category:expr, $name:expr $(, $key:literal => $value:expr)* $(,)?) => {
        if $crate::is_recording() {
            $crate::span($category, $name, $crate::json!({$($key: $value),*}))
        } else {
            $crate::Span::inactive()
        }
    };
}
#[cfg(not(feature = "enabled"))]
#[macro_export]
macro_rules! span {
    ($($tokens:tt)*) => {
        ()
    };
}

/// Instant breadcrumb. Call sites must supply enums, IDs, and counts only.
#[cfg(feature = "enabled")]
#[macro_export]
macro_rules! instant {
    ($category:expr, $name:expr $(, $key:literal => $value:expr)* $(,)?) => {
        if $crate::is_recording() {
            $crate::instant($category, $name, $crate::json!({$($key: $value),*}))
        }
    };
}
#[cfg(not(feature = "enabled"))]
#[macro_export]
macro_rules! instant {
    ($($tokens:tt)*) => {
        ()
    };
}

#[cfg(feature = "enabled")]
#[macro_export]
macro_rules! counter {
    ($category:expr, $name:expr $(, $key:literal => $value:expr)* $(,)?) => {
        if $crate::is_recording() {
            $crate::recorder_counter($category, $name, $crate::json!({$($key: $value),*}))
        }
    };
}
#[cfg(feature = "enabled")]
#[doc(hidden)]
pub use recorder::counter as recorder_counter;
#[cfg(not(feature = "enabled"))]
#[macro_export]
macro_rules! counter {
    ($($tokens:tt)*) => {
        ()
    };
}

#[cfg(feature = "enabled")]
#[macro_export]
macro_rules! incident {
    ($reason:expr) => {
        $crate::incident($reason)
    };
}
#[cfg(not(feature = "enabled"))]
#[macro_export]
macro_rules! incident {
    ($($tokens:tt)*) => {
        ()
    };
}

#[cfg(all(test, not(feature = "enabled")))]
mod disabled_tests {
    #[test]
    fn erased_arguments_need_not_resolve_or_run() {
        crate::span!("ui", "removed", "value" => nonexistent::function());
        crate::instant!("ui", "removed", "value" => panic!("must not run"));
        crate::counter!("ui", "removed", "value" => nonexistent::function());
        crate::incident!(nonexistent::function());
    }
}
