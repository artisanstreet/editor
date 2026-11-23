use std::{rc::Rc, time::Instant};

use aqua_ui::{
    AlignItems,
    element::{Box as UiBox, Element, Scroll},
    style::{Length, Style},
};
use editor_assets::AssetId;
use editor_core::{HostData, SettingsPage, SettingsSection};
use editor_ui::{
    engine::engine_icon,
    kit::{Density, IconSize, Row, RowItem, Space, Spaced, Surface},
    theme::ArtisanTheme,
};

#[derive(Debug, Default)]
pub struct NavMotion {
    page: Option<SettingsPage>,
    opened: Option<Instant>,
}

impl NavMotion {
    pub(crate) fn sync(&mut self, page: SettingsPage, now: Instant) {
        if self.page != Some(page) {
            self.page = Some(page);
            self.opened = Some(now);
        }
    }

    pub(crate) fn animating(&self, now: Instant) -> bool {
        self.opened
            .is_some_and(|start| now.saturating_duration_since(start).as_millis() < 150)
    }

    fn progress(&self, now: Instant, reduced: bool) -> f32 {
        if reduced {
            return 1.0;
        }
        let elapsed = self.opened.map_or(150.0, |start| {
            now.saturating_duration_since(start).as_secs_f32() * 1000.0
        });
        editor_ui::picker_motion::smooth_out((elapsed / 150.0).min(1.0))
    }
}

fn entry(
    theme: &ArtisanTheme,
    page: SettingsPage,
    icon: AssetId,
    current: SettingsPage,
    motion: f32,
    emit: &Rc<dyn Fn(SettingsPage)>,
) -> Element {
    let foreground = if matches!(page, SettingsPage::Engine(_)) {
        theme.colors.text
    } else {
        theme.colors.text_muted
    };
    let selected = page == current
        || (matches!(current, SettingsPage::Section(_)) && page == current.parent());
    let action = Rc::clone(emit);
    let selector = format!(
        "sidebar-settings-{}",
        page.label().to_lowercase().replace(' ', "-")
    );
    let row = RowItem::new(
        selector.clone(),
        *theme,
        Row::ListItem(Density::Regular),
        Surface::Inset,
        page.label(),
    )
    .leading(IconSize::Md.glyph(icon, foreground.to_reference_paint()))
    .on_activate(move || action(page))
    .build()
    .debug_selector(selector)
    .aria_selected(selected);
    UiBox::new()
        .style(Style::new().align(AlignItems::CENTER))
        .child(
            UiBox::new()
                .width(2.0)
                .height(18.0)
                .style(editor_ui::kit::Divider::selection_guide(theme))
                .opacity(if selected { motion } else { 0.0 })
                .build(),
        )
        .child(row)
        .build()
}

pub fn build(
    host: Option<&HostData>,
    current: SettingsPage,
    theme: &ArtisanTheme,
    motion: &NavMotion,
    now: Instant,
    reduced: bool,
    emit: &Rc<dyn Fn(SettingsPage)>,
) -> Element {
    let tools = [
        (SettingsPage::General, AssetId::TABLER_SETTINGS),
        (SettingsPage::Account, AssetId::TABLER_USER),
        (SettingsPage::About, AssetId::TABLER_LIST_DETAILS),
        (SettingsPage::Hosts, AssetId::TABLER_DEVICE_LAPTOP),
        (SettingsPage::Threads, AssetId::TABLER_MESSAGES),
        (SettingsPage::GlobalPrompt, AssetId::TABLER_FILE_TEXT),
        (
            SettingsPage::Section(SettingsSection::McpServers),
            AssetId::TABLER_PLUG_CONNECTED,
        ),
        (
            SettingsPage::Section(SettingsSection::Skills),
            AssetId::TABLER_TOOL,
        ),
    ];
    let group = |entries: &[(SettingsPage, AssetId)]| {
        let progress = motion.progress(now, reduced);
        entries
            .iter()
            .fold(
                UiBox::new().layout("column").spaced(Space::S2),
                |column, &(page, icon)| {
                    column.child(entry(theme, page, icon, current, progress, emit))
                },
            )
            .build()
    };
    let engines: Vec<_> = host
        .into_iter()
        .flat_map(|host| host.engines.values())
        .map(|status| {
            (
                SettingsPage::Engine(status.engine),
                engine_icon(theme, status.engine),
            )
        })
        .collect();
    let rail = UiBox::new()
        .layout("column")
        .spaced(Space::S32)
        .child(group(&tools))
        .children((!engines.is_empty()).then(|| group(&engines)))
        .build();
    Scroll::new()
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .child(rail)
        .build()
        .with_key("settings-dialog-nav")
}
