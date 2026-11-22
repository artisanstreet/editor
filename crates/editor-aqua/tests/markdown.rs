#[path = "markdown/presenter.rs"]
mod presenter;
#[path = "markdown/reveal.rs"]
mod reveal;
#[cfg(feature = "qualification")]
#[path = "markdown/transport.rs"]
mod transport;
#[expect(
    clippy::disallowed_types,
    reason = "test fixture driving the raw aqua text and element APIs, not view code"
)]
#[path = "markdown/visual.rs"]
mod visual;
