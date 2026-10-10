//! Reopening a thread puts the reader back where they left its transcript.

use super::windowing::long_scene;
use super::*;

const VIEWPORT_SIZE: (f32, f32) = (720.0, 480.0);

/// Mounts the long fixture, scrolls the reader to the middle, and returns
/// the offset they are at.
fn read_to_the_middle(
    surface: &Entity<ConversationSurface>,
    cx: &mut VisualTestContext,
) -> gpui::Point<gpui::Pixels> {
    cx.simulate_resize(size(px(VIEWPORT_SIZE.0), px(VIEWPORT_SIZE.1)));
    settle(cx);
    let handle = cx.update(|_, app| surface.read(app).scroll_handle().clone());
    let middle = handle.max_offset().y / 2.0;
    handle.set_offset(point(px(0.0), -middle));
    cx.update(|_, app| surface.update(app, |_, cx| cx.notify()));
    settle(cx);
    offset(surface, cx)
}

/// Replaces the window's surface with a fresh one over the same scene, as
/// reopening the thread does.
fn reopen(cx: &mut VisualTestContext) -> Entity<ConversationSurface> {
    let surface = cx.update(|window, app| {
        window.replace_root(app, |_, cx| {
            ConversationSurface::new(long_scene(120), ThemeMode::Dark, cx)
        })
    });
    settle(cx);
    surface
}

#[gpui::test]
fn a_reopened_transcript_opens_where_the_reader_left_it(cx: &mut TestAppContext) {
    let (surface, cx) =
        cx.add_window_view(|_, cx| ConversationSurface::new(long_scene(120), ThemeMode::Dark, cx));
    let left_at = read_to_the_middle(&surface, cx);
    let position = cx
        .update(|_, app| surface.read(app).reading_position())
        .expect("a reader in the middle has a position");

    let reopened = reopen(cx);
    let restored = cx.update(|_, app| {
        reopened.update(app, |surface, cx| {
            surface.restore_reading_position(position.clone(), cx)
        })
    });
    assert!(restored, "the position's turn is in the scene");
    settle(cx);

    assert_eq!(
        offset(&reopened, cx),
        left_at,
        "the reader is back on the exact content they left"
    );
    assert_eq!(
        cx.update(|_, app| reopened.read(app).reading_position()),
        Some(position),
        "leaving again remembers the same place"
    );
    let detached = cx.update(|_, app| {
        reopened.read(app).pending_actions().iter().any(|action| {
            matches!(
                action,
                ConversationSurfaceAction::ViewportObserved(observation) if !observation.at_bottom
            )
        })
    });
    assert!(
        detached,
        "the controller hears the reader is away from the end, so nothing follows"
    );
}

#[gpui::test]
fn a_follow_queued_before_the_restore_does_not_pull_the_reader_away(cx: &mut TestAppContext) {
    let (surface, cx) =
        cx.add_window_view(|_, cx| ConversationSurface::new(long_scene(120), ThemeMode::Dark, cx));
    let left_at = read_to_the_middle(&surface, cx);
    let position = cx
        .update(|_, app| surface.read(app).reading_position())
        .expect("a reader in the middle has a position");

    let reopened = reopen(cx);
    cx.update(|_, app| {
        reopened.update(app, |surface, cx| {
            // The snapshot's own follow to the end reaches the surface
            // first, exactly as the application pumps it.
            surface.follow_to_bottom(cx);
            surface.restore_reading_position(position, cx);
        });
    });
    settle(cx);

    assert_eq!(offset(&reopened, cx), left_at);
    assert!(cx.update(|_, app| reopened.read(app).bottom_glide.is_none()));
}

#[gpui::test]
fn a_reader_at_the_end_keeps_no_position(cx: &mut TestAppContext) {
    let (surface, cx) =
        cx.add_window_view(|_, cx| ConversationSurface::new(long_scene(120), ThemeMode::Dark, cx));
    cx.simulate_resize(size(px(VIEWPORT_SIZE.0), px(VIEWPORT_SIZE.1)));
    settle(cx);
    cx.update(|_, app| surface.update(app, ConversationSurface::scroll_to_bottom));
    settle(cx);

    assert_eq!(
        cx.update(|_, app| surface.read(app).reading_position()),
        None,
        "the end is where a thread opens anyway"
    );
}

#[gpui::test]
fn a_position_in_a_turn_that_is_gone_is_not_restored(cx: &mut TestAppContext) {
    let (surface, cx) =
        cx.add_window_view(|_, cx| ConversationSurface::new(long_scene(120), ThemeMode::Dark, cx));
    let left_at = read_to_the_middle(&surface, cx);
    let position = cx
        .update(|_, app| surface.read(app).reading_position())
        .expect("a reader in the middle has a position");

    let reopened = cx.update(|window, app| {
        window.replace_root(app, |_, cx| {
            ConversationSurface::new(long_scene(10), ThemeMode::Dark, cx)
        })
    });
    settle(cx);
    let restored = cx.update(|_, app| {
        reopened.update(app, |surface, cx| {
            surface.restore_reading_position(position, cx)
        })
    });

    assert!(!restored);
    assert_ne!(offset(&reopened, cx), left_at);
}
