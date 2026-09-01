//! White-box coverage for the controlled native command palette primitive.

#![allow(clippy::float_cmp)]

use std::cell::RefCell;
use std::rc::Rc;

use crate::button::FocusVisibility;
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens, SurfaceStep, ThemeMode};
use gpui::{
    Context, FocusHandle, IntoElement, Modifiers, ParentElement, Render, SharedString, Styled,
    TestAppContext, Window, div, point, px,
};

use super::{
    COMMAND_PALETTE_ROLE, CommandActivation, CommandGroup, CommandItem, CommandPalette,
    CommandStyle, DEFAULT_DEBUG_SELECTOR, DEFAULT_EMPTY_LABEL, activation_for_id,
    adjacent_group_id, navigation_target, resolved_highlight_id, visible_group_indices,
};

const ROOT_SELECTOR: &str = "command-palette-under-test";
const INPUT_SELECTOR: &str = "command-palette-under-test-input";
const LIST_SELECTOR: &str = "command-palette-under-test-list";
const EMPTY_SELECTOR: &str = "command-palette-under-test-empty";

fn shared(value: &str) -> SharedString {
    value.to_owned().into()
}

fn item(id: &str, label: &str) -> CommandItem {
    CommandItem::new(shared(id), shared(label))
}

fn group(id: &str, heading: Option<&str>, items: Vec<CommandItem>) -> CommandGroup {
    let mut value = CommandGroup::new(shared(id), items);
    if let Some(heading) = heading {
        value = value.heading(shared(heading));
    }
    value
}

fn sample_groups() -> Vec<CommandGroup> {
    vec![
        group(
            "files",
            Some("Files"),
            vec![item("open", "Open file"), item("save", "Save file")],
        ),
        group("empty", Some("Unused"), Vec::new()),
        group(
            "edit",
            Some("Edit"),
            vec![item("copy", "Copy"), item("paste", "Paste")],
        ),
    ]
}

#[test]
fn caller_order_and_surviving_group_boundaries_are_preserved() {
    let groups = sample_groups();

    assert_eq!(
        visible_group_indices(&groups),
        vec![0, 2],
        "empty groups are the only groups omitted from the visual sequence"
    );
    assert_eq!(
        groups
            .iter()
            .flat_map(|group| group.items.iter().map(|item| item.id.as_str()))
            .collect::<Vec<_>>(),
        vec!["open", "save", "copy", "paste"],
        "the primitive must not rank or reorder caller-supplied rows"
    );
}

#[test]
fn empty_and_all_disabled_lists_have_no_resolved_selection_or_activation() {
    let empty = Vec::new();
    assert_eq!(resolved_highlight_id(&empty, None), None);
    assert_eq!(activation_for_id(&empty, None), None);

    let all_disabled = vec![group(
        "disabled",
        None,
        vec![
            item("one", "One").disabled(true),
            item("two", "Two").disabled(true),
        ],
    )];
    let requested = shared("one");

    assert_eq!(resolved_highlight_id(&all_disabled, Some(&requested)), None);
    assert_eq!(activation_for_id(&all_disabled, Some(&requested)), None);
    assert_eq!(
        navigation_target(&all_disabled, None, "down", Modifiers::none()),
        Some(None)
    );
    assert_eq!(
        navigation_target(&all_disabled, None, "up", Modifiers::none()),
        Some(None)
    );
    assert_eq!(
        navigation_target(&all_disabled, None, "home", Modifiers::none()),
        Some(None)
    );
    assert_eq!(
        navigation_target(&all_disabled, None, "end", Modifiers::none()),
        Some(None)
    );
}

#[test]
fn navigation_skips_disabled_rows_and_stops_at_edges() {
    let groups = vec![
        group(
            "first",
            None,
            vec![
                item("a", "A"),
                item("blocked", "Blocked").disabled(true),
                item("b", "B"),
            ],
        ),
        group(
            "second",
            None,
            vec![
                item("also-blocked", "Also blocked").disabled(true),
                item("c", "C"),
            ],
        ),
        group("third", None, vec![item("d", "D")]),
    ];
    let a = shared("a");
    let b = shared("b");
    let c = shared("c");
    let d = shared("d");

    assert_eq!(
        navigation_target(&groups, Some(&a), "down", Modifiers::none()),
        Some(Some(b.clone()))
    );
    assert_eq!(
        navigation_target(&groups, Some(&b), "down", Modifiers::none()),
        Some(Some(c.clone()))
    );
    assert_eq!(
        navigation_target(&groups, Some(&d), "down", Modifiers::none()),
        Some(None),
        "down stops at the final enabled row"
    );
    assert_eq!(
        navigation_target(&groups, Some(&a), "up", Modifiers::none()),
        Some(None),
        "up stops at the first enabled row"
    );
    assert_eq!(
        navigation_target(&groups, Some(&d), "home", Modifiers::none()),
        Some(Some(a.clone()))
    );
    assert_eq!(
        navigation_target(&groups, Some(&a), "end", Modifiers::none()),
        Some(Some(d.clone()))
    );
}

