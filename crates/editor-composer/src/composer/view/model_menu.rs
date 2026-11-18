use super::{ComposerAction, Dispatch, MENU_VIEWPORT_MARGIN, ModelMenuFrame, ModelMenuItem};
use aqua_core::Role;
use aqua_ui::{
    element::{Box as UiBox, Element, Scroll},
    style::{Length, Style},
};
use editor_assets::AssetId;
use editor_ui::{
    kit::{
        Density, Divider, Finish, HitArea, IconButton, IconButtonSize, IconSize, Inset, Link,
        LinkTone, Picker, Radius, Row, RowItem, Space, Spaced, Surface, TextRole,
    },
    theme::{ArtisanTheme, Opacity},
};
use std::rc::Rc;

pub(super) const MODEL_MENU_WIDTH: f32 = 528.0;

pub(super) const MODEL_PANE_HEIGHT: f32 = 328.0;

pub(super) const MODEL_SIDE_WIDTH: f32 = 164.0;

pub(super) const MODEL_ITEM_HEIGHT: f32 = match Row::ListItem(Density::Tall).height() {
    Some(height) => height,
    None => 0.0,
};

pub(super) const MODEL_ITEM_GUTTER: f32 = Inset::Menu.px();

pub(super) const MODEL_ITEM_GAP: f32 = 1.0;

pub(super) const MODEL_PANE_INSET: f32 = Inset::Menu.px();

pub(super) const MODEL_PANE_PADDING: Space = super::super::model_list::PADDING;

pub(super) const MODEL_ENGINE_TAB: IconButtonSize = IconButtonSize::Md;

pub(super) const MODEL_BAR_HEIGHT: f32 = MODEL_ENGINE_TAB.edge();

pub(super) const MODEL_BAR_GAP: Space = Space::S4;

pub(super) const MODEL_LIST_HEIGHT: f32 =
    MODEL_PANE_HEIGHT - MODEL_BAR_HEIGHT - 2.0 * MODEL_PANE_INSET;

pub(super) const MODEL_EDGE_TOP: &str = "composer-model-edge-top";

pub(super) const MODEL_EDGE_BOTTOM: &str = "composer-model-edge-bottom";

pub(super) fn model_menu_items(
    host: &editor_core::HostData,
    picker: &super::super::picker::PickerState,
    current: Option<artisan_api::EngineConfig>,
) -> Vec<ModelMenuItem> {
    let config = picker.pane_config(host, current);
    let model = config.as_ref().and_then(|config| {
        host.engines
            .get(&config.engine)?
            .models
            .iter()
            .find(|model| config.model.as_ref() == Some(&model.id))
    });
    let mut items = vec![ModelMenuItem {
        pane: None,
        label: "Model",
        value: model.map_or_else(
            || {
                config
                    .as_ref()
                    .and_then(|config| config.model.clone())
                    .unwrap_or_default()
            },
            |model| model.name.clone(),
        ),
    }];
    if let Some(config) = &config {
        for axis in super::super::picker::Axis::PICKER {
            let options = picker.axis_options(axis, config, model);
            if !axis.shown(options.len()) {
                continue;
            }
            let value = options
                .iter()
                .find(|option| option.selected)
                .map(|option| option.label.clone())
                .unwrap_or_else(|| match axis {
                    super::super::picker::Axis::Special => "None".into(),
                    super::super::picker::Axis::Activation => "On".into(),
                    super::super::picker::Axis::Thinking => "Special mode".into(),
                    _ => "Not set".into(),
                });
            items.push(ModelMenuItem {
                pane: Some(axis),
                label: axis.title(),
                value,
            });
        }
    }
    items
}

