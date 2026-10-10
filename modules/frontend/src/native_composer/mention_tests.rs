//! The mention menu and mention editing, driven through real keystrokes.

#![forbid(unsafe_code)]

use std::{cell::Cell, rc::Rc};

use super::{NativeComposer, NativeComposerEvent};
use crate::composer_mentions::{GOAL_COMMAND, MentionCatalog, MentionTarget};
use gpui::{Entity, Subscription, TestAppContext, VisualTestContext, px, size};

const MENU: &str = "artisan-composer-mention-menu";

fn target(name: &str) -> MentionTarget {
    MentionTarget {
        name: name.to_owned(),
        description: format!("About {name}"),
    }
}

fn mounted(cx: &mut TestAppContext) -> (Entity<NativeComposer>, &mut VisualTestContext) {
    let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
    cx.simulate_resize(size(px(900.0), px(700.0)));
    cx.update(|window, app| {
        NativeComposer::bind_actions(app);
        view.update(app, |composer, composer_cx| {
            composer.set_mention_catalog(
                MentionCatalog::new(
                    vec![target("apple-design"), target("animation-vocabulary")],
                    vec![target("sentry")],
                    vec![GOAL_COMMAND],
                ),
                composer_cx,
            );
        });
        let focus = view.read(app).focus_handle.clone();
        window.focus(&focus, app);
    });
    cx.run_until_parked();
    (view, cx)
}

fn type_draft(cx: &mut VisualTestContext, view: &Entity<NativeComposer>, draft: &str) {
    let draft = draft.to_owned();
    cx.update(|_, app| {
        view.update(app, |composer, composer_cx| {
            let end = composer.state.draft().len();
            composer.replace_range(0..end, &draft, None, composer_cx);
        });
    });
    cx.run_until_parked();
}

fn draft(cx: &mut VisualTestContext, view: &Entity<NativeComposer>) -> String {
    cx.update(|_, app| view.read(app).state.draft().to_owned())
}

/// Whether the menu offers rows; a closing menu stays painted for its
/// exit, so this reads the composer rather than the painted tree.
fn menu_open(cx: &mut VisualTestContext, view: &Entity<NativeComposer>) -> bool {
    cx.update(|_, app| view.update(app, |composer, _| composer.mention_menu_offers_rows()))
}

fn count_sends(
    cx: &mut VisualTestContext,
    view: &Entity<NativeComposer>,
) -> (Rc<Cell<usize>>, Subscription) {
    let sends = Rc::new(Cell::new(0));
    let counter = Rc::clone(&sends);
    let subscription = cx.update(|_, app| {
        app.subscribe(view, move |_, event: &NativeComposerEvent, _| {
            if matches!(event, NativeComposerEvent::SendRequested) {
                counter.set(counter.get() + 1);
            }
        })
    });
    (sends, subscription)
}

#[gpui::test]
fn a_sigil_opens_the_menu_and_enter_writes_the_mention(cx: &mut TestAppContext) {
    let (view, cx) = mounted(cx);
    let (sends, _subscription) = count_sends(cx, &view);
    type_draft(cx, &view, "Polish with $app");
    assert!(menu_open(cx, &view), "a skill trigger opens the menu");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(draft(cx, &view), "Polish with $apple-design ");
    assert_eq!(sends.get(), 0, "choosing a row never sends");
    assert!(!menu_open(cx, &view), "the written mention closes the menu");
}

#[gpui::test]
fn every_sigil_opens_the_same_menu(cx: &mut TestAppContext) {
    let (view, cx) = mounted(cx);
    for sigil in ["/", "$", "@"] {
        type_draft(cx, &view, sigil);
        assert!(menu_open(cx, &view), "{sigil} opens the menu");
    }
    type_draft(cx, &view, "@sen");
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    assert_eq!(draft(cx, &view), "@sentry ");
}

