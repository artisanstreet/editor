#[expect(clippy::disallowed_types, reason = "test fixture driving the raw aqua text and element APIs, not view code")]
#[path = "text/mod.rs"]
mod text;
#[path = "text/wheel_glide.rs"]
mod wheel_glide;
