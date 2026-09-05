//! Rendered geometry regressions for the mounted composer and its real controls.
use artisan_ui::theme::ThemeMode;
use gpui::{AppContext as _, TestAppContext, px, size};

use crate::{
    native_composer::NativeComposer,
    native_composer_controls::{
        NATIVE_COMPOSER_PRIMARY_SELECTOR, NativeComposerControls, NativeComposerControlsSnapshot,
    },
    native_model_catalog::NativeModelCatalog,
    native_model_selector::{NATIVE_MODEL_SELECTOR_TRIGGER_SELECTOR, NativeModelSelector},
};

#[gpui::test]
fn composer_controls_keep_equal_edge_insets_as_the_draft_grows(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| {
        let controls =
            cx.new(|cx| NativeComposerControls::new(NativeComposerControlsSnapshot::default(), cx));
        let picker = cx.new(|cx| {
            NativeModelSelector::new(
                NativeModelCatalog::offline().unwrap(),
                None,
                ThemeMode::Dark,
                cx,
            )
        });
        let mut composer = NativeComposer::new(cx);
        composer.set_components(controls, picker, cx);
        composer
    });
    cx.simulate_resize(size(px(900.0), px(600.0)));
    for draft in [
        "",
        "A short prompt",
        "First line\nSecond line\nThird line\nFourth line\nFifth line\nSixth line",
    ] {
        cx.update(|_, app| {
            view.update(app, |composer, cx| {
                composer.set_draft(draft);
                cx.notify();
            })
        });
        cx.run_until_parked();
        let card = cx.debug_bounds("artisan-native-composer").unwrap();
        let picker = cx
            .debug_bounds(NATIVE_MODEL_SELECTOR_TRIGGER_SELECTOR)
            .unwrap();
        let primary = cx.debug_bounds(NATIVE_COMPOSER_PRIMARY_SELECTOR).unwrap();
        let left = picker.left() - card.left();
        let right = card.right() - primary.right();
        let picker_bottom = card.bottom() - picker.bottom();
        let primary_bottom = card.bottom() - primary.bottom();
        for inset in [left, right, picker_bottom, primary_bottom] {
            assert!(
                (px(8.0)..=px(9.0)).contains(&inset),
                "expected 8px padding plus optional hairline, got {inset:?} for {draft:?}; card={card:?} picker={picker:?} send={primary:?}"
            );
        }
        if draft.contains('\n') {
            assert!(
                card.size.height > px(128.0),
                "multiline drafts must grow the composer before scrolling"
            );
        }
        assert_eq!(picker_bottom, primary_bottom);
        assert_eq!(picker.size.height, px(32.0));
        assert_eq!(primary.size.height, px(32.0));
    }
}