#[gpui::test]
fn the_menu_paints_above_the_composer_and_lists_no_commands(cx: &mut TestAppContext) {
    let (view, cx) = mounted(cx);
    type_draft(cx, &view, "/");
    let menu = cx
        .debug_bounds(MENU)
        .expect("the menu paints while it opens");
    let editor = cx
        .debug_bounds(super::NATIVE_COMPOSER_EDITOR_SELECTOR)
        .expect("editor");
    assert!(
        menu.bottom() <= editor.top(),
        "the menu hangs over the composer"
    );
    // `/goa` names no skill or connector, so nothing is offered and enter
    // sends the text as written.
    type_draft(cx, &view, "/goa");
    assert!(!menu_open(cx, &view));
}

#[gpui::test]
fn escape_closes_the_menu_until_the_word_changes(cx: &mut TestAppContext) {
    let (view, cx) = mounted(cx);
    let (sends, _subscription) = count_sends(cx, &view);
    type_draft(cx, &view, "costs $app");
    assert!(menu_open(cx, &view));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!menu_open(cx, &view), "escape closes the menu");
    cx.simulate_keystrokes("enter");
    assert_eq!(sends.get(), 1, "with the menu closed enter sends");
    type_draft(cx, &view, "costs $ani");
    assert!(menu_open(cx, &view), "a new word opens it again");
}

#[gpui::test]
fn text_no_entry_matches_keeps_the_menu_closed(cx: &mut TestAppContext) {
    let (view, cx) = mounted(cx);
    type_draft(cx, &view, "costs $5");
    assert!(!menu_open(cx, &view));
    type_draft(cx, &view, "see src/lib.rs");
    assert!(!menu_open(cx, &view));
}

#[gpui::test]
fn arrows_move_the_highlight_instead_of_the_caret(cx: &mut TestAppContext) {
    let (view, cx) = mounted(cx);
    type_draft(cx, &view, "$a");
    cx.simulate_keystrokes("down");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let written = draft(cx, &view);
    assert!(
        written == "$apple-design " || written == "$animation-vocabulary ",
        "the second skill row was chosen: {written}"
    );
    assert_ne!(
        written, "$a",
        "down moved the highlight, not the caret, and enter chose a row"
    );
}

#[gpui::test]
fn backspace_removes_a_mention_whole(cx: &mut TestAppContext) {
    let (view, cx) = mounted(cx);
    type_draft(cx, &view, "hi $apple-design");
    cx.simulate_keystrokes("escape");
    cx.simulate_keystrokes("backspace");
    cx.run_until_parked();
    assert_eq!(draft(cx, &view), "hi ");
}

#[gpui::test]
fn arrows_step_over_a_mention(cx: &mut TestAppContext) {
    let (view, cx) = mounted(cx);
    type_draft(cx, &view, "$apple-design x");
    cx.simulate_keystrokes("left left");
    cx.run_until_parked();
    let caret = cx.update(|_, app| view.read(app).selection.clone());
    assert_eq!(caret, 13..13, "two lefts reach the mention's end");
    cx.simulate_keystrokes("left");
    cx.run_until_parked();
    let caret = cx.update(|_, app| view.read(app).selection.clone());
    assert_eq!(caret, 0..0, "one more steps over the whole mention");
}

#[gpui::test]
fn goal_paints_capitalized_while_the_draft_keeps_the_command(cx: &mut TestAppContext) {
    let (view, cx) = mounted(cx);
    type_draft(cx, &view, "/goal ship it");
    let painted = cx.update(|_, app| {
        let composer = view.read(app);
        let theme = artisan_ui::theme::ArtisanTheme::for_mode(artisan_ui::theme::ThemeMode::Dark);
        composer
            .composer_text(&theme, artisan_ui::theme::DesktopTheme::neutral_dark())
            .painted_text()
            .to_owned()
    });
    assert_eq!(painted, "/Goal ship it");
    assert_eq!(draft(cx, &view), "/goal ship it");
}
