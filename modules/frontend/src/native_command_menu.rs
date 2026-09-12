//! Native GPUI command menu for the rail (`Cmd/Ctrl+K` palette).
//!
//! Native counterpart of `routes/components/command-menu.svelte`, minus the
//! Effect controllers, router, and draft preparation, which the orchestrator
//! owns. The legacy surface is a Bits `CommandDialog` with a `CommandInput`
//! (`placeholder="Search threads and actions…"`, `CommandEmpty` copy `"No
//! results found."`), a static `Actions` group (`New thread`, `Open
//! settings`), and one `CommandGroup` per project holding that project's
//! threads with the searchable value
//! `` `${display_title} ${thread.title} ${thread.thread_id}` ``.
//!
//! [`CommandMenuState`] is the dependency-free interaction contract: open
//! trigger state, controlled query, ranked groups, keyboard movement, and
//! single-activation emission. Ranking reuses
//! [`crate::command_ranking::filter_and_rank_groups`] so matching, scoring,
//! and group ordering cannot drift from the audited scorer. [`NativeCommandMenu`]
//! is the thin GPUI view over that state.
//!
//! Fidelity mapping (legacy element → this module, Tailwind → Styled notes):
//!
//! - `CommandDialog` → the active native route uses one anchored titlebar
//!   dropdown; the retained centered `deferred` card remains available to
//!   callers of the legacy presentation and uses the shared
//!   [`artisan_ui::popover`] recipe.
//! - `CommandInput` → [`artisan_ui::input::Input`] plus
//!   [`NativeCommandMenuInputElement`], which registers GPUI's native text
//!   service for typing, paste, selection, and IME composition.
//! - `CommandGroup[heading]` → heading text rows in group order; the ranked
//!   group order comes from the scorer, matching Bits' `filter` + rank
//!   behavior. `rounded-sm` rows map onto the shared
//!   [`artisan_ui::list_row`] menu recipe (`px-2 py-1.5`, `--radius-xl`);
//!   the ramp step differs from `rounded-sm` and is named in code.
//! - `CommandItem` leading glyphs → [`artisan_ui::icon::icon`]: `Edit`
//!   becomes `TABLER_EDIT`, `Settings` becomes `TABLER_SETTINGS`, and
//!   `MessageCircle` becomes `TABLER_MESSAGE_CIRCLE`.
//! - `CommandEmpty` → the exact `"No results found."` copy in muted text.
//! - Toggle shortcut (`meta/ctrl+k`) → [`CommandMenuState::press_toggle`]
//!   plus [`CommandMenuState::matches_toggle_shortcut`]; the application binds
//!   the same titlebar entity with an open-or-focus command and restores the
//!   application root after dismissal.
//! - Activation (`StartNewThread` navigation, settings link, thread links) →
//!   [`CommandMenuAction`], drained once through
//!   [`CommandMenuState::take_pending_action`]; navigation and draft effects
//!   stay with the orchestrator.
//!
//! Deliberately absent: [`artisan_ui::button`] and [`artisan_ui::badge`]
//! have no counterpart in the legacy command menu (its trigger lives outside
//! this surface and its rows carry no badges), so they are not used here.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::{ops::Range, panic};

use artisan_assets::AssetId;
use artisan_ui::{
    asset_seam::asset_glyph,
    icon::{IconSize, IconStyle, IconTint, icon},
    input::Input,
    list_row::{ListRowContent, ListRowGeometry, ListRowStyle, ListRowTone, list_row},
    popover::{PopoverStyle, popover_content},
    theme::{ArtisanTheme, DesktopTheme, ThemeMode},
};
use gpui::{
    AnyElement, App, Bounds, ClickEvent, ClipboardItem, Context, Div, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, FocusHandle, FontWeight, GlobalElementId,
    InspectorElementId, InteractiveElement as _, KeyBinding, KeyDownEvent, LayoutId,
    ParentElement as _, Pixels, Point, Render, ScrollHandle, SharedString, Size, Stateful,
    StatefulInteractiveElement as _, Styled as _, UTF16Selection, Window, actions, deferred, div,
    prelude::{FluentBuilder as _, IntoElement},
    px,
};

use crate::command_ranking::{CommandGroup, CommandItem, filter_and_rank_groups};

actions!(
    native_command_menu,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Paste,
        Copy,
        Cut,
        Home,
        End,
    ]
);

