#[path = "markdown/presenter.rs"]
mod presenter;
#[path = "markdown/reveal.rs"]
mod reveal;
#[expect(clippy::disallowed_types, reason = "test fixture driving the raw aqua text and element APIs, not view code")]
#[path = "markdown/visual.rs"]
mod visual;
#[cfg(feature = "qualification")]
#[path = "markdown/transport.rs"]
mod transport;

mod prose_measure {
    use aqua_ui::style::Length;
    use editor_aqua::ui::markdown::{PROSE_MEASURE_PX, prose_measure};

    #[test]
    fn prose_is_capped_at_672_pixels() {
        assert_eq!(PROSE_MEASURE_PX, 672.0);
        assert!(matches!(prose_measure(false), Length::Px(width) if width == 672.0));
    }

    #[test]
    fn code_blocks_are_not_capped() {
        assert!(matches!(prose_measure(true), Length::Auto));
    }
}