#[expect(
    clippy::disallowed_types,
    reason = "the sliding pane tab is a path-traced silhouette painted on a canvas, like TabCard; its fill and rim are path paints, not box styles"
)]
pub(super) fn model_menu_tab(
    theme: &ArtisanTheme,
    tab: super::super::sliding_tab::SlidingTab,
    radius: f32,
    header: f32,
) -> Element {
    use aqua_core::{
        DisplayItem, GpuiBackground, Transform2D,
        vector::{FillRule, GpuiPath, GpuiPathStyle, LineCap, LineJoin},
    };
    use aqua_ui::element::Canvas;
    let fill = GpuiBackground::Solid(theme.colors.background.to_reference_paint());
    let rim = GpuiBackground::Solid(theme.colors.text.at(Opacity::Hairline).to_reference_paint());
    Canvas::new()
        .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
        .on_paint(move |bounds| {
            let shape = super::super::sliding_tab::Bounds {
                width: bounds.w,
                height: bounds.h,
                split: MODEL_SIDE_WIDTH,
                radius,
            };
            let item = |commands, style, background| DisplayItem::GpuiPath {
                transformed_window_logical_bounds: bounds,
                path: std::sync::Arc::new(GpuiPath {
                    commands_local: commands,
                    origin_window_logical: bounds.origin(),
                    element_transform: Transform2D::IDENTITY,
                    style,
                    background,
                }),
            };
            vec![
                item(
                    tab.cutout(shape, header, MODEL_PANE_INSET),
                    GpuiPathStyle::Fill {
                        rule: FillRule::EvenOdd,
                    },
                    fill,
                ),
                item(
                    tab.outline(shape),
                    GpuiPathStyle::Stroke {
                        width: 1.0,
                        cap: LineCap::Butt,
                        join: LineJoin::Round,
                        dash: Vec::new(),
                        dash_offset: 0.0,
                    },
                    rim,
                ),
            ]
        })
        .build()
        .exclude_from_hit_testing()
}

pub(super) fn model_engine_bar(
    theme: ArtisanTheme,
    engines: &[&artisan_api::EngineStatus],
    spied: Option<artisan_api::EngineId>,
    engine_light: Option<Element>,
    tooltip_target: (editor_core::HostInstance, u64),
    dispatch: &Dispatch,
) -> Element {
    let mut bar = UiBox::new()
        .height(MODEL_BAR_HEIGHT)
        .spaced(MODEL_BAR_GAP)
        .style(
            Style::new()
                .flex_shrink(0.0)
                .margin(Length::Px(MODEL_PANE_INSET)),
        );
    if let Some(engine_light) = engine_light {
        bar = bar.child(engine_light);
    }
    for status in engines {
        let engine = status.engine;
        let tooltip_action = Rc::clone(dispatch);
        let tooltip_anchor = format!("composer-engine-{engine}");
        let reason = engine_reason(status);
        let explain: Rc<dyn Fn(bool)> = Rc::new({
            let reason = reason.clone();
            move |shown| {
                if let Some(reason) = &reason {
                    tooltip_action(ComposerAction::TooltipHover {
                        instance: tooltip_target.0,
                        generation: tooltip_target.1,
                        anchor: tooltip_anchor.clone(),
                        content: Some(super::super::picker::TooltipContent {
                            anchor: tooltip_anchor.clone(),
                            advisory: None,
                            text: reason.clone(),
                            placement: super::super::picker::TooltipPlacement::Above,
                        }),
                        hovered: shown,
                    });
                }
            }
        });
        bar = bar.child(engine_tab(theme, engine, spied, reason, explain, dispatch));
    }
    bar.build()
}

pub(super) fn engine_reason(status: &artisan_api::EngineStatus) -> Option<String> {
    use super::super::picker::Gate;
    let name = status.engine.display_name();
    super::super::picker::gate(status).map(|gate| match gate {
        Gate::NotInstalled => format!("{name} isn't installed on this Forge."),
        Gate::Installing => format!("Installing {name}…"),
        Gate::InstallFailed => format!("Installing {name} failed."),
        Gate::SignIn => format!("Sign in to {name} to use its models."),
        Gate::SigningIn => format!("Signing in to {name}…"),
        Gate::Checking => format!("Checking {name}…"),
        Gate::NoModels => format!("{name} hasn't reported any models."),
        Gate::DiscoveringModels | Gate::DiscoveryFailed => {
            status.discovery.description(artisan_api::now())
        }
    })
}

