#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "this module is the Settings layout layer; its geometry is the carved prototype's, which has no kit tokens"
)]

use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Element},
    style::{Length, Style},
};

use editor_core::Problem;
use editor_ui::{
    carved::{
        Carve, RADIUS_MD, Type,
        button::{CarvedButton, Content, Tone as CarvedTone},
        face::{bevel, outline_layer},
        glyph::Icon,
    },
    kit::{ButtonTone, ProblemNotice, Space, Spaced, Surface, TextRole, Tone, surface::tint},
    theme::ArtisanTheme,
};

pub(crate) const CONTROL_WIDTH: f32 = 148.0;
const ROW_HEIGHT: f32 = 64.0;
const PAD_X: f32 = 20.0;
const SECTION_TITLE: Type = Type::heading(15.2, 19.0, 500);
pub(crate) const LABEL: Type = Type::sans(15.0, 20.0, 400);
pub(crate) const DESC: Type = Type::sans(13.44, 19.0, 400);
const META: Type = Type::sans(13.0, 18.0, 400);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ink {
    Plain,
    Muted,
    Success,
    Danger,
}

fn ink_color(theme: &ArtisanTheme, ink: Ink) -> editor_ui::theme::Oklch {
    let carve = Carve::of(theme);
    match ink {
        Ink::Plain => carve.ink,
        Ink::Muted => carve.ink_dim,
        Ink::Success => carve.ok,
        Ink::Danger => carve.err,
    }
}

fn styled(theme: &ArtisanTheme, role: TextRole, ink: Ink, value: String, wraps: bool) -> Element {
    let color = ink_color(theme, ink);
    role.tracked(theme, value)
        .gpui_source_color(color.to_reference_text_color())
        .wrap(wraps)
        .build()
}

pub(crate) fn text(
    theme: &ArtisanTheme,
    role: TextRole,
    ink: Ink,
    value: impl Into<String>,
) -> Element {
    styled(theme, role, ink, value.into(), true)
}

pub(crate) fn markdown(theme: &ArtisanTheme, kind: Type, ink: Ink, source: &str) -> Element {
    let color = ink_color(theme, ink).to_reference_text_color();
    let style = kind.style(theme);
    let runs = editor_ui::inline_code_text::inline_runs(source);
    let spans = runs.styled_spans(&style, color, theme);
    kind.text(theme, ink_color(theme, ink), runs.text)
        .spans(spans)
        .wrap(true)
        .build()
}

pub(crate) fn line(
    theme: &ArtisanTheme,
    role: TextRole,
    ink: Ink,
    value: impl Into<String>,
) -> Element {
    styled(theme, role, ink, value.into(), false)
}

pub(crate) fn typed(
    theme: &ArtisanTheme,
    kind: Type,
    ink: Ink,
    value: impl Into<String>,
) -> Element {
    kind.text(theme, ink_color(theme, ink), value.into())
        .wrap(true)
        .build()
}

pub(crate) fn label(theme: &ArtisanTheme, value: impl Into<String>) -> Element {
    LABEL
        .text(theme, ink_color(theme, Ink::Plain), value.into())
        .wrap(false)
        .build()
}

pub(crate) fn desc(theme: &ArtisanTheme, value: impl Into<String>) -> Element {
    UiBox::new()
        .style(Style::new().max_width(Length::Px(560.0)))
        .child(typed(theme, DESC, Ink::Muted, value))
        .build()
}

fn meta(theme: &ArtisanTheme, value: impl Into<String>) -> Element {
    META.text(theme, ink_color(theme, Ink::Muted), value.into())
        .wrap(false)
        .build()
}

pub(crate) fn column() -> UiBox {
    UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .style(Style::new().gap(28.0))
}

const PAGE_PAD_X: f32 = 36.0;
const PAGE_WIDTH: f32 = 880.0 + 2.0 * PAGE_PAD_X;

pub(crate) fn prompt_page() -> UiBox {
    UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .style(
            Style::new()
                .gap(16.0)
                .max_width(Length::Px(PAGE_WIDTH))
                .min_height(Length::Pct(0.0))
                .padding_insets(aqua_core::Insets {
                    left: PAGE_PAD_X,
                    right: PAGE_PAD_X,
                    top: 24.0,
                    bottom: 28.0,
                }),
        )
}

