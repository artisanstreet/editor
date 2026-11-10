use artisan_api::{EngineStatus, InstallState, SignInState};
use editor_assets::AssetId;

use editor_core::{HostData, SettingsPage};

#[derive(Clone, Debug)]
pub struct Group {
    pub heading: String,
    pub entries: Vec<Entry>,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub page: SettingsPage,
    pub icon: Option<AssetId>,
    pub need: Option<&'static str>,
    pub expanded: bool,
    pub children: Vec<SettingsPage>,
}

pub fn engine_need(status: &EngineStatus) -> Option<&'static str> {
    match (&status.install, &status.sign_in) {
        (InstallState::Absent, _) => Some("Install"),
        (InstallState::Failed { .. }, _) => Some("Failed"),
        (InstallState::Downloading { .. } | InstallState::Verifying { .. }, _) => {
            Some("Installing")
        }
        (_, SignInState::SignedOut | SignInState::Failed { .. }) => Some("Sign in"),
        _ => None,
    }
}

pub fn children(parent: SettingsPage) -> Vec<SettingsPage> {
    (parent == SettingsPage::Threads)
        .then_some(SettingsPage::GlobalPrompt)
        .into_iter()
        .chain(parent.sections().into_iter().map(SettingsPage::Section))
        .collect()
}

fn entry(
    page: SettingsPage,
    icon: Option<AssetId>,
    need: Option<&'static str>,
    current: SettingsPage,
) -> Entry {
    let expanded = page == current.parent();
    Entry {
        page,
        icon,
        need,
        expanded,
        children: if expanded { children(page) } else { Vec::new() },
    }
}

pub fn groups(host: Option<&HostData>, current: SettingsPage) -> Vec<Group> {
    let mut groups = vec![Group {
        heading: "Editor".into(),
        entries: [
            (SettingsPage::General, AssetId::TABLER_SETTINGS),
            (SettingsPage::Account, AssetId::TABLER_SHIELD_LOCK),
            (SettingsPage::Hosts, AssetId::TABLER_DEVICE_LAPTOP),
            (SettingsPage::About, AssetId::TABLER_LIST_DETAILS),
        ]
        .into_iter()
        .map(|(page, icon)| entry(page, Some(icon), None, current))
        .collect(),
    }];
    let Some(host) = host else {
        return groups;
    };
    groups.push(Group {
        heading: host.name.clone(),
        entries: [
            (SettingsPage::Threads, AssetId::TABLER_MESSAGES),
            (SettingsPage::Extensions, AssetId::TABLER_PLUG_CONNECTED),
        ]
        .into_iter()
        .map(|(page, icon)| entry(page, Some(icon), None, current))
        .collect(),
    });
    if !host.engines.is_empty() {
        groups.push(Group {
            heading: "Engines".into(),
            entries: host
                .engines
                .values()
                .map(|status| {
                    entry(
                        SettingsPage::Engine(status.engine),
                        None,
                        engine_need(status),
                        current,
                    )
                })
                .collect(),
        });
    }
    groups
}

#[derive(Default)]
pub struct NavMotion {
    parent: Option<SettingsPage>,
    from: Option<usize>,
    to: Option<usize>,
    started: Option<std::time::Instant>,
    opened: Option<std::time::Instant>,
}

impl NavMotion {
    pub fn sync(&mut self, current: SettingsPage, now: std::time::Instant) {
        let parent = current.parent();
        let to = children(parent).iter().position(|page| *page == current);
        if self.parent != Some(parent) {
            self.parent = Some(parent);
            self.from = None;
            self.to = to;
            self.started = Some(now);
            self.opened = Some(now);
        } else if self.to != to {
            self.from = self.to;
            self.to = to;
            self.started = Some(now);
        }
    }

    pub fn animating(&self, now: std::time::Instant) -> bool {
        self.started
            .is_some_and(|start| now.saturating_duration_since(start).as_millis() < 150)
            || self
                .opened
                .is_some_and(|start| now.saturating_duration_since(start).as_millis() < 150)
    }

    fn progress(start: Option<std::time::Instant>, now: std::time::Instant, reduced: bool) -> f32 {
        if reduced {
            return 1.0;
        }
        let elapsed = start.map_or(150.0, |start| {
            now.saturating_duration_since(start).as_secs_f32() * 1000.0
        });
        editor_ui::picker_motion::smooth_out((elapsed / 150.0).min(1.0))
    }
}

