#[path = "../../src/shell/refresh.rs"]
mod implementation;

use implementation::Refresh;

#[test]
fn transcript_animation_and_clock_changes_stay_inside_the_transcript() {
    for causes in [
        vec!["anim_clock"],
        vec!["views_frame", "after_layout", "al_motion"],
        vec!["views_frame", "al_compaction", "al_reveal", "al_stored"],
        vec!["landing_expiry", "reveals"],
    ] {
        assert_eq!(Refresh::from_causes(&causes), Refresh::Transcript);
    }
}

#[test]
fn independent_sidebar_and_transcript_updates_refresh_both_sections() {
    assert_eq!(
        Refresh::from_causes(&["sidebar_motion", "sidebar_tick"]),
        Refresh::Sidebar
    );
    assert_eq!(
        Refresh::from_causes(&["sidebar_motion", "after_layout"]),
        Refresh::Sections
    );
    assert_eq!(
        Refresh::Sidebar.merge(Refresh::Transcript),
        Refresh::Transcript.merge(Refresh::Sidebar)
    );
}

#[test]
fn unknown_or_layout_changes_keep_the_full_workspace_fallback() {
    for causes in [
        vec![],
        vec!["views_frame"],
        vec!["anim_sidebar_settled", "anim_clock"],
        vec!["anim_clock", "theme"],
        vec!["anim_events"],
    ] {
        assert_eq!(Refresh::from_causes(&causes), Refresh::Workspace);
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
    assert_eq!(
        Refresh::from_causes(&["anim_sidebar"]),
        Refresh::SidebarFrame
    );
    assert_eq!(
        Refresh::from_causes(&["anim_sidebar_resize", "sidebar_tick"]),
        Refresh::SidebarFrame
    );
    assert_eq!(
        Refresh::from_causes(&["anim_edges", "anim_clock"]),
        Refresh::FrameSections
    );
}
