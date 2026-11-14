use crate::frame::sidebar::{Command, Event};
use crate::sidebar::list::RowState;
use crate::ui::kit::{HitArea, Link, LinkTone, Radius, Row, RowItem, Space, Spaced, Surface, TextRole};
use crate::ui::theme::{ArtisanTheme, Oklch};
use aqua_core::Role;
use aqua_core::scene::GpuiQuadPulse;
use aqua_text::overflow::OverflowMode;
use aqua_ui::element::{Box as UiBox, Element, Text};
use aqua_ui::style::{Length, Style};
use aqua_ui::{AlignItems, JustifyContent};
use artisan_api::{ItemId, ProjectId, ThreadId};
use std::rc::Rc;

pub const ROW: Row = Row::Heading;
pub const HEIGHT: f32 = match ROW.height() {
    Some(height) => height,
    None => 0.0,
};
pub const BAR_HEIGHT: f32 = HEIGHT + Space::S4.px();
pub const RADIUS: Radius = Radius::Panel;
const MIN_WIDTH: f32 = 96.0;
const MAX_WIDTH: f32 = 720.0;
const PULSE_MS: u32 = 1_600;

pub use crate::ui::project_mark::Mark;

pub struct Tab {
    pub id: ThreadId,
    pub project: ProjectId,
    pub trail: Vec<(ItemId, String)>,
    pub title: String,
    pub location: Option<String>,
    pub url: Option<String>,
    pub mark: Mark,
    pub state: RowState,
    pub active: bool,
}

#[must_use]
pub fn strip(
    theme: &ArtisanTheme,
    tabs: &[Tab],
    reduced: bool,
    emit: &Rc<dyn Fn(Event)>,
) -> Vec<Element> {
    tabs.iter()
        .filter(|tab| tab.active)
        .map(|tab| tab_view(theme, tab, reduced, Rc::clone(emit)))
        .collect()
}

fn tab_view(theme: &ArtisanTheme, tab: &Tab, reduced: bool, emit: Rc<dyn Fn(Event)>) -> Element {
    let color = match tab.active {
        true => theme.colors.text,
        false => theme.colors.text_muted,
    };
    let id = tab.id.clone();
    let label = tab
        .location
        .iter()
        .map(String::as_str)
        .chain([tab.title.as_str()])
        .chain(tab.trail.iter().map(|(_, name)| name.as_str()))
        .collect::<Vec<_>>()
        .join(" / ");
    let content = UiBox::new()
        .layout("row")
        .spaced(Space::S6)
        .style(
            TextRole::Label
                .inherit(theme)
                .gpui_source_text_color(color.to_reference_text_color())
                .flex_grow(1.0)
                .min_width(Length::Pct(0.0))
                .align(AlignItems::CENTER),
        )
        .children(
            tab.location
                .as_ref()
                .map(|location_label| location(theme, tab, location_label, Rc::clone(&emit))),
        )
        .children(tab.location.as_ref().map(|_| separator(theme)))
        .child(thread(theme, tab, reduced, Rc::clone(&emit)))
        .children(agents(theme, tab, &emit))
        .build();
    let row = RowItem::new(
        format!("artisan-tab-row-{}", tab.id),
        *theme,
        ROW,
        Surface::Card,
        label.clone(),
    )
    .body(content)
    .build();
    let area = HitArea::new(format!("artisan-tab-{}", tab.id), Role::Group, label)
        .child(row)
        .on_activate(move || emit(Event::Navigate(Command::Open(id.clone()))))
        .build();
    let area = match tab.active {
        true => area.debug_selector(crate::frame::seam::ACTIVE),
        false => area,
    };
    UiBox::new()
        .layout("column")
        .height(if tab.active { BAR_HEIGHT } else { HEIGHT })
        .style(
            Style::new()
                .justify(JustifyContent::END)
                .min_width(Length::Px(MIN_WIDTH))
                .max_width(Length::Px(MAX_WIDTH))
                .flex_shrink(1.0),
        )
        .child(
            UiBox::new()
                .layout("column")
                .height(HEIGHT)
                .style(Style::new().width(Length::Pct(100.0)))
                .child(area)
                .build(),
        )
        .build()
}

