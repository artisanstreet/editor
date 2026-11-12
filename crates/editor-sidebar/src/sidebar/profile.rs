use std::rc::Rc;

use aqua_core::{CursorIcon, Insets, Role};
use aqua_text::{FontFamily, FontWeight, TextStyle};
use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Button, Element, Scroll, Text},
    style::{Length, Style},
};
use editor_assets::AssetId;

use super::{controller::Sidebar, rail, usage_view, view::ViewAction};
use editor_core::{Core, Screen, SettingsPage};
use editor_ui::{
    asset::source_asset_glyph,
    glass::{GlassStrength, glass_card_shadows, glass_layers},
    theme::{
        ArtisanTheme, Elevation, Opacity, RadiusStep, RadiusTokens, TextStep, TypographyTokens,
    },
};

fn label(
    theme: ArtisanTheme,
    text: impl Into<String>,
    size: f32,
    line_height: f32,
    color: aqua::GpuiTextColor,
    medium: bool,
) -> Element {
    Text::new(text.into())
        .text_style(TextStyle {
            families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
            size_px: size,
            line_height_px: line_height,
            weight: if medium {
                FontWeight::MEDIUM
            } else {
                FontWeight::NORMAL
            },
            ..TextStyle::default()
        })
        .gpui_source_color(color)
        .build()
}

fn action(
    theme: ArtisanTheme,
    id: &'static str,
    icon: AssetId,
    title: &'static str,
    emit: Rc<dyn Fn(ViewAction)>,
    screen: Screen,
    generation: u64,
    machines_generation: u64,
    submenu: bool,
) -> Element {
    let hover = emit.clone();
    Button::new()
        .width(Length::Pct(100.0))
        .gap(8.0)
        .radius(RadiusTokens::value(RadiusStep::LgMid))
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .padding_insets(Insets {
                    left: 12.0,
                    right: 12.0,
                    top: 8.0,
                    bottom: 8.0,
                }),
        )
        .child(
            UiBox::new()
                .width(16.0)
                .height(16.0)
                .style(Style::new().flex_shrink(0.0))
                .child(source_asset_glyph(
                    icon,
                    15.0,
                    theme.colors.text_muted.to_reference_paint(),
                ))
                .build(),
        )
        .child(label(
            theme,
            title,
            TypographyTokens::size(TextStep::Sm),
            20.0,
            theme.colors.text.to_reference_text_color(),
            false,
        ))
        .on_activate(move || emit(ViewAction::ProfileNavigate(screen.clone())))
        .build()
        .with_key(id)
        .role(Role::MenuItem)
        .aria_label(title)
        .cursor(CursorIcon::Pointer)
        .on_hover_change(move |hovered| {
            if submenu {
                hover(ViewAction::MachinesHover {
                    id: id.to_owned(),
                    hovered,
                });
            } else if hovered {
                hover(ViewAction::CloseProfileMachinesFor {
                    generation,
                    machines_generation,
                });
                hover(ViewAction::ProfileHover {
                    id: id.to_owned(),
                    hovered: true,
                });
            }
        })
}