pub(super) fn engine_tab(
    theme: ArtisanTheme,
    engine: artisan_api::EngineId,
    spied: Option<artisan_api::EngineId>,
    reason: Option<String>,
    explain: Rc<dyn Fn(bool)>,
    dispatch: &Dispatch,
) -> Element {
    let action = Rc::clone(dispatch);
    let focus_explain = Rc::clone(&explain);
    let tab = IconButton::new(
        format!("composer-engine-{engine}"),
        theme,
        engine_icon(&theme, engine),
        engine.display_name(),
        MODEL_ENGINE_TAB,
        Finish::Plain,
    )
    .nested(Radius::Panel.px(), MODEL_PANE_INSET)
    .selected(spied == Some(engine))
    .on_activate(move |_| action(ComposerAction::JumpEngine(engine)))
    .build()
    .aria_selected(spied == Some(engine))
    .on_hover_change(move |hovered| explain(hovered))
    .on_focus_change(move |focused| focus_explain(focused));
    match reason {
        Some(reason) => tab
            .aria_description(reason)
            .style(Style::new().opacity(Opacity::Unavailable.alpha())),
        None => tab,
    }
}

pub(super) fn model_gate_row(
    theme: &ArtisanTheme,
    engine: artisan_api::EngineId,
    gate: super::super::picker::Gate,
    dispatch: &Dispatch,
) -> Element {
    let mut body = UiBox::new()
        .layout("column")
        .height(super::super::model_list::GATE_ROW_HEIGHT)
        .padded(Space::S12)
        .spaced(Space::S4)
        .style(Style::new().flex_shrink(0.0))
        .child(
            TextRole::Label
                .text(
                    theme,
                    format!("{} is not ready: {}.", engine.display_name(), gate.reason()),
                )
                .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
                .wrap(true)
                .build(),
        );
    if let Some(action) = gate.action() {
        let name = engine.display_name();
        let action_label = match action {
            super::super::picker::GateAction::SignIn => format!("Sign in to {name}"),
            super::super::picker::GateAction::Install => format!("Install {name}"),
            super::super::picker::GateAction::OpenSettings => format!("Open {name} settings"),
        };
        let action = Rc::clone(dispatch);
        body = body.child(
            Link::new(
                format!("composer-engine-gate-{engine}"),
                *theme,
                LinkTone::Link,
                action_label,
            )
            .on_activate(move || action(ComposerAction::OpenEngineSettings(engine)))
            .build(),
        );
    }
    body.build()
}

