#![cfg(test)]

#[expect(clippy::disallowed_types, reason = "test fixture driving the raw aqua text and element APIs, not view code")]
#[path = "inspector/mod.rs"]
mod inspector;