pub fn content(
    sidebar: &Sidebar,
    core: &Core,
    theme: ArtisanTheme,
    usage_height: f32,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let host = sidebar.host_data(core);
    let profile = core.account().profile();
    let seed = profile
        .map(|p| p.account_id.clone())
        .or_else(|| {
            host.map(|host| {
                core.hosts()
                    .find(|(config, _)| config.id == host.id)
                    .map_or_else(
                        || host.id.0.clone(),
                        |(config, _)| config.fingerprint.clone(),
                    )
            })
        })
        .unwrap_or_else(|| "no-host".into());
    let name = profile
        .map(|p| {
            if p.display_name.is_empty() {
                p.email.clone()
            } else {
                p.display_name.clone()
            }
        })
        .or_else(|| host.map(|h| h.name.clone()))
        .unwrap_or_else(|| "Artisan".into());
    let status = host.map_or_else(
        || "No host".to_owned(),
        |host| host.status_line().unwrap_or(&host.name).to_owned(),
    );
    let reports: Vec<_> = host
        .into_iter()
        .flat_map(|host| host.engines.values())
        .filter_map(|status| {
            status
                .usage
                .as_ref()
                .filter(|usage| !usage.windows.is_empty())
                .map(|usage| (status.engine, usage.clone()))
        })
        .collect();
    let toggle = emit.clone();
    let hover = emit.clone();
    let host_row = Button::new()
        .width(Length::Pct(100.0))
        .padding(8.0)
        .gap(12.0)
        .radius(RadiusTokens::value(RadiusStep::LgMid))
        .style(Style::new().align(AlignItems::CENTER))
        .child(rail::avatar(&seed))
        .child(
            UiBox::new()
                .layout("column")
                .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                .child(label(
                    theme,
                    name,
                    TypographyTokens::size(TextStep::Sm),
                    20.0,
                    theme.colors.text.to_reference_text_color(),
                    true,
                ))
                .child(label(
                    theme,
                    status,
                    TypographyTokens::size(TextStep::Xs),
                    16.0,
                    theme.colors.text_muted.to_reference_text_color(),
                    false,
                ))
                .build(),
        )
        .child(source_asset_glyph(
            AssetId::TABLER_CHEVRON_RIGHT,
            15.0,
            theme.colors.text_muted.to_reference_paint(),
        ))
        .on_activate(move || toggle(ViewAction::ToggleProfileMachines))
        .build()
        .with_key("artisan-desktop-profile-machine")
        .role(Role::Button)
        .aria_label("Choose Forge")
        .aria_expanded(sidebar.profile_machines_open)
        .cursor(CursorIcon::Pointer)
        .on_hover_change(move |hovered| {
            if hovered {
                hover(ViewAction::OpenProfileMachines);
                hover(ViewAction::ProfileHover {
                    id: "artisan-desktop-profile-machine".to_owned(),
                    hovered: true,
                });
            }
        });
    let line = || {
        UiBox::new()
            .height(1.0)
            .style(
                Style::new().gpui_background(aqua_core::GpuiBackground::Solid(
                    theme.colors.text.at(Opacity::Hairline).to_reference_paint(),
                )),
            )
            .style(Style::new().margin_insets(Insets {
                top: 4.0,
                bottom: 4.0,
                left: 0.0,
                right: 0.0,
            }))
            .build()
    };
    let mut body = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .child(sidebar.profile_hover.pill(&theme, 12.0, true))
        .child(UiBox::new().padding(4.0).child(host_row).build())
        .child(line());
    if !reports.is_empty() {
        let callback = emit.clone();
        body = body
            .child(
                Scroll::new()
                    .style(Style::new().max_height(Length::Px(usage_height.max(0.0))))
                    .child(usage_view::profile_usage(
                        &reports,
                        theme,
                        Rc::new(move |engine, index, hovered| {
                            callback(ViewAction::ProfileUsageHover {
                                engine,
                                index,
                                hovered,
                            })
                        }),
                    ))
                    .build()
                    .with_key("artisan-profile-usage-scroll"),
            )
            .child(line());
    }
    let settings = action(
        theme,
        "artisan-desktop-profile-action-0",
        AssetId::TABLER_SETTINGS,
        "Settings",
        emit.clone(),
        Screen::Settings(SettingsPage::General),
        sidebar.profile_generation,
        sidebar.machines_generation,
        false,
    );
    let usage = action(
        theme,
        "artisan-desktop-profile-action-1",
        AssetId::TABLER_CHART_COLUMN,
        "Usage",
        emit.clone(),
        Screen::Usage,
        sidebar.profile_generation,
        sidebar.machines_generation,
        false,
    );
    let dark = theme.colors.mode.is_dark();
    let icon = if dark {
        AssetId::TABLER_MOON
    } else {
        AssetId::TABLER_SUN
    };
    let button_label = if dark {
        "Switch to light theme"
    } else {
        "Switch to dark theme"
    };
    let set_theme = emit.clone();
    let theme_button = Button::new()
        .width(28.0)
        .height(28.0)
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .justify(JustifyContent::CENTER),
        )
        .child(source_asset_glyph(
            icon,
            15.0,
            theme.colors.text_muted.to_reference_paint(),
        ))
        .on_activate(move || set_theme(ViewAction::SetDarkMode(!dark)))
        .build()
        .with_key("artisan-desktop-profile-theme")
        .role(Role::Button)
        .aria_label(button_label)
        .cursor(CursorIcon::Pointer);
    let theme_row = UiBox::new()
        .height(36.0)
        .width(Length::Pct(100.0))
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .justify(JustifyContent::SPACE_BETWEEN)
                .padding_insets(Insets {
                    left: 12.0,
                    right: 4.0,
                    top: 0.0,
                    bottom: 0.0,
                }),
        )
        .child(label(
            theme,
            "Theme",
            TypographyTokens::size(TextStep::Sm),
            20.0,
            theme.colors.text.to_reference_text_color(),
            false,
        ))
        .child(theme_button)
        .build();
    let actions = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .padding(4.0)
        .child(settings)
        .child(usage)
        .child(
            UiBox::new()
                .height(1.0)
                .style(
                    Style::new().gpui_background(aqua_core::GpuiBackground::Solid(
                        theme.colors.text.at(Opacity::Hairline).to_reference_paint(),
                    )),
                )
                .style(Style::new().margin_insets(Insets {
                    left: -4.0,
                    right: -4.0,
                    top: 8.0,
                    bottom: 4.0,
                }))
                .build(),
        )
        .child(theme_row)
        .build()
        .with_key("artisan-desktop-profile-actions-hover-surface");
    body = body.child(actions);
    let leave = emit.clone();
    let body = body.build().with_key("artisan-profile-content").on_input(
        aqua_ui::input::InputKind::PointerLeave,
        aqua_ui::input::EventPhase::Bubble,
        move |event| {
            if event.current_target == event.target {
                leave(ViewAction::ClearProfileHover);
            }
            aqua_ui::input::InputResponse::default()
        },
    );
    let radius = RadiusTokens::value(RadiusStep::X2l);
    UiBox::new()
        .width(256.0)
        .radius(radius)
        .style(
            Style::new().gpui_border(
                Insets::uniform(1.0),
                theme
                    .colors
                    .text
                    .at(Opacity::GlassBorder)
                    .to_reference_paint(),
                false,
            ),
        )
        .clip(true)
        .style(
            Style::new().shadows(
                glass_card_shadows(&theme, Elevation::Overlay)
                    .into_iter()
                    .map(Into::into)
                    .collect(),
            ),
        )
        .child(glass_layers(&theme, GlassStrength::Strong, radius))
        .child(body)
        .build()
        .with_key("artisan-desktop-profile-menu")
}

