//! Mounts every `artisan-editor` catalog example as one reply body, in both
//! theme modes, and drives the pointer over and through each block: every
//! kind must lay out, paint, and survive hover and click without panicking.

use artisan_ui::markdown_renderer::MarkdownRenderer;
use artisan_ui::theme::{ArtisanTheme, ThemeMode};
use gpui::{
    Context, IntoElement, Modifiers, ParentElement as _, Render, Styled as _, TestAppContext,
    Window, div, px,
};

const CATALOG: &str = include_str!("../../skills/artisan-editor/references/blocks.md");

struct CatalogProbe {
    renderer: MarkdownRenderer,
    mode: ThemeMode,
}

impl Render for CatalogProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(760.0)).child(self.renderer.render_source(
            CATALOG,
            ArtisanTheme::for_mode(self.mode),
            "probe",
        ))
    }
}

fn mount_and_drive(cx: &mut TestAppContext, mode: ThemeMode) {
    let (_, cx) = cx.add_window_view(|_, _| CatalogProbe {
        renderer: MarkdownRenderer::new(),
        mode,
    });
    let expected = CATALOG.matches("```artisan-").count();
    let blocks: Vec<_> = (0..1000)
        .filter_map(|index| {
            // Debug selectors are `&'static str`; the test leaks its few.
            let selector: &'static str =
                Box::leak(format!("probe-markdown-block-{index}-code").into_boxed_str());
            cx.debug_bounds(selector)
        })
        .collect();
    assert_eq!(
        blocks.len(),
        expected,
        "every catalog fence paints one block"
    );
    for bounds in &blocks {
        assert!(
            bounds.size.height > px(8.0),
            "a block painted empty: {bounds:?}"
        );
        let center = bounds.center();
        cx.simulate_mouse_move(center, None, Modifiers::none());
        cx.simulate_click(center, Modifiers::none());
        cx.run_until_parked();
    }
}

#[gpui::test]
fn every_catalog_block_paints_and_survives_the_pointer_in_dark(cx: &mut TestAppContext) {
    mount_and_drive(cx, ThemeMode::Dark);
}

#[gpui::test]
fn every_catalog_block_paints_and_survives_the_pointer_in_light(cx: &mut TestAppContext) {
    mount_and_drive(cx, ThemeMode::Light);
}
