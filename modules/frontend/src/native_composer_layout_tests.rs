//! Rendered geometry regressions for the mounted composer and its real controls.
//!
//! All bounds are logical pixels. The GPUI test window renders at scale
//! factor 1, so these assertions pin the reference box at scale 1; the
//! native gate must confirm the same 128px empty card on a real display.
use artisan_ui::theme::ThemeMode;
use gpui::{AppContext as _, TestAppContext, px, size};

use crate::{
    native_composer::{NATIVE_COMPOSER_EDITOR_SELECTOR, NativeComposer},
    native_composer_controls::{
        NATIVE_COMPOSER_CONTROL_ROW_SELECTOR, NATIVE_COMPOSER_PRIMARY_SELECTOR,
        NativeComposerControls, NativeComposerControlsSnapshot, PendingSteeringRow,
    },
    native_model_catalog::NativeModelCatalog,
    native_model_selector::{NATIVE_MODEL_SELECTOR_TRIGGER_SELECTOR, NativeModelSelector},
};

fn mount_composer(
    cx: &mut TestAppContext,
    snapshot: NativeComposerControlsSnapshot,
) -> (gpui::Entity<NativeComposer>, &mut gpui::VisualTestContext) {
    let (view, window_cx) = cx.add_window_view(|_, cx| {
        let controls = cx.new(|cx| NativeComposerControls::new(snapshot, cx));
        let picker = cx.new(|cx| {
            NativeModelSelector::new(
                NativeModelCatalog::from_manifest_json(include_str!(
                    "../../../tests/fixtures/model_catalog.json"
                ))
                .unwrap(),
                None,
                ThemeMode::Dark,
                cx,
            )
        });
        let mut composer = NativeComposer::new(cx);
        composer.set_components(&controls, &picker, cx);
        composer
    });
    (view, window_cx)
}

