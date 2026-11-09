use crate::core::HostData;
use crate::frame::projects::{Group, Metrics, ROW_GAP, ROW_HEIGHT, ThreadRow};
use crate::frame::sidebar::{Command, Event};
use crate::sidebar::list::RowState;
use crate::ui::theme::{ArtisanTheme, Oklch};
use aqua::{Aqua, Style, pct, px};
use aqua_core::scene::GpuiQuadPulse;
use aqua_core::{
    CursorIcon, DisplayItem, GpuiBackground, GpuiGradientColorSpace, GpuiGradientStop, Insets, Role,
};
use aqua_text::overflow::OverflowMode;
use aqua_text::{FontFamily, FontWeight, TextStyle};
use aqua_ui::element::{Element, Scroll};
use aqua_ui::style::Length;
use aqua_ui::{AlignItems, JustifyContent};
use std::{rc::Rc, sync::Arc};

pub const ROW_PAD: f32 = 8.0;
const ROW_RADIUS: f32 = 6.0;
const ICON: f32 = 16.0;
const GAP: f32 = 8.0;
const FADE: f32 = 32.0;
const RAIL_MIX: f32 = 0.45;
const RAIL_INSET: f32 = 8.0;
const BODY_BLUR: f32 = 2.0;
const DOT: f32 = 6.0;
const TEXT: f32 = 14.0;
const LINE: f32 = 20.0;
pub const PULSE_MS: u32 = 1_600;

pub struct Tree<'a> {
    pub groups: &'a [Group],
    pub openness: &'a dyn Fn(&artisan_api::ProjectId) -> f32,
    pub pinned: Option<&'a (artisan_api::ProjectId, f32)>,
    pub metrics: Metrics,
    pub host: &'a HostData,
    pub reduced: bool,
    pub offset: aqua_core::Point,
    pub fades: (f32, f32),
    pub emit: Rc<dyn Fn(Event)>,
}

#[must_use]
pub fn tree(theme: &ArtisanTheme, tree: &Tree<'_>) -> Element {
    let mut column = Aqua::Box()
        .layout("column")
        .styles([Aqua::Style().width(pct(100.0)).min_width(px(0.0))]);
    for (index, group) in tree.groups.iter().enumerate() {
        let open = (tree.openness)(&group.project);
        let row = project_row(theme, tree, group, "artisan-tree-project");
        let stuck = tree.pinned.is_some_and(|(project, _)| *project == group.project);
        column = column.child(
            Aqua::Box()
                .layout("column")
                .styles([Aqua::Style()
                    .margin_insets(Insets {
                        top: if index == 0 { 0.0 } else { ROW_GAP },
                        ..Insets::default()
                    })
                    .opacity(if stuck { 0.0 } else { 1.0 })])
                .child(row)
                .build()
                .with_key(format!("artisan-tree-header-{}", group.project)),
        );
        if open > 0.0 && !group.threads.is_empty() {
            column = column.child(body(theme, group, tree.metrics, open, tree.reduced, &tree.emit));
        }
    }
    let scroll = Scroll::new()
        .offset(tree.offset)
        .width(Length::Pct(100.0))
        .style(
            Style::new()
                .column()
                .flex_grow(1.0)
                .min_width(Length::Px(0.0))
                .min_height(Length::Px(0.0)),
        )
        .child(column.build())
        .build()
        .with_key(crate::shell::wheel::PROJECT_TREE);
    let (above, below) = tree.fades;
    let pinned = tree.pinned.and_then(|(project, shift)| {
        let group = tree.groups.iter().find(|group| &group.project == project)?;
        Some((pin(theme, tree, group, *shift), tree.metrics.row + shift))
    });
    let fade_top = pinned.as_ref().map_or(0.0, |(_, bottom)| *bottom);
    Aqua::Box()
        .layout("column")
        .styles([Aqua::Style()
            .flex_grow(1.0)
            .min_width(px(0.0))
            .min_height(px(0.0))])
        .child(scroll)
        .children((above > 0.0).then(|| fade(theme, Some(fade_top), above)))
        .children((below > 0.0).then(|| fade(theme, None, below)))
        .children(pinned.map(|(header, _)| header))
        .build()
}