pub fn build(
    host: Option<&HostData>,
    current: SettingsPage,
    theme: editor_ui::theme::ArtisanTheme,
    motion: &NavMotion,
    now: std::time::Instant,
    reduced: bool,
    emit: std::rc::Rc<dyn Fn(SettingsPage)>,
) -> aqua_ui::element::Element {
    use super::usage_view::engine_icon;
    use editor_ui::{
        asset::source_asset_glyph,
        theme::{RadiusStep, RadiusTokens, TextStep, TypographyTokens},
    };
    use aqua_core::{CursorIcon, Insets, Role};
    use aqua_ui::{
        AlignItems,
        element::{Box as UiBox, Button, Scroll, Text},
        style::{Length, Style},
    };

    let mut nav = UiBox::new().layout("column").gap(2.0);
    for group in groups(host, current) {
        nav = nav.child(
            UiBox::new()
                .height(21.0)
                .style(
                    Style::new()
                        .margin_insets(Insets {
                            top: 14.0,
                            ..Insets::default()
                        })
                        .padding_insets(Insets {
                            left: 8.0,
                            right: 8.0,
                            top: 0.0,
                            bottom: 0.0,
                        })
                        .align(AlignItems::CENTER),
                )
                .child(
                    Text::new(group.heading)
                        .font_size(TypographyTokens::size(TextStep::Xs))
                        .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
                        .build(),
                )
                .build(),
        );
        for entry in group.entries {
            let page = entry.page;
            let icon = entry.icon.unwrap_or_else(|| match page {
                SettingsPage::Engine(engine) => engine_icon(&theme, engine),
                _ => AssetId::TABLER_SETTINGS,
            });
            let has_children = !children(page).is_empty();
            let emit_parent = emit.clone();
            let mut row = Button::new()
                .height(34.0)
                .width(Length::Pct(100.0))
                .gap(8.0)
                .radius(RadiusTokens::value(RadiusStep::Md))
                .style(
                    Style::new()
                        .align(AlignItems::CENTER)
                        .padding_insets(Insets {
                            left: 8.0,
                            right: 8.0,
                            top: 0.0,
                            bottom: 0.0,
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
                .child(
                    Text::new(page.label())
                        .font_size(TypographyTokens::size(TextStep::Sm))
                        .gpui_source_color(theme.colors.text.to_reference_text_color())
                        .build(),
                );
            if has_children {
                row = row.child(UiBox::new().style(Style::new().flex_grow(1.0)).build());
                if let Some(need) = entry.need {
                    row = row.child(
                        Text::new(need)
                            .font_size(theme.typography.label_text)
                            .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
                            .build(),
                    );
                }
                row = row.child(source_asset_glyph(
                    if entry.expanded {
                        AssetId::TABLER_CHEVRON_DOWN
                    } else {
                        AssetId::TABLER_CHEVRON_RIGHT
                    },
                    12.0,
                    theme.colors.text_muted.to_reference_paint(),
                ));
            }
            let id = match page {
                SettingsPage::Engine(engine) => format!("sidebar-settings-engine-{engine}"),
                _ => format!(
                    "sidebar-settings-{}",
                    page.label().to_lowercase().replace(' ', "-")
                ),
            };
            let mut row = row
                .on_activate(move || emit_parent(page))
                .build()
                .with_key(id)
                .role(Role::Button)
                .aria_label(page.label())
                .aria_selected(entry.expanded)
                .cursor(CursorIcon::Pointer);
            if has_children {
                row = row.aria_expanded(entry.expanded);
            }
            nav = nav.child(row);
            if entry.expanded && !entry.children.is_empty() {
                let progress = NavMotion::progress(motion.opened, now, reduced);
                let mut children = UiBox::new()
                    .layout("column")
                    .width(Length::Pct(100.0))
                    .style(Style::new().margin_insets(Insets {
                        left: 16.0,
                        top: 4.0,
                        bottom: 4.0,
                        right: 0.0,
                    }))
                    .child(
                        UiBox::new()
                            .width(1.0)
                            .style(
                                Style::new().gpui_background(aqua_core::GpuiBackground::Solid(
                                    theme.colors.line.to_reference_paint(),
                                )),
                            )
                            .style(Style::new().absolute_at(Some(0.0), Some(0.0), None, Some(0.0)))
                            .build(),
                    );
                if let Some(to) = motion.to {
                    let target = (to as f32).mul_add(32.0, 6.0);
                    let p = NavMotion::progress(motion.started, now, reduced);
                    let (top, height, opacity) = match motion.from {
                        Some(from) => (
                            ((target - ((from as f32).mul_add(32.0, 6.0))) * p)
                                + (from as f32).mul_add(32.0, 6.0),
                            20.0,
                            1.0,
                        ),
                        None => (10.0_f32.mul_add(1.0 - p, target), 20.0 * p, p),
                    };
                    children = children.child(
                        UiBox::new()
                            .width(2.0)
                            .height(height)
                            .radius(1.0)
                            .opacity(opacity)
                            .style(
                                Style::new().gpui_background(aqua_core::GpuiBackground::Solid(
                                    theme.colors.text.to_reference_paint(),
                                )),
                            )
                            .style(Style::new().absolute_at(Some(0.0), Some(top), None, None))
                            .build(),
                    );
                }
                let total_height = entry.children.len() as f32 * 32.0;
                for child in entry.children {
                    let selected = child == current;
                    let click = emit.clone();
                    let id = format!(
                        "sidebar-settings-section-{:?}-{}",
                        page,
                        match child {
                            SettingsPage::Section(section) => section.name(),
                            _ => "global-prompt",
                        }
                    );
                    children =
                        children.child(
                            Button::new()
                                .height(32.0)
                                .width(Length::Pct(100.0))
                                .style(Style::new().align(AlignItems::CENTER).padding_insets(
                                    Insets {
                                        left: 17.0,
                                        right: 8.0,
                                        top: 0.0,
                                        bottom: 0.0,
                                    },
                                ))
                                .child(
                                    Text::new(child.label())
                                        .font_size(theme.typography.control_text)
                                        .gpui_source_color(if selected {
                                            theme.colors.text.to_reference_text_color()
                                        } else {
                                            theme.colors.text_muted.to_reference_text_color()
                                        })
                                        .build(),
                                )
                                .on_activate(move || click(child))
                                .build()
                                .with_key(id)
                                .role(Role::Button)
                                .aria_label(child.label())
                                .aria_selected(selected)
                                .cursor(CursorIcon::Pointer),
                        );
                }
                nav = nav.child(
                    UiBox::new()
                        .height(total_height * progress + 8.0)
                        .opacity(progress)
                        .clip(true)
                        .child(children.build())
                        .build(),
                );
            }
        }
    }
    Scroll::new()
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .style(Style::new().padding(Length::Px(16.0)))
        .child(nav.build())
        .build()
        .with_key("settings-dialog-nav")
}
