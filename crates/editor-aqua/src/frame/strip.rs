use crate::ui::chrome::card::Surfaces;
use crate::frame::sidebar::{Command, Event};
use crate::frame::tree::{PULSE_MS, inherited_label};
use crate::sidebar::list::RowState;
use crate::ui::theme::ArtisanTheme;
use aqua::Style;
use aqua::{Aqua, px};
use aqua_core::scene::GpuiQuadPulse;
use aqua_core::{CursorIcon, Insets, Role};
use aqua_ui::AlignItems;
use aqua_ui::element::Element;
use artisan_api::{ItemId, ProjectId, ThreadId};
use std::rc::Rc;

pub const BAR_HEIGHT: f32 = HEIGHT + 4.0;
pub(super) const HEIGHT: f32 = 32.0;
const RADIUS: f32 = 8.0;
const MIN_WIDTH: f32 = 96.0;
const MAX_WIDTH: f32 = 720.0;
const PAD: f32 = 12.0;

pub use crate::sidebar::project_mark::Mark;

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
    let surfaces = Surfaces::of(theme);
    let color = if tab.active {
        surfaces.ink
    } else {
        surfaces.ink_dim
    };
    let id = tab.id.clone();
    let content = Aqua::Box()
        .layout("row")
        .styles([Aqua::Style()
            .flex_grow(1.0)
            .align(AlignItems::CENTER)
            .gap(6.0)
            .padding_insets(Insets {
                left: PAD,
                right: PAD,
                ..Insets::default()
            })])
        .children(
            tab.location
                .as_ref()
                .map(|location_label| location(theme, tab, location_label, Rc::clone(&emit))),
        )
        .children(
            tab.location
                .as_ref()
                .map(|_| crate::frame::tree::label(theme, "/", surfaces.ink_dim, false)),
        )
        .child(thread(theme, tab, reduced, Rc::clone(&emit)))
        .children(agents(theme, tab, &emit))
        .build();
    let button = Aqua::Button()
        .layout("column")
        .styles([Aqua::Style()
            .height(px(HEIGHT))
            .margin_insets(Insets {
                top: match tab.active {
                    true => BAR_HEIGHT - HEIGHT,
                    false => 0.0,
                },
                ..Insets::default()
            })
            .min_width(px(MIN_WIDTH))
            .max_width(px(MAX_WIDTH))
            .flex_shrink(1.0)
            .radius(px(RADIUS))
            .gpui_source_text_color(color.to_reference_text_color())])
        .child(content)
        .on_activate(move || emit(Event::Navigate(Command::Open(id.clone()))))
        .build()
        .with_key(format!("artisan-tab-{}", tab.id))
        .role(Role::Group)
        .aria_label(
            tab.location
                .iter()
                .map(String::as_str)
                .chain([tab.title.as_str()])
                .chain(tab.trail.iter().map(|(_, name)| name.as_str()))
                .collect::<Vec<_>>()
                .join(" / "),
        )
        .cursor(CursorIcon::Pointer);
    match tab.active {
        true => button.debug_selector(crate::frame::seam::ACTIVE),
        false => button.hover_style(lit(theme)),
    }
}

fn location(
    theme: &ArtisanTheme,
    tab: &Tab,
    location_label: &str,
    emit: Rc<dyn Fn(Event)>,
) -> Element {
    let content = Aqua::Box()
        .layout("row")
        .styles([Aqua::Style()
            .align(AlignItems::CENTER)
            .gap(3.0)
            .min_width(px(0.0))])
        .child(tab.mark.view(theme.colors.text_muted))
        .child(match tab.url {
            Some(_) => crate::frame::tree::label(theme, location_label, theme.colors.link, false),
            None => inherited_label(theme, location_label, false),
        })
        .build();
    match &tab.url {
        Some(url) => {
            let url = url.clone();
            Aqua::Button()
                .styles([Aqua::Style().min_width(px(0.0)).flex_shrink(1.0)])
                .child(content)
                .on_activate(move || emit(Event::Navigate(Command::OpenUrl(url.clone()))))
                .build()
                .with_key("artisan-thread-repository-link")
                .role(Role::Button)
                .aria_label(format!("Open {location_label}"))
                .cursor(CursorIcon::Pointer)
        }
        None => {
            let project = tab.project.clone();
            Aqua::Button()
                .styles([
                    Aqua::Style().min_width(px(0.0)).flex_shrink(1.0),
                    dim(theme),
                ])
                .child(content)
                .on_activate(move || emit(Event::Navigate(Command::Project(project.clone()))))
                .build()
                .with_key("artisan-thread-project-link")
                .role(Role::Button)
                .aria_label(format!("New thread in {location_label}"))
                .cursor(CursorIcon::Pointer)
                .hover_style(lit(theme))
        }
    }
}