pub(crate) fn prompt_body(theme: &ArtisanTheme, above: Element, editor: Element) -> Element {
    let carve = Carve::of(theme);
    UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .style(
            Style::new()
                .gap(12.0)
                .flex_grow(1.0)
                .min_height(Length::Pct(0.0))
                .padding_insets(aqua_core::Insets {
                    left: PAD_X,
                    right: PAD_X,
                    top: 16.0,
                    bottom: 16.0,
                }),
        )
        .child(above)
        .child(
            UiBox::new()
                .layout("column")
                .width(Length::Pct(100.0))
                .style(
                    Style::new()
                        .flex_grow(1.0)
                        .min_height(Length::Px(220.0))
                        .radius(Length::Px(RADIUS_MD))
                        .background(carve.well.to_paint())
                        .shadows(carve.inset()),
                )
                .clip(true)
                .child(editor)
                .build(),
        )
        .build()
}

pub(crate) fn inline(gap: f32) -> UiBox {
    UiBox::new().style(Style::new().gap(gap).align(AlignItems::CENTER))
}

pub(crate) fn page_body() -> UiBox {
    UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .style(
            Style::new()
                .max_width(Length::Px(PAGE_WIDTH))
                .padding_insets(aqua_core::Insets {
                    left: PAGE_PAD_X,
                    right: PAGE_PAD_X,
                    top: 28.0,
                    bottom: 40.0,
                }),
        )
}

pub(crate) fn stack(space: Space) -> UiBox {
    UiBox::new().layout("column").spaced(space)
}

pub(crate) fn actions() -> UiBox {
    UiBox::new().style(
        Style::new()
            .gap(8.0)
            .align(AlignItems::CENTER)
            .justify(JustifyContent::END)
            .flex_shrink(0.0),
    )
}

pub(crate) fn fixed(child: Element) -> Element {
    UiBox::new()
        .width(CONTROL_WIDTH)
        .style(Style::new().flex_shrink(0.0))
        .child(child)
        .build()
}

pub(crate) fn grow(child: Element) -> Element {
    UiBox::new()
        .style(Style::new().flex_grow(1.0).min_width(Length::Pct(0.0)))
        .child(child)
        .build()
}

const fn carved_tone(tone: ButtonTone) -> CarvedTone {
    match tone {
        ButtonTone::Primary | ButtonTone::Special => CarvedTone::Horizon,
        ButtonTone::Neutral => CarvedTone::Secondary,
        ButtonTone::Quiet => CarvedTone::Quiet,
    }
}

pub(crate) fn button(
    theme: &ArtisanTheme,
    id: impl Into<String>,
    label: impl Into<String>,
    tone: ButtonTone,
    disabled: bool,
    on_activate: impl Fn() + 'static,
) -> Element {
    CarvedButton::label(id.into(), *theme, carved_tone(tone), label)
        .disabled(disabled)
        .on_activate(move |_| on_activate())
        .build()
}

pub(crate) fn carved(
    theme: &ArtisanTheme,
    id: impl Into<String>,
    tone: CarvedTone,
    content: Content,
    disabled: bool,
    on_activate: impl Fn() + 'static,
) -> Element {
    CarvedButton::new(id.into(), *theme, tone, content)
        .disabled(disabled)
        .on_activate(move |_| on_activate())
        .build()
}

pub(crate) fn gear(
    theme: &ArtisanTheme,
    id: impl Into<String>,
    name: impl Into<String>,
    on_activate: impl Fn() + 'static,
) -> Element {
    carved(
        theme,
        id,
        CarvedTone::Secondary,
        Content::Icon(Icon::Gear, name.into()),
        false,
        on_activate,
    )
}

pub(crate) fn section(
    theme: &ArtisanTheme,
    title: &str,
    action: Option<Element>,
    body: Element,
) -> Element {
    let heading = SECTION_TITLE
        .text(theme, ink_color(theme, Ink::Plain), title.to_owned())
        .wrap(false)
        .build()
        .role(aqua_core::Role::Heading(2));
    let head = UiBox::new()
        .style(
            Style::new()
                .gap(12.0)
                .padding_insets(aqua_core::Insets {
                    left: 2.0,
                    right: 2.0,
                    top: 0.0,
                    bottom: 0.0,
                })
                .align(AlignItems::END)
                .justify(JustifyContent::SPACE_BETWEEN),
        )
        .child(heading)
        .children(action);
    UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .style(Style::new().gap(10.0).flex_shrink(0.0))
        .child(head.build())
        .child(body)
        .build()
}

