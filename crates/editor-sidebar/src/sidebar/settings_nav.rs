use artisan_api::{EngineStatus, InstallState, SignInState};
use editor_assets::AssetId;

use editor_core::{HostData, SettingsPage};
use editor_ui::kit::{Density, IconSize, Row, RowItem, Space, Spaced, Surface, TextRole};

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
    opened: Option<std::time::Instant>,
}

impl NavMotion {
    pub fn sync(&mut self, current: SettingsPage, now: std::time::Instant) {
        let parent = current.parent();
        if self.parent != Some(parent) {
            self.parent = Some(parent);
            self.opened = Some(now);
        }
    }

    pub fn animating(&self, now: std::time::Instant) -> bool {
        self.opened
            .is_some_and(|start| now.saturating_duration_since(start).as_millis() < 150)
    }

    fn progress(&self, now: std::time::Instant, reduced: bool) -> f32 {
        if reduced {
            return 1.0;
        }
        let elapsed = self.opened.map_or(150.0, |start| {
            now.saturating_duration_since(start).as_secs_f32() * 1000.0
        });
        editor_ui::picker_motion::smooth_out((elapsed / 150.0).min(1.0))
    }
}

pub const ROW: Row = Row::ListItem(Density::Regular);
const LIST: Surface = Surface::Inset;
const ROW_GAP: Space = Space::S2;
const GROUP_GAP: Space = Space::S12;

pub fn children_height(count: usize) -> f32 {
    let rows = count as f32;
    let gaps = rows.max(1.0) - 1.0;
    rows.mul_add(ROW.height().unwrap_or_default(), gaps * ROW_GAP.px())
}

fn section_id(page: SettingsPage) -> String {
    match page {
        SettingsPage::Engine(engine) => format!("sidebar-settings-engine-{engine}"),
        _ => format!(
            "sidebar-settings-{}",
            page.label().to_lowercase().replace(' ', "-")
        ),
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
    use aqua_ui::{
        AlignItems,
        element::{Box as UiBox, Scroll},
        style::{Length, Style},
    };
    use editor_ui::engine::engine_icon;

    let mut nav = UiBox::new().layout("column").spaced(GROUP_GAP);
    for group in groups(host, current) {
        let mut column = UiBox::new().layout("column").spaced(ROW_GAP).child(
            RowItem::new(
                format!("sidebar-settings-heading-{}", group.heading),
                theme,
                Row::ListHeading,
                LIST,
                group.heading.clone(),
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
            let mut row = RowItem::new(section_id(page), theme, ROW, LIST, page.label())
                .leading(IconSize::Md.glyph(icon, theme.colors.text_muted.to_reference_paint()))
                .selected(entry.expanded && !has_children)
                .on_activate(move || emit_parent(page));
            if has_children {
                let chevron = if entry.expanded {
                    AssetId::TABLER_CHEVRON_DOWN
                } else {
                    AssetId::TABLER_CHEVRON_RIGHT
                };
                let mut trailing = UiBox::new()
                    .spaced(Space::S8)
                    .style(Style::new().align(AlignItems::CENTER).flex_shrink(0.0));
                if let Some(need) = entry.need {
                    trailing = trailing.child(
                        TextRole::Caption
                            .text(&theme, need)
                            .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
                            .build(),
                    );
                }
                row = row.trailing(
                    trailing
                        .child(IconSize::Sm.glyph(chevron, theme.colors.text_muted.to_reference_paint()))
                        .build(),
                );
            }
            let row = row.build();
            column = column.child(match has_children {
                true => row.aria_expanded(entry.expanded),
                false => row,
            });
            if entry.expanded && !entry.children.is_empty() {
                let progress = motion.progress(now, reduced);
                let total_height = children_height(entry.children.len());
                let mut children = UiBox::new()
                    .layout("column")
                    .width(Length::Pct(100.0))
                    .spaced(ROW_GAP);
                for child in entry.children {
                    let click = emit.clone();
                    let id = format!(
                        "sidebar-settings-section-{:?}-{}",
                        page,
                        match child {
                            SettingsPage::Section(section) => section.name(),
                            _ => "global-prompt",
                        }
                    );
                    children = children.child(
                        RowItem::new(id, theme, ROW, LIST, child.label())
                            .leading(UiBox::new().build())
                            .selected(child == current)
                            .on_activate(move || click(child))
                            .build(),
                    );
                }
                column = column.child(
                    UiBox::new()
                        .height(total_height * progress)
                        .opacity(progress)
                        .clip(true)
                        .child(children.build())
                        .build(),
                );
            }
        }
        nav = nav.child(column.build());
    }
    Scroll::new()
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .padded(Space::S16)
        .child(nav.build())
        .build()
        .with_key("settings-dialog-nav")
}