#[test]
fn meta_and_alt_arrows_use_stable_first_last_and_group_targets() {
    let groups = vec![
        group("first", None, vec![item("a", "A"), item("b", "B")]),
        group(
            "middle",
            None,
            vec![item("blocked", "Blocked").disabled(true), item("c", "C")],
        ),
        group("last", None, vec![item("d", "D"), item("e", "E")]),
    ];
    let a = shared("a");
    let b = shared("b");
    let c = shared("c");
    let d = shared("d");
    let e = shared("e");
    let meta = Modifiers {
        platform: true,
        ..Modifiers::none()
    };
    let alt = Modifiers {
        alt: true,
        ..Modifiers::none()
    };

    assert_eq!(
        navigation_target(&groups, Some(&c), "arrowup", meta),
        Some(Some(a.clone()))
    );
    assert_eq!(
        navigation_target(&groups, Some(&c), "arrowdown", meta),
        Some(Some(e.clone()))
    );
    assert_eq!(
        navigation_target(&groups, Some(&a), "arrowdown", alt),
        Some(Some(c.clone()))
    );
    assert_eq!(
        navigation_target(&groups, Some(&c), "arrowdown", alt),
        Some(Some(d.clone()))
    );
    assert_eq!(
        navigation_target(&groups, Some(&d), "arrowup", alt),
        Some(Some(c.clone()))
    );
    assert_eq!(
        navigation_target(&groups, Some(&c), "arrowup", alt),
        Some(Some(b.clone()))
    );
    assert_eq!(adjacent_group_id(&groups, Some(&a), true), Some(c));
}

#[test]
fn highlight_retains_identity_across_reordered_snapshots_and_falls_back_safely() {
    let requested = shared("keep-me");
    let initial = vec![group(
        "first",
        None,
        vec![item("other", "Other"), item("keep-me", "Keep me")],
    )];
    let reordered = vec![group(
        "first",
        None,
        vec![item("keep-me", "Keep me"), item("other", "Other")],
    )];
    let removed = vec![group(
        "first",
        None,
        vec![item("new-first", "New first"), item("other", "Other")],
    )];

    assert_eq!(
        resolved_highlight_id(&initial, Some(&requested)),
        Some(requested.clone())
    );
    assert_eq!(
        resolved_highlight_id(&reordered, Some(&requested)),
        Some(requested)
    );
    assert_eq!(
        resolved_highlight_id(&removed, Some(&shared("keep-me"))),
        Some(shared("new-first"))
    );
}

#[test]
fn activation_reports_group_item_and_label_and_rejects_disabled_or_unknown_ids() {
    let groups = vec![group(
        "files",
        None,
        vec![
            item("open", "Open file"),
            item("save", "Save file").disabled(true),
        ],
    )];

    assert_eq!(
        activation_for_id(&groups, Some(&shared("open"))),
        Some(CommandActivation::new("files", "open", "Open file"))
    );
    assert_eq!(activation_for_id(&groups, Some(&shared("save"))), None);
    assert_eq!(activation_for_id(&groups, Some(&shared("missing"))), None);
}

#[test]
fn style_resolves_required_geometry_colors_and_disabled_opacity_in_both_modes() {
    for (mode, input_surface) in [
        (ThemeMode::Light, SurfaceStep::S100),
        (ThemeMode::Dark, SurfaceStep::S900),
    ] {
        let theme = ArtisanTheme::for_mode(mode);
        let style = CommandStyle::resolve(theme);

        assert_eq!(style.outer_padding, theme.spacing.steps(1.0));
        assert_eq!(style.outer_radius, RadiusTokens::value(RadiusStep::X4l));
        assert_eq!(style.background, theme.colors.popover.to_paint());
        assert_eq!(style.foreground, theme.colors.popover_foreground.to_paint());
        assert_eq!(
            style.input_background,
            theme.surfaces.value(input_surface).to_paint()
        );
        assert_eq!(style.input_border, theme.colors.input.to_paint());
        assert_eq!(style.input_height, theme.density.control_default);
        assert_eq!(style.list_max_height, px(288.0));
        assert_eq!(style.list_max_height, theme.density.command_list_max_height);
        assert_eq!(style.list_scroll_padding, theme.spacing.steps(1.0));
        assert_eq!(style.item_horizontal_padding, theme.spacing.steps(2.0));
        assert_eq!(style.item_vertical_padding, theme.spacing.steps(1.5));
        assert_eq!(
            style.item_corner_radius,
            RadiusTokens::value(RadiusStep::Sm)
        );
        assert_eq!(style.item_text_size, theme.typography.control_text);
        assert_eq!(style.heading_text_size, theme.typography.label_text);
        assert_eq!(style.highlight_background, theme.colors.muted.to_paint());
        assert_eq!(
            style.highlight_foreground,
            theme.colors.foreground.to_paint()
        );
        assert_eq!(style.disabled_opacity.to_bits(), 0.5_f32.to_bits());
    }
}