fn pin(theme: &ArtisanTheme, tree: &Tree<'_>, group: &Group, shift: f32) -> Element {
    Aqua::Box()
        .layout("column")
        .styles([Style::new()
            .absolute_at(Some(0.0), Some(shift), Some(0.0), None)
            .gpui_background(GpuiBackground::Solid(theme.colors.frame.to_reference_paint()))])
        .child(project_row(theme, tree, group, "artisan-tree-pinned"))
        .build()
}

fn body(theme: &ArtisanTheme, group: &Group, metrics: Metrics, open: f32, reduced: bool, emit: &Rc<dyn Fn(Event)>) -> Element {
    let full = metrics.body_height(group.threads.len());
    let mut style = Aqua::Style()
        .height(px(full * open))
        .flex_shrink(0.0)
        .clip(true)
        .opacity(open);
    if open < 0.99 {
        style = style.content_blur(BODY_BLUR * (1.0 - open));
    }
    Aqua::Box()
        .layout("column")
        .styles([style])
        .child(rail(theme, group, reduced, emit))
        .build()
        .with_key(format!("artisan-tree-body-{}", group.project))
}

fn fade(theme: &ArtisanTheme, top: Option<f32>, opacity: f32) -> Element {
    let edge = theme.colors.frame;
    let (place, angle) = match top {
        Some(top) => (Style::new().absolute_at(Some(0.0), Some(top), Some(0.0), None), 180.0),
        None => (Style::new().absolute_at(Some(0.0), None, Some(0.0), Some(0.0)), 0.0),
    };
    Aqua::Box()
        .styles([place
            .height(px(FADE))
            .opacity(opacity)
            .gpui_background(GpuiBackground::LinearGradient {
                angle,
                color_space: GpuiGradientColorSpace::Oklab,
                stops: [
                    GpuiGradientStop {
                        percentage: 0.0,
                        color: edge.to_reference_paint(),
                    },
                    GpuiGradientStop {
                        percentage: 1.0,
                        color: edge.with_alpha(0.0).to_reference_paint(),
                    },
                ],
            })])
        .build()
        .exclude_from_hit_testing()
}

fn rail(theme: &ArtisanTheme, group: &Group, reduced: bool, emit: &Rc<dyn Fn(Event)>) -> Element {
    let line = theme.colors.frame.animate_to(theme.colors.text, RAIL_MIX);
    Aqua::Box()
        .layout("column")
        .styles([Aqua::Style()
            .gap(ROW_GAP)
            .flex_shrink(0.0)
            .min_width(px(0.0))
            .margin_insets(Insets {
                left: ROW_PAD + ICON / 2.0 - 0.5,
                top: ROW_GAP,
                ..Insets::default()
            })
            .padding_insets(Insets {
                left: RAIL_INSET,
                ..Insets::default()
            })
            .gpui_border(
                Insets {
                    left: 1.0,
                    ..Insets::default()
                },
                line.to_reference_paint(),
                false,
            )])
        .children(
            group
                .threads
                .iter()
                .map(|row| thread_row(theme, row, reduced, Rc::clone(emit))),
        )
        .build()
}

fn project_row(theme: &ArtisanTheme, tree: &Tree<'_>, group: &Group, key: &str) -> Element {
    let forge = tree
        .host
        .repository(&group.project)
        .map(|repository| repository.host.as_deref());
    let mark = crate::sidebar::project_mark::project_mark(
        tree.host.project_icon(&group.project),
        forge,
        ICON,
        theme.colors.text_muted.to_reference_paint(),
    );
    let emit = Rc::clone(&tree.emit);
    let project = group.project.clone();
    Aqua::Button()
        .styles([row_style()])
        .child(
            Aqua::Box()
                .styles([Aqua::Style()
                    .width(px(ICON))
                    .height(px(ICON))
                    .flex_shrink(0.0)])
                .child(mark)
                .build(),
        )
        .child(label(theme, &group.name, theme.colors.text, true))
        .on_activate(move || emit(Event::Project(project.clone())))
        .build()
        .with_key(format!("{key}-{}", group.project))
        .role(Role::Button)
        .aria_label(group.name.clone())
        .cursor(CursorIcon::Pointer)
        .hover_style(hover(theme))
}

