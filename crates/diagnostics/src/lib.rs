pub mod crash;
pub mod recent;
pub mod subscriber;

pub use crash::CrashReporter;
pub use recent::RecentLines;
pub use subscriber::{SubscribeError, subscribe};

pub fn chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}