struct PaletteProbe {
    focus: FocusHandle,
    query: SharedString,
    groups: Vec<CommandGroup>,
    highlighted: Option<SharedString>,
    theme: ArtisanTheme,
    highlights: Rc<RefCell<Vec<Option<SharedString>>>>,
    activations: Rc<RefCell<Vec<CommandActivation>>>,
}

impl PaletteProbe {
    fn new(
        cx: &mut Context<Self>,
        query: impl Into<SharedString>,
        groups: Vec<CommandGroup>,
        highlighted: Option<SharedString>,
        highlights: Rc<RefCell<Vec<Option<SharedString>>>>,
        activations: Rc<RefCell<Vec<CommandActivation>>>,
    ) -> Self {
        Self {
            focus: cx.focus_handle(),
            query: query.into(),
            groups,
            highlighted,
            theme: ArtisanTheme::for_mode(ThemeMode::Light),
            highlights,
            activations,
        }
    }
}

impl Render for PaletteProbe {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let highlights = self.highlights.clone();
        let activations = self.activations.clone();
        let palette = CommandPalette::new(
            "command-palette",
            self.focus.clone(),
            self.theme,
            self.query.clone(),
            self.groups.clone(),
        )
        .highlighted_id(self.highlighted.clone())
        .placeholder("Search commands")
        .focus_visibility(FocusVisibility::Visible)
        .debug_selector(ROOT_SELECTOR)
        .on_highlight_change(move |id, _, _| highlights.borrow_mut().push(id))
        .on_activate(move |activation, _, _| activations.borrow_mut().push(activation));

        div().w(px(420.0)).child(palette)
    }
}

#[gpui::test]
fn controlled_accessors_keep_query_groups_identity_and_semantic_metadata(cx: &mut TestAppContext) {
    let groups = sample_groups();
    let (view, cx) = cx.add_window_view(move |_, cx| {
        PaletteProbe::new(
            cx,
            "does-not-match",
            groups,
            Some(shared("copy")),
            Rc::new(RefCell::new(Vec::new())),
            Rc::new(RefCell::new(Vec::new())),
        )
    });

    cx.update(|_, app| {
        let probe = view.read(app);
        let palette = CommandPalette::new(
            "accessors",
            probe.focus.clone(),
            probe.theme,
            probe.query.clone(),
            probe.groups.clone(),
        )
        .highlighted_id(Some(shared("copy")))
        .focus_visibility(FocusVisibility::Visible);

        assert_eq!(palette.query(), "does-not-match");
        assert_eq!(palette.query_value(), &shared("does-not-match"));
        assert_eq!(palette.groups(), probe.groups.as_slice());
        assert_eq!(palette.requested_highlight(), Some(&shared("copy")));
        assert_eq!(palette.resolved_highlight(), Some(shared("copy")));
        assert_eq!(
            palette.activation(),
            Some(CommandActivation::new("edit", "copy", "Copy"))
        );
        assert_eq!(palette.focus_handle(), &probe.focus);
        assert_eq!(palette.visual_style(), CommandStyle::resolve(probe.theme));
        assert_eq!(palette.semantics().role, COMMAND_PALETTE_ROLE);
        assert_eq!(palette.semantics().item_count, 4);
        assert_eq!(palette.semantics().highlighted_id, Some(shared("copy")));
        assert_eq!(
            palette.semantics().focus_visibility,
            FocusVisibility::Visible
        );
    });
}