pub(crate) fn bare_section(body: Element) -> Element {
    UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .style(Style::new().flex_shrink(0.0))
        .child(body)
        .build()
}

pub(crate) fn card(theme: &ArtisanTheme, blocks: Vec<Element>) -> Element {
    card_box(theme, blocks).build()
}

pub(crate) fn ruled(theme: &ArtisanTheme, block: Element) -> Element {
    UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .child(block)
        .child(bevel(&Carve::of(theme)))
        .build()
}

pub(crate) fn card_box(theme: &ArtisanTheme, blocks: Vec<Element>) -> UiBox {
    let carve = Carve::of(theme);
    let rows = blocks.into_iter().enumerate().map(|(index, block)| {
        if index == 0 {
            block
        } else {
            ruled(theme, block)
        }
    });
    UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .style(
            Style::new()
                .radius(Length::Px(RADIUS_MD))
                .background(carve.card.to_paint())
                .shadows(carve.rest())
                .flex_shrink(0.0),
        )
        .child(outline_layer(&carve, RADIUS_MD))
        .children(rows)
}

pub(crate) fn row(
    theme: &ArtisanTheme,
    title: impl Into<String>,
    description: impl Into<String>,
    control: Option<Element>,
) -> Element {
    let description = description.into();
    let text = if description.is_empty() {
        label(theme, title)
    } else {
        stack_gap(3.0)
            .child(label(theme, title))
            .child(desc(theme, description.clone()))
            .build()
    };
    row_of(theme, text, control, !description.is_empty())
}

pub(crate) fn stack_gap(pixels: f32) -> UiBox {
    UiBox::new()
        .layout("column")
        .style(Style::new().gap(pixels).min_width(Length::Px(0.0)))
}

pub(crate) fn row_of(
    _theme: &ArtisanTheme,
    text: Element,
    control: Option<Element>,
    auto: bool,
) -> Element {
    let mut row = UiBox::new().width(Length::Pct(100.0)).style(
        Style::new()
            .gap(24.0)
            .align(AlignItems::CENTER)
            .justify(JustifyContent::SPACE_BETWEEN)
            .padding_insets(aqua_core::Insets {
                left: PAD_X,
                right: PAD_X,
                top: if auto { 16.0 } else { 0.0 },
                bottom: if auto { 16.0 } else { 0.0 },
            }),
    );
    if !auto {
        row = row.height(ROW_HEIGHT);
    }
    row.child(
        UiBox::new()
            .style(Style::new().flex_shrink(1.0).min_width(Length::Px(0.0)))
            .child(text)
            .build(),
    )
    .children(control.map(|control| {
        UiBox::new()
            .style(
                Style::new()
                    .gap(8.0)
                    .align(AlignItems::CENTER)
                    .flex_shrink(0.0),
            )
            .child(control)
            .build()
    }))
    .build()
}

pub(crate) fn block(content: Element) -> Element {
    UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .style(Style::new().padding_insets(aqua_core::Insets {
            left: PAD_X,
            right: PAD_X,
            top: 16.0,
            bottom: 16.0,
        }))
        .child(content)
        .build()
}

pub(crate) fn note(theme: &ArtisanTheme, value: &str) -> Element {
    row_of(theme, desc(theme, value), None, false)
}

pub(crate) fn problem(
    theme: &ArtisanTheme,
    key: impl Into<aqua_ui::element::Key>,
    problem: &Problem,
) -> ProblemNotice {
    ProblemNotice::new(key, *theme, problem)
}

pub(crate) fn callout(theme: &ArtisanTheme, tone: Tone, value: impl Into<String>) -> Element {
    let words = TextRole::Label
        .text(theme, value.into())
        .gpui_source_color(tint(theme, tone).to_reference_text_color())
        .wrap(true)
        .build();
    Surface::Callout(tone).build(theme, vec![words])
}

pub(crate) fn input(theme: &ArtisanTheme, field: Element) -> Element {
    let carve = Carve::of(theme);
    UiBox::new()
        .height(36.0)
        .style(
            editor_ui::carved::well::sunken(&carve, RADIUS_MD)
                .flex_grow(1.0)
                .min_width(Length::Px(0.0))
                .align(AlignItems::CENTER)
                .padding_insets(aqua_core::Insets {
                    left: 6.0,
                    right: 6.0,
                    top: 0.0,
                    bottom: 0.0,
                }),
        )
        .child(
            UiBox::new()
                .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                .child(field)
                .build(),
        )
        .build()
}