/// Stable debug selector for the command-menu root.
pub const COMMAND_MENU_SELECTOR: &str = "artisan-native-command-menu";
/// Stable debug selector for the query input branch.
pub const COMMAND_MENU_INPUT_SELECTOR: &str = "artisan-native-command-menu-input";
/// Stable debug selector for the ranked list branch.
pub const COMMAND_MENU_LIST_SELECTOR: &str = "artisan-native-command-menu-list";
/// Stable debug selector for the anchored titlebar dropdown.
pub const COMMAND_MENU_DROPDOWN_SELECTOR: &str = "artisan-native-command-menu-dropdown";
/// Prefix for the stable selectors painted on command rows.
pub const COMMAND_MENU_ROW_SELECTOR_PREFIX: &str = "artisan-native-command-menu-row";
/// Exact legacy input placeholder (`command-menu.svelte`).
pub const COMMAND_MENU_PLACEHOLDER: &str = "Search threads and actions…";
/// Exact legacy empty-list copy (`CommandEmpty`).
pub const COMMAND_MENU_EMPTY_LABEL: &str = "No results found.";
/// The toggle shortcut key (`meta/ctrl+k` in the legacy window handler).
pub const COMMAND_MENU_SHORTCUT_KEY: &str = "k";
/// Key context for the shared titlebar query input.
const COMMAND_MENU_KEY_CONTEXT: &str = "artisan-native-command-menu";
/// Stable identity of the static actions group.
pub const ACTIONS_GROUP_ID: &str = "actions";
/// Exact legacy heading of the static actions group.
pub const ACTIONS_GROUP_HEADING: &str = "Actions";
/// Stable identity of the new-thread action row.
pub const NEW_THREAD_ITEM_ID: &str = "new-thread";
/// Stable identity of the open-settings action row.
pub const OPEN_SETTINGS_ITEM_ID: &str = "open-settings";
/// Prefix used for project rows in the live project catalog.
pub const PROJECT_ITEM_ID_PREFIX: &str = "project-";
/// Exact legacy new-thread action label.
pub const NEW_THREAD_LABEL: &str = "New task";
/// Exact legacy open-settings action label.
pub const OPEN_SETTINGS_LABEL: &str = "Open settings";
/// Heading used for threads without a project (`command-menu.svelte`).
pub const UNASSIGNED_GROUP_HEADING: &str = "Unassigned";

/// Preferred dialog width in logical pixels (legacy `sm:max-w-lg`, 32 rem).
const MENU_WIDTH_PX: f32 = 512.0;
/// Bounded list height so long thread catalogs scroll instead of overflowing.
const MENU_LIST_MAX_HEIGHT_PX: f32 = 320.0;
/// Dialog top offset keeps the palette in the upper half like a command bar.
const MENU_TOP_OFFSET_PX: f32 = 96.0;

// Phase-1 split submodules (see native_command_menu/).

#[path = "native_command_menu/state.rs"]
mod state;

#[path = "native_command_menu/view.rs"]
mod view;