fn thread_row(theme: &ArtisanTheme, row: &ThreadRow, reduced: bool, emit: Rc<dyn Fn(Event)>) -> Element {
    let color = if row.selected {
        theme.colors.thread_title_active
    } else {
        theme.colors.text
    };
    let id = row.id.clone();
    Aqua::Button()
        .styles([row_style()])
        .child(label(theme, &row.title, color, true))
        .children(state_dot(theme, row.state, reduced))
        .on_activate(move || emit(Event::Navigate(Command::Open(id.clone()))))
        .build()
        .with_key(format!("artisan-tree-thread-{}", row.id))
        .role(Role::Button)
        .aria_label(row.title.clone())
        .cursor(CursorIcon::Pointer)
        .hover_style(hover(theme))
}

#[must_use]
pub fn state_dot(theme: &ArtisanTheme, state: RowState, reduced: bool) -> Option<Element> {
    let color = match state {
        RowState::Working => theme.colors.text,
        RowState::Waiting => theme.colors.banner_warning.text,
        RowState::Unread => theme.colors.unread,
        RowState::Failed => theme.colors.red.solid,
        RowState::Idle | RowState::Parked => return None,
    };
    let mut dot = Style::new()
        .width(px(DOT))
        .height(px(DOT))
        .radius(px(DOT / 2.0))
        .gpui_background(GpuiBackground::Solid(color.to_reference_paint()));
    if state == RowState::Working && !reduced {
        dot = dot.gpui_quad_pulse(GpuiQuadPulse {
            start_ms: 0,
            period_ms: PULSE_MS,
            min_opacity: 0.4,
        });
    }
    Some(
        Aqua::Box()
            .styles([Aqua::Style()
                .width(px(ICON))
                .height(px(ICON))
                .flex_shrink(0.0)
                .align(AlignItems::CENTER)
                .justify(JustifyContent::CENTER)])
            .child(Aqua::Box().styles([dot]).build())
            .build(),
    )
}

fn row_style() -> Style {
    Aqua::Style()
        .width(pct(100.0))
        .height(px(ROW_HEIGHT))
        .flex_shrink(0.0)
        .align(AlignItems::CENTER)
        .gap(GAP)
        .padding_insets(Insets {
            left: ROW_PAD,
            right: ROW_PAD,
            ..Insets::default()
        })
        .radius(px(ROW_RADIUS))
}

#[must_use]
pub fn hover(theme: &ArtisanTheme) -> Style {
    let (top, bottom) = crate::ui::hover::hover_fill_stops(theme, theme.colors.text);
    Style::new().gpui_background(GpuiBackground::LinearGradient {
        angle: 180.0,
        color_space: GpuiGradientColorSpace::Oklab,
        stops: [
            GpuiGradientStop {
                percentage: 0.0,
                color: top.to_reference_paint(),
            },
            GpuiGradientStop {
                percentage: 1.0,
                color: bottom.to_reference_paint(),
            },
        ],
    })
}

#[must_use]
pub fn label(theme: &ArtisanTheme, words: &str, color: Oklch, fill: bool) -> Element {
    words_box(text(theme, words).gpui_color(color.to_reference_paint()).build(), fill)
}

#[must_use]
pub fn inherited_label(theme: &ArtisanTheme, words: &str, fill: bool) -> Element {
    words_box(text(theme, words).build(), fill)
}

fn text(theme: &ArtisanTheme, words: &str) -> aqua_ui::element::Text {
    Aqua::Text(words.to_owned())
        .text_style(TextStyle {
            families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
            size_px: TEXT,
            line_height_px: LINE,
            weight: FontWeight::NORMAL,
            ..TextStyle::default()
        })
        .wrap(false)
        .overflow(OverflowMode::EndEllipsis)
}

fn words_box(text: Element, fill: bool) -> Element {
    Aqua::Box()
        .layout("row")
        .styles([Aqua::Style()
            .flex_grow(if fill { 1.0 } else { 0.0 })
            .flex_shrink(1.0)
            .min_width(px(0.0))
            .clip(true)])
        .child(text)
        .build()
}

#[must_use]
pub fn glyph(source: &'static str, edge: f32, tint: Oklch) -> Element {
    let source: Arc<str> = source.into();
    let tint = tint.to_reference_paint();
    Aqua::Canvas()
        .styles([Aqua::Style()
            .width(px(edge))
            .height(px(edge))
            .flex_shrink(0.0)])
        .on_paint(move |bounds| {
            vec![DisplayItem::GpuiSvg {
                bounds,
                source: Arc::clone(&source),
                tint: Some(tint),
                opacity: 1.0,
            }]
        })
        .build()
}