pub(crate) fn form_block(theme: &ArtisanTheme, children: Vec<Element>) -> Element {
    let carve = Carve::of(theme);
    UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .style(
            Style::new()
                .gap(14.0)
                .background(carve.bg.to_paint())
                .padding_insets(aqua_core::Insets {
                    left: PAD_X,
                    right: PAD_X,
                    top: 18.0,
                    bottom: 18.0,
                }),
        )
        .children(children)
        .build()
}

pub(crate) fn field(theme: &ArtisanTheme, name: &str, input: Element) -> Element {
    stack_gap(6.0)
        .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
        .child(typed(theme, Type::sans(13.12, 20.0, 500), Ink::Muted, name))
        .child(input)
        .build()
}

pub(crate) fn inset_tag(theme: &ArtisanTheme, value: &str) -> Element {
    let carve = Carve::of(theme);
    UiBox::new()
        .height(22.0)
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .flex_shrink(0.0)
                .radius(Length::Px(editor_ui::carved::RADIUS_SM))
                .background(carve.bg.to_paint())
                .shadows(carve.inset())
                .padding_insets(aqua_core::Insets {
                    left: 8.0,
                    right: 8.0,
                    top: 0.0,
                    bottom: 0.0,
                }),
        )
        .child(
            Type::sans(11.52, 11.52, 500)
                .text(theme, carve.ink_dim, value.to_owned())
                .wrap(false)
                .build(),
        )
        .build()
}

pub(crate) fn code_pill(theme: &ArtisanTheme, value: &str) -> Element {
    let carve = Carve::of(theme);
    UiBox::new()
        .style(
            Style::new()
                .flex_shrink(0.0)
                .radius(Length::Px(5.0))
                .background(carve.bg.to_paint())
                .border_of(1.0, carve.line.to_paint())
                .padding_insets(aqua_core::Insets {
                    left: 7.0,
                    right: 7.0,
                    top: 1.0,
                    bottom: 1.0,
                }),
        )
        .child(
            Type::mono(13.12, 19.0, 400)
                .text(theme, carve.ink, value.to_owned())
                .wrap(false)
                .build(),
        )
        .build()
}

pub(crate) fn tile(mark: Element) -> Element {
    UiBox::new()
        .width(24.0)
        .height(24.0)
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .justify(JustifyContent::CENTER)
                .flex_shrink(0.0),
        )
        .child(mark)
        .build()
}

pub(crate) fn titled(theme: &ArtisanTheme, title: Element, sub: Option<&str>) -> Element {
    stack_gap(3.0)
        .child(title)
        .children(sub.map(|sub| meta(theme, sub)))
        .build()
}

pub(crate) fn raised(content: Element, radius: f32, theme: &ArtisanTheme) -> Element {
    UiBox::new()
        .style(
            Style::new()
                .radius(Length::Px(radius))
                .flex_shrink(0.0)
                .shadows(Carve::of(theme).rest()),
        )
        .child(content)
        .build()
}