pub fn machines(
    sidebar: &Sidebar,
    core: &Core,
    theme: ArtisanTheme,
    viewport_width: f32,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let width = 300.0_f32.min((viewport_width - 16.0).max(0.0));
    let emit_leave = emit.clone();
    let current = sidebar.host_data(core).map(|host| host.id.clone());
    let mut rows = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .padding(4.0)
        .child(sidebar.machines_hover.pill(&theme, 12.0, true));
    for (config, host) in core.hosts() {
        let id = config.id.clone();
        let host_id = id.clone();
        let host_instance = host.instance();
        let name = config.name.clone();
        let secondary = host.status_line().unwrap_or(&config.address).to_owned();
        let active = current.as_ref() == Some(&id);
        let navigate = emit.clone();
        let hover = emit.clone();
        let hover_id = format!("machine-option-{id}");
        let mut row = Button::new()
            .width(Length::Pct(100.0))
            .padding(8.0)
            .gap(10.0)
            .radius(RadiusTokens::value(RadiusStep::LgMid))
            .style(Style::new().align(AlignItems::CENTER))
            .child(rail::avatar(&config.fingerprint))
            .child(
                UiBox::new()
                    .layout("column")
                    .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                    .child(label(
                        theme,
                        name.clone(),
                        TypographyTokens::size(TextStep::Sm),
                        20.0,
                        theme.colors.text.to_reference_text_color(),
                        false,
                    ))
                    .child(label(
                        theme,
                        secondary,
                        TypographyTokens::size(TextStep::Xs),
                        16.0,
                        theme.colors.text_muted.to_reference_text_color(),
                        false,
                    ))
                    .build(),
            );
        if active {
            row = row.child(source_asset_glyph(
                AssetId::TABLER_CHECK,
                15.0,
                theme.colors.text.to_reference_paint(),
            ));
        }
        rows = rows.child(
            row.on_activate(move || {
                navigate(ViewAction::ProfileOpenHost {
                    host: host_id.clone(),
                    instance: host_instance,
                })
            })
            .build()
            .with_key(format!("machine-option-{id}"))
            .role(Role::MenuItem)
            .aria_label(name)
            .aria_selected(active)
            .cursor(CursorIcon::Pointer)
            .on_hover_change(move |hovered| {
                hover(ViewAction::MachinesHover {
                    id: hover_id.clone(),
                    hovered,
                })
            }),
        );
    }
    rows = rows
        .child(editor_ui::kit::Divider::Rule.build(&theme, 4.0))
        .child(action(
            theme,
            "sidebar-manage-forges",
            AssetId::TABLER_SETTINGS,
            "Manage Forges…",
            emit,
            Screen::Settings(SettingsPage::Account),
            sidebar.profile_generation,
            sidebar.machines_generation,
            true,
        ));
    let leave = emit_leave;
    let rows = rows.build().with_key("machines-hover-surface").on_input(
        aqua_ui::input::InputKind::PointerLeave,
        aqua_ui::input::EventPhase::Bubble,
        move |event| {
            if event.current_target == event.target {
                leave(ViewAction::ClearMachinesHover);
            }
            aqua_ui::input::InputResponse::default()
        },
    );
    let radius = RadiusTokens::value(RadiusStep::X2l);
    UiBox::new()
        .width(width)
        .radius(radius)
        .style(
            Style::new().gpui_border(
                Insets::uniform(1.0),
                theme
                    .colors
                    .text
                    .at(Opacity::GlassBorder)
                    .to_reference_paint(),
                false,
            ),
        )
        .clip(true)
        .style(
            Style::new().shadows(
                glass_card_shadows(&theme, Elevation::Overlay)
                    .into_iter()
                    .map(Into::into)
                    .collect(),
            ),
        )
        .child(glass_layers(&theme, GlassStrength::Strong, radius))
        .child(
            Scroll::new()
                .style(Style::new().max_height(Length::Px(400.0)))
                .child(rows)
                .build(),
        )
        .build()
        .with_key("machine-dropdown")
}