pub(super) fn model_list_pane(
    theme: ArtisanTheme,
    host: &editor_core::HostData,
    picker: &super::super::picker::PickerState,
    current: Option<&artisan_api::EngineConfig>,
    scroll: f32,
    dispatch: &Dispatch,
) -> Element {
    use super::super::model_list::{GROUP_ROW_HEIGHT, ROW_GAP, SECTION_SPACER};
    use super::super::picker::ModelRow;
    let muted = theme.colors.text_muted;
    let mut rows = UiBox::new()
        .layout("column")
        .padded(MODEL_PANE_PADDING)
        .spaced(ROW_GAP)
        .style(Style::new().flex_shrink(0.0));
    let mut section = None;
    for entry in &picker.models {
        let engine = entry.engine();
        if section.is_some_and(|previous| previous != engine) {
            rows = rows.child(
                UiBox::new()
                    .height(SECTION_SPACER)
                    .style(Style::new().flex_shrink(0.0))
                    .build(),
            );
        }
        section = Some(engine);
        match entry {
            ModelRow::Engine(_) => {
                rows = rows.child(
                    RowItem::new(
                        format!("composer-model-engine-{engine}"),
                        theme,
                        Row::Heading,
                        Surface::Menu,
                        engine.display_name(),
                    )
                    .build(),
                );
            }
            ModelRow::Gate(_) => {
                if let Some(gate) = host
                    .engines
                    .get(&engine)
                    .and_then(super::super::picker::gate)
                {
                    rows = rows.child(model_gate_row(&theme, engine, gate, dispatch));
                }
            }
            ModelRow::Group { name, open, .. } => {
                let name = name.clone();
                let is_open = *open;
                let group_label = if name.is_empty() {
                    "Other".to_owned()
                } else {
                    name.clone()
                };
                let action = Rc::clone(dispatch);
                let heading = RowItem::new(
                    format!("composer-model-group-{engine}-{group_label}"),
                    theme,
                    Row::Heading,
                    Surface::Menu,
                    group_label.clone(),
                )
                .trailing(IconSize::Sm.glyph(
                    if is_open {
                        AssetId::TABLER_CHEVRON_DOWN
                    } else {
                        AssetId::TABLER_CHEVRON_RIGHT
                    },
                    muted.to_reference_paint(),
                ))
                .build();
                rows = rows.child(
                    HitArea::new(
                        format!("composer-model-group-toggle-{engine}-{group_label}"),
                        &theme,
                        Role::Button,
                        group_label,
                    )
                    .child(heading)
                    .on_activate(move || {
                        action(ComposerAction::ToggleModelGroup {
                            engine,
                            name: name.clone(),
                            open: is_open,
                        })
                    })
                    .build()
                    .style(
                        Style::new()
                            .height(Length::Px(GROUP_ROW_HEIGHT))
                            .flex_shrink(0.0),
                    )
                    .aria_expanded(is_open),
                );
            }
            ModelRow::Model { id, index, .. } => {
                let Some(model) = host
                    .engines
                    .get(&engine)
                    .and_then(|status| status.models.get(*index))
                    .filter(|model| &model.id == id)
                else {
                    continue;
                };
                let chosen = current.is_some_and(|config| {
                    config.engine == engine && config.model.as_ref() == Some(id)
                });
                let model_id = id.clone();
                let action = Rc::clone(dispatch);
                let mut row = RowItem::new(
                    format!("composer-model-{engine}-{id}"),
                    theme,
                    Row::MenuItem,
                    Surface::Menu,
                    model.name.clone(),
                )
                .nested(Radius::Panel.px(), MODEL_PANE_INSET)
                .leading(
                    IconSize::Md.glyph(
                        model
                            .lab
                            .as_deref()
                            .and_then(lab_icon)
                            .unwrap_or_else(|| engine_icon(&theme, engine)),
                        theme.colors.text.to_reference_paint(),
                    ),
                )
                .on_activate(move || {
                    action(ComposerAction::ChooseModel {
                        engine,
                        id: model_id.clone(),
                    })
                });
                if chosen {
                    row = row.trailing(
                        IconSize::Md.glyph(AssetId::TABLER_CHECK, muted.to_reference_paint()),
                    );
                }
                rows = rows.child(row.build().aria_selected(chosen));
            }
        }
    }
    UiBox::new()
        .layout("column")
        .height(MODEL_LIST_HEIGHT)
        .clip(true)
        .style(Style::new().flex_shrink(0.0))
        .child(
            Scroll::new()
                .height(MODEL_LIST_HEIGHT)
                .offset(aqua_core::Point::new(0.0, scroll))
                .style(Style::new().column())
                .child(rows.build())
                .build()
                .with_key(editor_ui::scroll::MODEL_MENU),
        )
        .child(picker.model_edges.inset_band(
            MODEL_EDGE_TOP,
            editor_ui::scroll_edge::Edge::Top,
            theme.colors.background,
            0.0,
        ))
        .child(picker.model_edges.inset_band(
            MODEL_EDGE_BOTTOM,
            editor_ui::scroll_edge::Edge::Bottom,
            theme.colors.background,
            Radius::Panel.px(),
        ))
        .build()
}