pub(crate) fn about_hero(identity: Element, description: Element) -> Element {
    UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .style(
            Style::new()
                .gap(14.0)
                .align(AlignItems::START)
                .padding(Length::Px(PAD_X)),
        )
        .child(identity)
        .child(description)
        .build()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Edges {
    pub(crate) first: bool,
    pub(crate) last: bool,
}

pub(crate) fn licence(
    theme: &ArtisanTheme,
    key: String,
    name: &str,
    body: Option<&str>,
    edges: Edges,
    on_toggle: impl Fn() + 'static,
) -> Element {
    let carve = Carve::of(theme);
    let open = body.is_some();
    let rim = 1.0;
    let top = if edges.first { RADIUS_MD - rim } else { 0.0 };
    let bottom = if edges.last { RADIUS_MD - rim } else { 0.0 };
    let fill = editor_ui::kit::live::layer(editor_ui::kit::Hover::Neutral, theme, 0.0, false)
        .style(
            Style::new()
                .absolute_at(
                    Some(rim),
                    Some(if edges.first { rim } else { 0.0 }),
                    Some(rim),
                    Some(if edges.last { rim } else { 0.0 }),
                )
                .corners(aqua_core::CornerRadii {
                    top_left: top,
                    top_right: top,
                    bottom_right: bottom,
                    bottom_left: bottom,
                }),
        );
    let trigger = aqua_ui::element::Button::new()
        .width(Length::Pct(100.0))
        .height(56.0)
        .gap(16.0)
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .justify(JustifyContent::SPACE_BETWEEN)
                .padding_insets(aqua_core::Insets {
                    left: PAD_X,
                    right: PAD_X,
                    top: 0.0,
                    bottom: 0.0,
                }),
        )
        .child(label(theme, name))
        .child(editor_ui::carved::glyph::glyph(
            if open {
                Icon::ChevronUp
            } else {
                Icon::ChevronDown
            },
            15.0,
            if open { carve.ink } else { carve.ink_dim },
        ))
        .on_activate(on_toggle)
        .build()
        .with_key(key)
        .role(aqua_core::Role::Button)
        .aria_label(format!("{name} licence"))
        .aria_expanded(open)
        .cursor(aqua_core::CursorIcon::Pointer)
        .focus_visible_style(editor_ui::carved::button::focus(theme));
    let well = body.map(|body| {
        UiBox::new()
            .width(Length::Pct(100.0))
            .style(Style::new().padding_insets(aqua_core::Insets {
                left: PAD_X,
                right: PAD_X,
                top: 0.0,
                bottom: 16.0,
            }))
            .child(
                aqua_ui::element::Scroll::new()
                    .width(Length::Pct(100.0))
                    .style(
                        Style::new()
                            .max_height(Length::Px(220.0))
                            .radius(Length::Px(RADIUS_MD))
                            .background(carve.well.to_paint())
                            .shadows(carve.inset())
                            .padding_insets(aqua_core::Insets {
                                left: 16.0,
                                right: 16.0,
                                top: 14.0,
                                bottom: 14.0,
                            }),
                    )
                    .child(
                        Type::mono(12.48, 19.0, 400)
                            .text(theme, carve.ink_dim, body.to_owned())
                            .wrap(true)
                            .build(),
                    )
                    .build(),
            )
            .build()
    });
    editor_ui::kit::live::track(
        UiBox::new()
            .layout("column")
            .width(Length::Pct(100.0))
            .child(fill)
            .child(trigger)
            .children(well)
            .build(),
    )
}

pub(crate) fn hero(identity: Element, actions: Element) -> Element {
    UiBox::new()
        .width(Length::Pct(100.0))
        .style(
            Style::new()
                .gap(20.0)
                .align(AlignItems::CENTER)
                .justify(JustifyContent::SPACE_BETWEEN)
                .padding_insets(aqua_core::Insets {
                    left: PAD_X,
                    right: PAD_X,
                    top: 22.0,
                    bottom: 22.0,
                }),
        )
        .child(identity)
        .child(actions)
        .build()
}

pub(crate) fn overlap(width: f32, height: f32, layers: Vec<(f32, Element)>) -> Element {
    layers
        .into_iter()
        .fold(
            UiBox::new()
                .width(width)
                .height(height)
                .style(Style::new().flex_shrink(0.0)),
            |stack, (top, layer)| {
                stack.child(
                    UiBox::new()
                        .style(Style::new().absolute_at(Some(0.0), Some(top), None, None))
                        .child(layer)
                        .build(),
                )
            },
        )
        .build()
}

pub(crate) fn machine_row(
    tile: Option<Element>,
    text: Element,
    control: Option<Element>,
    padded: bool,
) -> Element {
    let left = UiBox::new()
        .style(
            Style::new()
                .gap(14.0)
                .align(AlignItems::CENTER)
                .min_width(Length::Px(0.0))
                .flex_shrink(1.0),
        )
        .children(tile)
        .child(text)
        .build();
    let mut row = UiBox::new().width(Length::Pct(100.0)).style(
        Style::new()
            .gap(16.0)
            .align(AlignItems::CENTER)
            .justify(JustifyContent::SPACE_BETWEEN)
            .padding_insets(aqua_core::Insets {
                left: PAD_X,
                right: PAD_X,
                top: if padded { 14.0 } else { 0.0 },
                bottom: if padded { 14.0 } else { 0.0 },
            }),
    );
    if !padded {
        row = row.height(ROW_HEIGHT);
    }
    row.child(left)
        .children(control.map(|control| {
            UiBox::new()
                .style(
                    Style::new()
                        .gap(8.0)
                        .align(AlignItems::CENTER)
                        .flex_shrink(0.0),
                )
                .child(control)
                .build()
        }))
        .build()
}