pub use state::*;
pub use view::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_groups() -> Vec<CommandMenuGroup> {
        vec![
            CommandMenuGroup::actions(),
            CommandMenuGroup::new(
                "project-a",
                "Project A",
                vec![
                    CommandMenuEntry::thread("thread-1", "Ship the port", "old title one"),
                    CommandMenuEntry::thread("thread-2", "Fix the rail", "old title two"),
                ],
            ),
        ]
    }

    #[test]
    fn toggle_shortcut_matches_only_meta_or_ctrl_k() {
        assert!(CommandMenuState::matches_toggle_shortcut("k", true, false));
        assert!(CommandMenuState::matches_toggle_shortcut("k", false, true));
        assert!(!CommandMenuState::matches_toggle_shortcut(
            "k", false, false
        ));
        assert!(!CommandMenuState::matches_toggle_shortcut("j", true, false));
        assert!(!CommandMenuState::matches_toggle_shortcut(
            "kk", true, false
        ));
    }

    #[test]
    fn open_close_cycle_resets_query_and_highlight() {
        let mut state = CommandMenuState::new(fixture_groups());
        assert!(!state.is_open());
        state.open();
        assert!(state.is_open());
        state.set_query("ship");
        assert_eq!(state.row_count(), 1);
        state.dismiss();
        assert!(!state.is_open());
        assert!(state.highlighted_flat().is_none());
        state.open();
        assert_eq!(state.query(), "");
        assert_eq!(state.highlighted_flat(), Some(0));
    }

    #[test]
    fn blank_query_keeps_catalog_order_and_filters_on_text() {
        let mut state = CommandMenuState::new(fixture_groups());
        state.open();
        assert_eq!(state.row_count(), 4);
        assert_eq!(
            state.highlighted_action(),
            Some(CommandMenuAction::NewThread)
        );
        state.set_query("ship");
        assert_eq!(state.row_count(), 1);
        assert_eq!(
            state.highlighted_action(),
            Some(CommandMenuAction::OpenThread {
                thread_id: String::from("thread-1")
            })
        );
        state.set_query("thread-2");
        assert_eq!(state.row_count(), 1);
        state.set_query("zzz-no-match");
        assert_eq!(state.row_count(), 0);
        assert!(state.highlighted_action().is_none());
    }

    #[test]
    fn keyboard_movement_wraps_and_jumps() {
        let mut state = CommandMenuState::new(fixture_groups());
        state.open();
        assert_eq!(state.row_count(), 4);
        state.move_previous();
        assert_eq!(state.highlighted_flat(), Some(3));
        state.move_next();
        assert_eq!(state.highlighted_flat(), Some(0));
        state.move_last();
        assert_eq!(state.highlighted_flat(), Some(3));
        state.move_first();
        assert_eq!(state.highlighted_flat(), Some(0));
        state.dismiss();
        state.move_next();
        assert!(state.highlighted_flat().is_none());
    }

    #[test]
    fn activation_closes_first_and_emits_exactly_once() {
        let mut state = CommandMenuState::new(fixture_groups());
        state.open();
        state.move_next();
        state.move_next();
        let action = state.activate_highlighted();
        assert_eq!(
            action,
            Some(CommandMenuAction::OpenThread {
                thread_id: String::from("thread-1")
            })
        );
        assert!(!state.is_open());
        assert!(state.activate_highlighted().is_none());
        assert!(state.activate_row(0).is_none());
        assert_eq!(
            state.pending_action(),
            Some(&CommandMenuAction::OpenThread {
                thread_id: String::from("thread-1")
            })
        );
        assert_eq!(
            state.take_pending_action(),
            Some(CommandMenuAction::OpenThread {
                thread_id: String::from("thread-1")
            })
        );
        assert!(state.take_pending_action().is_none());
    }

    #[test]
    fn typed_emission_resolves_stable_thread_identity() {
        let mut state = CommandMenuState::new(fixture_groups());
        state.open();
        for character in "fix".chars() {
            state.push_query_char(character);
        }
        assert_eq!(state.query(), "fix");
        assert_eq!(
            state.activate_highlighted(),
            Some(CommandMenuAction::OpenThread {
                thread_id: String::from("thread-2")
            })
        );
        assert!(!state.pop_query_char());
    }

    #[test]
    fn project_rows_are_live_searchable_actions() {
        let mut state = CommandMenuState::new(vec![CommandMenuGroup::new(
            "projects",
            "Projects",
            vec![CommandMenuEntry::project("project-a", "Artisan")],
        )]);
        state.open();
        state.set_query("project-a");
        assert_eq!(state.row_count(), 1);
        assert_eq!(
            state.activate_highlighted(),
            Some(CommandMenuAction::OpenProject {
                project_id: String::from("project-a")
            })
        );
    }

    #[test]
    fn native_input_ranges_keep_utf16_and_utf8_boundaries_aligned() {
        let text = "A😀é";
        assert_eq!(utf8_offset_to_utf16(text, 0), Some(0));
        assert_eq!(utf8_offset_to_utf16(text, 1), Some(1));
        assert_eq!(utf8_offset_to_utf16(text, 5), Some(3));
        assert_eq!(utf8_offset_to_utf16(text, text.len()), Some(4));
        assert_eq!(utf16_offset_to_utf8(text, 2), None);
        assert_eq!(utf16_offset_to_utf8(text, 3), Some(5));
        assert_eq!(previous_character_boundary(text, 5), 1);
        assert_eq!(next_character_boundary(text, 1), 5);
    }

    #[test]
    fn disabled_refuses_open_and_discards_pending() {
        let mut state = CommandMenuState::new(fixture_groups());
        state.open();
        let _ = state.activate_highlighted();
        assert!(state.pending_action().is_some());
        state.set_disabled(true);
        assert!(!state.is_open());
        assert!(state.pending_action().is_none());
        state.set_disabled(false);
        state.press_toggle();
        assert!(state.is_open());
        state.press_toggle();
        assert!(!state.is_open());
    }
}
