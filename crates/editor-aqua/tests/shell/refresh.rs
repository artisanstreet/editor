use editor_aqua::shell::refresh::{Cause, Invalidation, Refresh};

fn refresh(causes: &[Cause]) -> Refresh {
    let mut invalidation = Invalidation::default();
    for cause in causes {
        invalidation.note(*cause, true);
    }
    invalidation.refresh()
}

#[test]
fn transcript_animation_and_clock_changes_stay_inside_the_transcript() {
    for causes in [
        vec![Cause::WorkClock],
        vec![Cause::ViewsFrame, Cause::AfterLayout, Cause::LayoutMotion],
        vec![Cause::ViewsFrame, Cause::LayoutMotion],
        vec![Cause::LandingExpiry, Cause::Reveals],
    ] {
        assert_eq!(refresh(&causes), Refresh::Transcript);
    }
}

#[test]
fn rows_revealed_by_scrolling_refresh_only_the_transcript() {
    assert_eq!(
        refresh(&[Cause::ViewsFrame, Cause::Viewport, Cause::Rows]),
        Refresh::Transcript
    );
}

#[test]
fn independent_sidebar_and_transcript_updates_refresh_both_sections() {
    assert_eq!(
        refresh(&[Cause::SidebarMotion, Cause::SidebarTick]),
        Refresh::Sidebar
    );
    assert_eq!(
        refresh(&[Cause::SidebarMotion, Cause::AfterLayout]),
        Refresh::Sections
    );
    assert_eq!(
        Refresh::Sidebar.merge(Refresh::Transcript),
        Refresh::Transcript.merge(Refresh::Sidebar)
    );
}

#[test]
fn unattributed_or_layout_changes_keep_the_full_workspace_fallback() {
    for causes in [
        vec![],
        vec![Cause::ViewsFrame],
        vec![Cause::SidebarSettled, Cause::WorkClock],
        vec![Cause::WorkClock, Cause::Titlebar],
        vec![Cause::SidebarEvents],
    ] {
        assert_eq!(refresh(&causes), Refresh::Workspace);
    }
}

#[test]
fn combining_update_scopes_is_associative_and_workspace_changes_dominate() {
    for a in [
        Refresh::None,
        Refresh::Sidebar,
        Refresh::SidebarFrame,
        Refresh::Transcript,
        Refresh::Sections,
        Refresh::FrameSections,
        Refresh::Workspace,
    ] {
        for b in [
            Refresh::None,
            Refresh::Sidebar,
            Refresh::SidebarFrame,
            Refresh::Transcript,
            Refresh::Sections,
            Refresh::FrameSections,
            Refresh::Workspace,
        ] {
            for c in [
                Refresh::None,
                Refresh::Sidebar,
                Refresh::SidebarFrame,
                Refresh::Transcript,
                Refresh::Sections,
                Refresh::FrameSections,
                Refresh::Workspace,
            ] {
                assert_eq!(a.merge(b).merge(c), a.merge(b.merge(c)));
                assert_eq!(a.merge(Refresh::Workspace), Refresh::Workspace);
            }
        }
    }
}

#[test]
fn sidebar_frame_animation_updates_the_chrome_and_combines_with_transcript_changes() {
    assert_eq!(refresh(&[Cause::SidebarAnimation]), Refresh::SidebarFrame);
    assert_eq!(
        refresh(&[Cause::SidebarResize, Cause::SidebarTick]),
        Refresh::SidebarFrame
    );
    assert_eq!(
        refresh(&[Cause::SidebarResize, Cause::WorkClock]),
        Refresh::FrameSections
    );
}

#[test]
fn stored_reply_traffic_refreshes_only_the_transcript() {
    for causes in [
        vec![Cause::StoredRows],
        vec![Cause::RowSources],
        vec![Cause::RowsInvalidated],
        vec![
            Cause::ViewsFrame,
            Cause::StoredRows,
            Cause::RowSources,
            Cause::RowsInvalidated,
            Cause::ThreadItems,
        ],
    ] {
        assert_eq!(refresh(&causes), Refresh::Transcript, "{causes:?}");
    }
    assert_eq!(
        refresh(&[Cause::StoredRows, Cause::SidebarTick]),
        Refresh::Sections
    );
}

#[test]
fn shell_wide_conversation_invalidation_still_rebuilds_the_workspace() {
    assert_eq!(
        refresh(&[Cause::RowsInvalidated, Cause::ConversationInvalidated]),
        Refresh::Workspace
    );
}