pub(super) fn model_axis_pane(
    theme: ArtisanTheme,
    axis: super::super::picker::Axis,
    options: &[super::super::picker::AxisOption],
    keyboard_cursor: Option<usize>,
    dispatch: &Dispatch,
) -> Element {
    let mut rows = UiBox::new()
        .layout("column")
        .padded(MODEL_PANE_PADDING)
        .spaced(Space::S2)
        .style(Style::new().flex_shrink(0.0));
    let mut special = false;
    for (index, option) in options.iter().enumerate() {
        if option.special && !special && axis != super::super::picker::Axis::Special {
            special = true;
            rows = rows
                .child(Divider::Rule.build(&theme, MODEL_PANE_INSET))
                .child(
                    RowItem::new(
                        format!("composer-axis-{axis:?}-special"),
                        theme,
                        Row::Heading,
                        Surface::Menu,
                        "Special efforts",
                    )
                    .build(),
                );
        }
        let advisory = option.advisory.as_deref().filter(|advisory| {
            !option
                .description
                .as_deref()
                .is_some_and(|description| description.contains(advisory))
        });
        let detail = [option.description.as_deref(), advisory]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
        let detail = Some(detail).filter(|detail| {
            !detail.is_empty()
                && !matches!(
                    axis,
                    super::super::picker::Axis::Speed
                        | super::super::picker::Axis::Thinking
                        | super::super::picker::Axis::Permission
                )
        });
        let action = Rc::clone(dispatch);
        let pointer_action = Rc::clone(dispatch);
        let shape = match detail {
            Some(_) => Row::ListItem(Density::Tall),
            None => Row::MenuItem,
        };
        let mut row = RowItem::new(
            format!("composer-axis-{axis:?}-{index}"),
            theme,
            shape,
            Surface::Menu,
            option.label.clone(),
        )
        .role(Role::MenuItem)
        .nested(Radius::Panel.px(), MODEL_PANE_INSET)
        .selected(keyboard_cursor == Some(index))
        .on_activate(move || action(ComposerAction::ChooseAxis { axis, index }))
        .on_hover(move |hovered| {
            if hovered {
                pointer_action(ComposerAction::AxisPointer { axis, index });
            }
        });
        if let Some(detail) = detail {
            row = row.detail(detail);
        }
        if option.selected {
            row = row.trailing(IconSize::Md.glyph(
                AssetId::TABLER_CHECK,
                theme.colors.text_muted.to_reference_paint(),
            ));
        }
        rows = rows.child(row.build().aria_selected(option.selected));
    }
    Scroll::new()
        .height(MODEL_PANE_HEIGHT)
        .style(Style::new().column())
        .child(rows.build())
        .build()
        .with_key(format!("composer-axis-pane-{axis:?}"))
}