#[gpui::test]
fn stable_selectors_cover_root_input_list_and_empty_branches_without_ids(cx: &mut TestAppContext) {
    let groups = vec![
        group("secret-group", None, vec![item("secret-item", "Visible")]),
        group("empty-group", None, Vec::new()),
    ];
    let (_view, cx) = cx.add_window_view(move |_, cx| {
        PaletteProbe::new(
            cx,
            "",
            groups,
            None,
            Rc::new(RefCell::new(Vec::new())),
            Rc::new(RefCell::new(Vec::new())),
        )
    });

    assert!(cx.debug_bounds(ROOT_SELECTOR).is_some());
    assert!(cx.debug_bounds(INPUT_SELECTOR).is_some());
    assert!(cx.debug_bounds(LIST_SELECTOR).is_some());
    assert!(cx.debug_bounds(EMPTY_SELECTOR).is_none());
    assert!(
        cx.debug_bounds("command-palette-under-test-secret-group-secret-item")
            .is_none(),
        "IDs belong to element identity, never to diagnostic selectors"
    );
    assert_eq!(DEFAULT_DEBUG_SELECTOR, "artisan-command-palette");
    assert_eq!(DEFAULT_EMPTY_LABEL, "No results found.");
}

#[gpui::test]
fn empty_render_uses_empty_selector(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(move |_, cx| {
        PaletteProbe::new(
            cx,
            "",
            vec![group("nothing", None, Vec::new())],
            None,
            Rc::new(RefCell::new(Vec::new())),
            Rc::new(RefCell::new(Vec::new())),
        )
    });

    assert!(cx.debug_bounds(ROOT_SELECTOR).is_some());
    assert!(cx.debug_bounds(INPUT_SELECTOR).is_some());
    assert!(cx.debug_bounds(EMPTY_SELECTOR).is_some());
    assert!(cx.debug_bounds(LIST_SELECTOR).is_none());
}

#[gpui::test]
fn all_disabled_rows_remain_visible_but_are_not_a_selection(cx: &mut TestAppContext) {
    let (_view, cx) = cx.add_window_view(move |_, cx| {
        PaletteProbe::new(
            cx,
            "",
            vec![group(
                "disabled",
                None,
                vec![item("disabled", "Disabled").disabled(true)],
            )],
            Some(shared("disabled")),
            Rc::new(RefCell::new(Vec::new())),
            Rc::new(RefCell::new(Vec::new())),
        )
    });

    assert!(cx.debug_bounds(EMPTY_SELECTOR).is_none());
    assert!(cx.debug_bounds(LIST_SELECTOR).is_some());
}

#[gpui::test]
fn supplied_focus_keyboard_and_pointer_callbacks_activate_once(cx: &mut TestAppContext) {
    let highlights = Rc::new(RefCell::new(Vec::new()));
    let activations = Rc::new(RefCell::new(Vec::new()));
    let groups = vec![group(
        "files",
        None,
        vec![item("open", "Open"), item("save", "Save")],
    )];
    let highlights_for_view = highlights.clone();
    let activations_for_view = activations.clone();
    let (view, cx) = cx.add_window_view(move |_, cx| {
        PaletteProbe::new(
            cx,
            "",
            groups,
            Some(shared("open")),
            highlights_for_view,
            activations_for_view,
        )
    });

    cx.update(|window, app| {
        let focus = view.read(app).focus.clone();
        window.focus(&focus);
    });
    cx.run_until_parked();

    cx.simulate_keystrokes("down");
    assert_eq!(highlights.borrow().as_slice(), &[Some(shared("save"))]);

    cx.simulate_keystrokes("enter space");
    assert_eq!(
        activations.borrow().as_slice(),
        &[
            CommandActivation::new("files", "save", "Save"),
            CommandActivation::new("files", "save", "Save"),
        ]
    );

    let list = cx
        .debug_bounds(LIST_SELECTOR)
        .expect("a non-empty palette must expose its list bounds");
    let second_row = point(
        list.origin.x + list.size.width / 2.0,
        list.origin.y + list.size.height - px(16.0),
    );
    cx.simulate_click(second_row, Modifiers::none());

    assert_eq!(
        activations.borrow().as_slice(),
        &[
            CommandActivation::new("files", "save", "Save"),
            CommandActivation::new("files", "save", "Save"),
            CommandActivation::new("files", "save", "Save"),
        ],
        "one pointer click must produce one activation"
    );
}

#[gpui::test]
fn finite_many_row_rendering_is_capped_by_the_theme_list_height(cx: &mut TestAppContext) {
    let many = (0..128)
        .map(|index| item(&format!("item-{index}"), &format!("Item {index}")))
        .collect();
    let (_view, cx) = cx.add_window_view(move |_, cx| {
        PaletteProbe::new(
            cx,
            "",
            vec![group("many", Some("Many"), many)],
            None,
            Rc::new(RefCell::new(Vec::new())),
            Rc::new(RefCell::new(Vec::new())),
        )
    });

    let list = cx
        .debug_bounds(LIST_SELECTOR)
        .expect("many rows must still produce a list");
    assert!(list.size.height <= px(288.0));
    assert_eq!(
        list.size.height.min(px(288.0)),
        list.size.height,
        "the scroll viewport must honor max-h-72 / 288px"
    );
}