fn lit(theme: &ArtisanTheme) -> Style {
    Aqua::Style().gpui_source_text_color(Surfaces::of(theme).ink.to_reference_text_color())
}

fn dim(theme: &ArtisanTheme) -> Style {
    Aqua::Style().gpui_source_text_color(Surfaces::of(theme).ink_dim.to_reference_text_color())
}

fn thread(theme: &ArtisanTheme, tab: &Tab, reduced: bool, emit: Rc<dyn Fn(Event)>) -> Element {
    let words = title(
        theme,
        &tab.title,
        tab.trail.is_empty(),
        tab.state == RowState::Working && !reduced,
    );
    match tab.trail.is_empty() {
        true => words,
        false => Aqua::Button()
            .styles([
                Aqua::Style().min_width(px(0.0)).flex_shrink(1.0),
                dim(theme),
            ])
            .child(words)
            .on_activate(move || emit(Event::Navigate(Command::Agent(None))))
            .build()
            .with_key("artisan-thread-crumb")
            .role(Role::Button)
            .aria_label(format!("Open {}", tab.title))
            .cursor(CursorIcon::Pointer)
            .hover_style(lit(theme)),
    }
}

fn agents(theme: &ArtisanTheme, tab: &Tab, emit: &Rc<dyn Fn(Event)>) -> Vec<Element> {
    let surfaces = Surfaces::of(theme);
    let last = tab.trail.len().saturating_sub(1);
    tab.trail
        .iter()
        .enumerate()
        .flat_map(|(index, (agent, name))| {
            let emit = Rc::clone(emit);
            let target = agent.clone();
            let crumb = Aqua::Button()
                .styles([
                    Aqua::Style()
                        .min_width(px(0.0))
                        .flex_grow(if index == last { 1.0 } else { 0.0 })
                        .flex_shrink(if index == last { 1.0 } else { 0.0 }),
                    if index == last {
                        lit(theme)
                    } else {
                        dim(theme)
                    },
                ])
                .child(inherited_label(theme, name, index == last))
                .on_activate(move || emit(Event::Navigate(Command::Agent(Some(target.clone())))))
                .build()
                .with_key(format!("artisan-agent-crumb-{agent}"))
                .role(Role::Button)
                .aria_label(format!("Open {name}"))
                .cursor(CursorIcon::Pointer)
                .hover_style(lit(theme));
            [
                crate::frame::tree::label(theme, "/", surfaces.ink_dim, false),
                crumb,
            ]
        })
        .collect()
}

fn title(theme: &ArtisanTheme, words: &str, fill: bool, pulsing: bool) -> Element {
    let label = inherited_label(theme, words, fill);
    match pulsing {
        true => Aqua::Box()
            .layout("row")
            .styles([
                Aqua::Style()
                    .flex_grow(if fill { 1.0 } else { 0.0 })
                    .flex_shrink(1.0)
                    .min_width(px(0.0)),
                Style::new().gpui_quad_pulse(GpuiQuadPulse {
                    start_ms: 0,
                    period_ms: PULSE_MS,
                    min_opacity: 0.4,
                }),
            ])
            .child(label)
            .build(),
        false => label,
    }
}