#[gpui::test]
fn composer_controls_keep_equal_edge_insets_as_the_draft_grows(cx: &mut TestAppContext) {
    let (view, cx) = mount_composer(cx, NativeComposerControlsSnapshot::default());
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
            });
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
        // Reference (`thread-composer.svelte:553`): card `p-2`, so every
        // control edge sits exactly 8px inside the card.
        for inset in [left, right, picker_bottom, primary_bottom] {
            assert_eq!(
                inset,
                px(8.0),
                "expected 8px card padding, got {inset:?} for {draft:?}; card={card:?} picker={picker:?} send={primary:?}"
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

/// The compact empty box keeps 8px card padding, a 64px editor, and a 32px
/// control row. Their combined height is 112px without unused flex space.
#[gpui::test]
fn composer_empty_card_has_no_extra_flex_space(cx: &mut TestAppContext) {
    let (_view, cx) = mount_composer(cx, NativeComposerControlsSnapshot::default());
    cx.simulate_resize(size(px(900.0), px(600.0)));
    cx.run_until_parked();

    let card = cx.debug_bounds("artisan-native-composer").unwrap();
    assert_eq!(
        card.size.height,
        px(112.0),
        "empty composer should fit its editor and controls, got {card:?}"
    );
    let editor = cx.debug_bounds(NATIVE_COMPOSER_EDITOR_SELECTOR).unwrap();
    assert_eq!(
        editor.size.height,
        px(64.0),
        "empty editor should keep its 64px base, got {editor:?}"
    );
    assert_eq!(editor.top() - card.top(), px(8.0));
    assert_eq!(editor.left() - card.left(), px(8.0));
    assert_eq!(card.right() - editor.right(), px(8.0));

    // Reference placeholder (`thread-composer.svelte:562`): `inset-x-3
    // top-2` inside the padded editor, shown while the draft is empty.
    let placeholder = cx
        .debug_bounds("artisan-native-composer-placeholder")
        .expect("empty draft shows the placeholder");
    assert_eq!(placeholder.top() - editor.top(), px(8.0));
    assert_eq!(placeholder.left() - editor.left(), px(12.0));

    let row = cx
        .debug_bounds(NATIVE_COMPOSER_CONTROL_ROW_SELECTOR)
        .unwrap();
    assert_eq!(row.size.height, px(32.0));
    assert_eq!(card.bottom() - row.bottom(), px(8.0));
}

/// A typed draft hides the placeholder; the editor keeps its box.
#[gpui::test]
fn composer_hides_the_placeholder_once_drafted(cx: &mut TestAppContext) {
    let (view, cx) = mount_composer(cx, NativeComposerControlsSnapshot::default());
    cx.simulate_resize(size(px(900.0), px(600.0)));
    cx.update(|_, app| {
        view.update(app, |composer, cx| {
            composer.set_draft("hello");
            cx.notify();
        });
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("artisan-native-composer-placeholder")
            .is_none(),
        "a nonempty draft must not paint the placeholder"
    );
}

/// Reference (`thread-composer.svelte:571-587`): the editor is uncapped with
/// no internal scroll — long drafts grow the overlay card instead. A 12-line
/// draft paints 12 × 24px = 288px of text plus the editor's own py-8 padding
/// (16px) for 304px of editor, so the card settles at 8 + 304 + 32 + 8 =
/// 352px. Tail clearance below the grown card is the transcript end space
/// (surface lane), never a reinstated cap.
#[gpui::test]
fn composer_grows_uncapped_with_long_drafts(cx: &mut TestAppContext) {
    let (view, cx) = mount_composer(cx, NativeComposerControlsSnapshot::default());
    cx.simulate_resize(size(px(900.0), px(600.0)));
    let draft = (1..=12)
        .map(|line| format!("Line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    cx.update(|_, app| {
        view.update(app, |composer, cx| {
            composer.set_draft(draft.as_str());
            cx.notify();
        });
    });
    cx.run_until_parked();
    let card = cx.debug_bounds("artisan-native-composer").unwrap();
    let editor = cx.debug_bounds(NATIVE_COMPOSER_EDITOR_SELECTOR).unwrap();
    assert_eq!(
        editor.size.height,
        px(304.0),
        "long drafts grow the editor past the old cap, got {editor:?}"
    );
    assert_eq!(
        card.size.height,
        px(352.0),
        "uncapped card must reach 352px, got {card:?}"
    );
}

/// Lip rows (`steering-lip.svelte:28-32`, `button.svelte:24`): 16px text on
/// a 24px line with 8px vertical padding. A recallable row also mounts two
/// `icon-sm` (32px) buttons, which bind the row content above the 24px
/// text: 16 + max(24, 32) = 48px. An informing-only row with no actions
/// paints the bare 16 + 24 = 40px.
#[gpui::test]
fn composer_lip_row_matches_reference_geometry(cx: &mut TestAppContext) {
    let snapshot = NativeComposerControlsSnapshot {
        pending_steering: vec![PendingSteeringRow::new("cmd-1", 7, "  Keep this  ", true)],
        ..NativeComposerControlsSnapshot::default()
    };
    let (_view, cx) = mount_composer(cx, snapshot);
    cx.simulate_resize(size(px(900.0), px(600.0)));
    cx.run_until_parked();

    let lip = cx
        .debug_bounds("artisan-native-composer-steering-lip")
        .expect("a pending steer paints the lip");
    let row = cx
        .debug_bounds("artisan-native-composer-steering-row-cmd-1-7")
        .expect("the queued steer paints exactly one row");
    assert_eq!(
        row.size.height,
        px(48.0),
        "recallable lip row must be 48px tall, got {row:?}"
    );
    assert_eq!(row.left(), lip.left());
    assert_eq!(row.right(), lip.right());
}

/// An informing-only row carries no 32px actions, so the 24px text binds:
/// 16 + 24 = 40px.
#[gpui::test]
fn composer_informing_lip_row_matches_reference_geometry(cx: &mut TestAppContext) {
    let snapshot = NativeComposerControlsSnapshot {
        pending_steering: vec![PendingSteeringRow::new("cmd-2", 3, "Attached image", false)],
        ..NativeComposerControlsSnapshot::default()
    };
    let (_view, cx) = mount_composer(cx, snapshot);
    cx.simulate_resize(size(px(900.0), px(600.0)));
    cx.run_until_parked();

    let row = cx
        .debug_bounds("artisan-native-composer-steering-row-cmd-2-3")
        .expect("the informing steer paints exactly one row");
    assert_eq!(
        row.size.height,
        px(40.0),
        "informing lip row must be 40px tall, got {row:?}"
    );
}