#[expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "the model rail's 1 px item gap and the sheet's top-trimmed inset have no kit token"
)]
pub fn model_menu(
    theme: ArtisanTheme,
    host: &editor_core::HostData,
    picker: &super::super::picker::PickerState,
    current: Option<artisan_api::EngineConfig>,
    viewport: aqua_core::Size,
    frame: ModelMenuFrame,
    tooltip_target: (editor_core::HostInstance, u64),
    dispatch: Dispatch,
) -> Element {
    let ModelMenuFrame {
        items,
        pane,
        scroll,
        spied,
        tab,
        engine_light,
    } = frame;
    let sheet = Surface::Picker(Picker::Sheet);
    let pane_radius = Radius::Panel.px();
    let engines = super::super::picker::offered_engines(host);
    let model_icon = picker.pane_config(host, current.clone()).map(|config| {
        host.engines
            .get(&config.engine)
            .and_then(|status| {
                status
                    .models
                    .iter()
                    .find(|model| config.model.as_ref() == Some(&model.id))
            })
            .and_then(|model| model.lab.as_deref())
            .and_then(lab_icon)
            .unwrap_or_else(|| engine_icon(&theme, config.engine))
    });
    let item_height = model_item_height(items.len());
    let mut buttons = UiBox::new()
        .layout("column")
        .width(MODEL_SIDE_WIDTH - MODEL_ITEM_GUTTER)
        .gap(MODEL_ITEM_GAP)
        .style(Style::new().flex_shrink(0.0));
    for (position, item) in items.iter().enumerate() {
        let target = item.pane;
        let action = Rc::clone(&dispatch);
        let mut side = RowItem::new(
            format!("composer-pane-{position}"),
            theme,
            Row::ListItem(Density::Tall),
            sheet,
            item.value.clone(),
        )
        .detail(item.label)
        .role(Role::Tab)
        .nested(pane_radius, 0.0)
        .hoverable(target != pane)
        .on_activate(move || action(ComposerAction::SelectPane(target)));
        if let Some(icon) = model_icon.filter(|_| target.is_none()) {
            side = side.leading(IconSize::Md.glyph(icon, theme.colors.text.to_reference_paint()));
        }
        buttons = buttons.child(
            side.build()
                .style(
                    Style::new()
                        .height(Length::Px(item_height))
                        .flex_shrink(0.0),
                )
                .aria_label(item.name())
                .aria_selected(target == pane),
        );
    }
    let mut pane_body = UiBox::new()
        .layout("column")
        .height(MODEL_PANE_HEIGHT)
        .clip(true)
        .style(Radius::Panel.style())
        .style(Style::new().flex_grow(1.0).min_width(Length::Pct(0.0)));
    match pane {
        None => {
            pane_body = pane_body
                .child(model_engine_bar(
                    theme,
                    &engines,
                    spied,
                    engine_light,
                    tooltip_target,
                    &dispatch,
                ))
                .child(model_list_pane(
                    theme,
                    host,
                    picker,
                    current.as_ref(),
                    scroll,
                    &dispatch,
                ));
        }
        Some(axis) => {
            let config = picker.pane_config(host, current.clone());
            let model = config.as_ref().and_then(|config| {
                host.engines
                    .get(&config.engine)?
                    .models
                    .iter()
                    .find(|model| config.model.as_ref() == Some(&model.id))
            });
            let options = config
                .as_ref()
                .map_or_else(Vec::new, |config| picker.axis_options(axis, config, model));
            pane_body = pane_body.child(model_axis_pane(
                theme,
                axis,
                &options,
                picker.keyboard_axis_cursor(),
                &dispatch,
            ));
        }
    }
    let key_dispatch = Rc::clone(&dispatch);
    let resolution_note = current.as_ref().and_then(|config| {
        let model = host
            .engines
            .get(&config.engine)?
            .models
            .iter()
            .find(|model| config.model.as_ref() == Some(&model.id))?;
        let changes = model.resolve_config(config, &picker.customizations).changes;
        (!changes.is_empty()).then(|| {
            changes
                .into_iter()
                .map(|change| format!("{}: {}", change.axis.title(), change.reason))
                .collect::<Vec<_>>()
                .join(" ")
        })
    });
    let mut children = vec![
        UiBox::new()
            .height(MODEL_PANE_HEIGHT)
            .child(model_menu_tab(
                &theme,
                tab,
                pane_radius,
                if pane.is_none() {
                    MODEL_BAR_HEIGHT + MODEL_PANE_INSET
                } else {
                    0.0
                },
            ))
            .child(buttons.build())
            .child(
                UiBox::new()
                    .width(MODEL_ITEM_GUTTER)
                    .style(Style::new().flex_shrink(0.0))
                    .build(),
            )
            .child(pane_body.build())
            .build(),
    ];
    if let Some(note) = resolution_note {
        children.push(
            UiBox::new()
                .layout("column")
                .padded(MODEL_PANE_PADDING)
                .style(Style::new().min_width(Length::Pct(0.0)))
                .child(TextRole::Caption.text(&theme, note).wrap(true).build())
                .build(),
        );
    }
    sheet
        .build(&theme, children)
        .style(
            Style::new()
                .width(Length::Px(
                    MODEL_MENU_WIDTH.min(viewport.w - MENU_VIEWPORT_MARGIN),
                ))
                .padding_insets(aqua_core::Insets {
                    left: MODEL_ITEM_GUTTER,
                    ..aqua_core::Insets::uniform(sheet.padding_px())
                }),
        )
        .with_key("composer-model-menu")
        .on_input(
            aqua_ui::input::InputKind::KeyDown,
            aqua_ui::input::EventPhase::Bubble,
            move |event| {
                use aqua_core::{InputEvent, Key, NamedKey};
                use aqua_ui::input::{InputResponse, RoutedInput};
                let Some(axis) = pane else {
                    return InputResponse::default();
                };
                let key = match &event.input {
                    RoutedInput::Native(InputEvent::KeyDown(event)) => match event.key {
                        Key::Named(NamedKey::Home) => Some(super::super::picker::AxisKey::Home),
                        Key::Named(NamedKey::End) => Some(super::super::picker::AxisKey::End),
                        Key::Named(NamedKey::Up) => Some(super::super::picker::AxisKey::Up),
                        Key::Named(NamedKey::Down) => Some(super::super::picker::AxisKey::Down),
                        Key::Named(NamedKey::Enter) | Key::Char(' ') => {
                            Some(super::super::picker::AxisKey::Accept)
                        }
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(key) = key {
                    key_dispatch(ComposerAction::AxisKey { axis, key });
                    InputResponse {
                        stop_propagation: true,
                        ..InputResponse::handled()
                    }
                } else {
                    InputResponse::default()
                }
            },
        )
}

pub(super) fn model_item_height(count: usize) -> f32 {
    let count = count.max(1) as f32;
    MODEL_ITEM_HEIGHT.min((MODEL_PANE_HEIGHT - (count - 1.0) * MODEL_ITEM_GAP) / count)
}

pub(super) fn lab_icon(lab: &str) -> Option<AssetId> {
    Some(match lab {
        "Alibaba" => AssetId::SVGL_QWEN,
        "Anthropic" => AssetId::SVGL_CLAUDE_AI,
        "DeepSeek" => AssetId::SVGL_DEEPSEEK,
        "Google" => AssetId::LOBE_GOOGLE,
        "Meituan" => AssetId::SIMPLE_ICONS_MEITUAN,
        "Meta" => AssetId::SVGL_META,
        "MiniMax" => AssetId::LOBE_MINIMAX,
        "Mistral" => AssetId::LOBE_MISTRAL,
        "Moonshot AI" => AssetId::BRANDS_KIMI,
        "NVIDIA" => AssetId::LOBE_NVIDIA,
        "OpenAI" => AssetId::SVGL_OPENAI,
        "Reflection AI" => AssetId::BRANDS_REFLECTION,
        "StepFun" => AssetId::LOBE_STEPFUN,
        "Tencent" => AssetId::LOBE_TENCENT,
        "xAI" => AssetId::LOBE_XAI,
        "Xiaomi" => AssetId::SIMPLE_ICONS_XIAOMI,
        "Z.ai" => AssetId::BRANDS_ZAI,
        _ => return None,
    })
}

pub(super) fn engine_icon(theme: &ArtisanTheme, engine: artisan_api::EngineId) -> AssetId {
    match engine {
        artisan_api::EngineId::Claude => AssetId::SVGL_CLAUDE_AI,
        artisan_api::EngineId::Codex => AssetId::SVGL_OPENAI,
        artisan_api::EngineId::OpenCode if theme.colors.mode.is_dark() => AssetId::BRANDS_OPENCODE,
        artisan_api::EngineId::OpenCode => AssetId::BRANDS_OPENCODE_LIGHT,
    }
}

pub(super) fn axis_options(
    composer: &super::super::controller::Composer,
    host: &editor_core::HostData,
    axis: super::super::picker::Axis,
) -> Option<Vec<super::super::picker::AxisOption>> {
    let preview = composer
        .picker
        .pane_config(host, composer.effective_config(host))?;
    let model = host.engines.get(&preview.engine).and_then(|status| {
        preview
            .model
            .as_ref()
            .and_then(|id| status.models.iter().find(|model| &model.id == id))
    });
    Some(composer.picker.axis_options(axis, &preview, model))
}

pub(super) fn choose_axis_config(
    composer: &mut super::super::controller::Composer,
    core: &mut editor_core::Core,
    host_id: &editor_core::HostId,
    config: artisan_api::EngineConfig,
) {
    let host = core
        .host(host_id)
        .expect("invariant: the composer's host stays in the store while it dispatches");
    let instance = host.instance();
    composer.apply_permissions(host, config.permissions);
    composer.config = Some(config.clone());
    composer.edited();
    if let Some(host) = core.host_mut_if_instance(host_id, instance) {
        host.remember_model_config(config);
    }
}