fn location(
    theme: &ArtisanTheme,
    tab: &Tab,
    location_label: &str,
    emit: Rc<dyn Fn(Event)>,
) -> Element {
    let name = match &tab.url {
        Some(url) => {
            let url = url.clone();
            Link::new(
                "artisan-thread-repository-link",
                *theme,
                LinkTone::Link,
                location_label,
            )
            .on_activate(move || emit(Event::Navigate(Command::OpenUrl(url.clone()))))
            .build()
            .aria_label(format!("Open {location_label}"))
        }
        None => {
            let project = tab.project.clone();
            Link::new(
                "artisan-thread-project-link",
                *theme,
                LinkTone::Crumb,
                location_label,
            )
            .on_activate(move || emit(Event::Navigate(Command::Project(project.clone()))))
            .build()
            .aria_label(format!("New thread in {location_label}"))
        }
    };
    UiBox::new()
        .layout("row")
        .spaced(Space::S4)
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .min_width(Length::Pct(0.0)),
        )
        .child(tab.mark.view(theme.colors.text_muted))
        .child(name)
        .build()
}

fn words(text: Text, color: Oklch, fill: bool) -> Element {
    frame(
        text.gpui_source_color(color.to_reference_text_color()),
        fill,
    )
}

fn frame(text: Text, fill: bool) -> Element {
    UiBox::new()
        .layout("row")
        .style(
            Style::new()
                .flex_grow(if fill { 1.0 } else { 0.0 })
                .flex_shrink(1.0)
                .min_width(Length::Pct(0.0))
                .clip(true),
        )
        .child(text.wrap(false).overflow(OverflowMode::EndEllipsis).build())
        .build()
}

fn separator(theme: &ArtisanTheme) -> Element {
    words(TextRole::Label.text(theme, "/"), theme.colors.text_muted, false)
}

fn thread(theme: &ArtisanTheme, tab: &Tab, reduced: bool, emit: Rc<dyn Fn(Event)>) -> Element {
    let pulsing = tab.state == RowState::Working && !reduced;
    match tab.trail.is_empty() {
        true => title(frame(Text::new(tab.title.clone()), true), pulsing),
        false => title(
            Link::new("artisan-thread-crumb", *theme, LinkTone::Crumb, tab.title.clone())
                .on_activate(move || emit(Event::Navigate(Command::Agent(None))))
                .build()
                .aria_label(format!("Open {}", tab.title)),
            pulsing,
        ),
    }
}

fn agents(theme: &ArtisanTheme, tab: &Tab, emit: &Rc<dyn Fn(Event)>) -> Vec<Element> {
    let last = tab.trail.len().saturating_sub(1);
    tab.trail
        .iter()
        .enumerate()
        .flat_map(|(index, (agent, name))| {
            let emit = Rc::clone(emit);
            let target = agent.clone();
            let crumb = Link::new(
                format!("artisan-agent-crumb-{agent}"),
                *theme,
                LinkTone::Crumb,
                name.clone(),
            )
            .current(index == last)
            .on_activate(move || emit(Event::Navigate(Command::Agent(Some(target.clone())))))
            .build()
            .aria_label(format!("Open {name}"));
            [separator(theme), crumb]
        })
        .collect()
}

fn title(label: Element, pulsing: bool) -> Element {
    match pulsing {
        true => UiBox::new()
            .layout("row")
            .style(
                Style::new()
                    .flex_grow(1.0)
                    .flex_shrink(1.0)
                    .min_width(Length::Pct(0.0))
                    .gpui_quad_pulse(GpuiQuadPulse {
                        start_ms: 0,
                        period_ms: PULSE_MS,
                        min_opacity: 0.4,
                    }),
            )
            .child(label)
            .build(),
        false => label,
    }
}
