//! Behavioral coverage for trailing citation attributions in assistant prose.
//!
//! A paragraph ending in a bare `Source`/`Sources` link must hoist the
//! attribution into secondary pills that trail the sentence in its own row
//! — never a chip column beneath the paragraph. The bounds assertions pin
//! the pill inside the paragraph's vertical span: a below-paragraph layout
//! would paint the pill at or past the paragraph bottom edge. Exact line
//! heights stay out of scope: test-environment font metrics differ from
//! production, so only row integrity is asserted here.

use artisan_ui::markdown_renderer::{MarkdownBodyTone, MarkdownRenderer};
use artisan_ui::theme::{ArtisanTheme, ThemeMode};
use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, Styled, TestAppContext,
    Window, div, px,
};

const HOST_SELECTOR: &str = "cite-host";
const PARAGRAPH_SELECTOR: &str = "cite-probe-markdown-block-0";
const PILL_SELECTOR: &str = "cite-probe-markdown-block-0-cite-0";

const BODY: &str = "It’s by Morrissey.[Source](https://example.com/song)";

/// A short assistant reply in a fixed-width host: prose plus pill fit one line.
struct TrailingCitationProbe {
    renderer: MarkdownRenderer,
}

impl Render for TrailingCitationProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(600.0))
            .debug_selector(|| HOST_SELECTOR.to_string())
            .child(self.renderer.render_source_with_tone(
                BODY,
                ArtisanTheme::for_mode(ThemeMode::Dark),
                "cite-probe",
                MarkdownBodyTone::Foreground,
            ))
    }
}

#[gpui::test]
fn plain_prose_control_reports_content_width(cx: &mut TestAppContext) {
    struct PlainProbe {
        renderer: MarkdownRenderer,
    }
    impl Render for PlainProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(600.0))
                .debug_selector(|| "plain-host".to_string())
                .child(self.renderer.render_source_with_tone(
                    "It’s by Morrissey.",
                    ArtisanTheme::for_mode(ThemeMode::Dark),
                    "plain-probe",
                    MarkdownBodyTone::Foreground,
                ))
        }
    }
    let (_, cx) = cx.add_window_view(|_, _| PlainProbe {
        renderer: MarkdownRenderer::new(),
    });
    let paragraph = cx
        .debug_bounds("plain-probe-markdown-block-0")
        .expect("paragraph must paint inspectable bounds");
    // Block prose fills its host width regardless of content length.
    assert_eq!(paragraph.size.width, px(600.0));
}

#[gpui::test]
fn trailing_source_pill_shares_the_sentence_line(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| TrailingCitationProbe {
        renderer: MarkdownRenderer::new(),
    });

    let paragraph = cx
        .debug_bounds(PARAGRAPH_SELECTOR)
        .expect("paragraph must paint inspectable bounds");
    let pill = cx
        .debug_bounds(PILL_SELECTOR)
        .expect("citation pill must paint inspectable bounds");

    // Row integrity: the pill shares the prose row instead of dropping to
    // a chip column beneath the paragraph. Exact line counts stay out of
    // scope (test fonts wrap differently than production fonts).
    // The secondary pill keeps the 20 px badge geometry.
    assert_eq!(pill.size.height, px(20.0));
    // The pill lives inside the paragraph's vertical span — a chip column
    // beneath the paragraph would start at or past its bottom edge.
    assert!(
        pill.top() >= paragraph.top(),
        "pill top {pill:?} must not rise above the paragraph {paragraph:?}"
    );
    assert!(
        pill.bottom() <= paragraph.bottom() + px(1.0),
        "pill bottom {pill:?} must not drop below the paragraph {paragraph:?}"
    );
    // The pill rides 4 px above the line bottom — centered on the 28 px
    // prose line — instead of sinking flush with it.
    assert!(
        pill.bottom() <= paragraph.bottom() - px(3.0),
        "pill bottom {pill:?} must lift off the paragraph bottom {paragraph:?}"
    );
    assert!(
        pill.bottom() >= paragraph.bottom() - px(5.0),
        "pill bottom {pill:?} must not float above the paragraph {paragraph:?}"
    );
    // The pill trails the prose instead of overlapping it.
    assert!(
        pill.left() > paragraph.left(),
        "pill {pill:?} must sit right of the paragraph start {paragraph:?}"
    );
    // The prose hugs its content instead of stretching full width: the
    // pill ends the sentence rather than parking at the far edge.
    let prose = cx
        .debug_bounds("cite-probe-markdown-block-0-prose")
        .expect("prose must paint inspectable bounds");
    assert!(
        prose.size.width < paragraph.size.width,
        "prose {prose:?} must hug its content inside {paragraph:?}"
    );
    assert!(
        pill.left() - (prose.origin.x + prose.size.width) <= px(16.0),
        "pill {pill:?} must trail the prose end {prose:?}"
    );
}
