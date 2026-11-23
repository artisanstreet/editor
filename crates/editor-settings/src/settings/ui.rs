use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Element},
    style::{Length, Style},
};

use editor_core::Problem;
use editor_ui::{
    kit::{
        Button, ButtonTone, Divider, ProblemNotice, RowItem, RowKind, Space, Spaced, Surface,
        TextRole, Tone, surface::tint,
    },
    theme::ArtisanTheme,
};

const CARD: Surface = Surface::Inset;
pub(crate) const CONTROL_WIDTH: f32 = 144.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ink {
    Plain,
    Muted,
    Success,
    Danger,
}

fn ink_color(theme: &ArtisanTheme, ink: Ink) -> editor_ui::theme::Oklch {
    match ink {
        Ink::Plain => theme.colors.text,
        Ink::Muted => theme.colors.text_muted,
        Ink::Success => theme.colors.green.solid,
        Ink::Danger => theme.colors.destructive_text,
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

pub(crate) fn markdown(theme: &ArtisanTheme, role: TextRole, ink: Ink, source: &str) -> Element {
    let color = ink_color(theme, ink).to_reference_text_color();
    let runs = editor_ui::inline_code_text::inline_runs(source);
    let spans = runs.styled_spans(&role.style(theme), color, theme);
    role.text(theme, runs.text)
        .gpui_source_color(color)
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

pub(crate) fn column() -> UiBox {
    UiBox::new().layout("column").spaced(Space::S20)
}

pub(crate) fn stack(space: Space) -> UiBox {
    UiBox::new().layout("column").spaced(space)
}

pub(crate) fn actions() -> UiBox {
    UiBox::new().spaced(Space::S8).style(
        Style::new()
            .align(AlignItems::CENTER)
            .justify(JustifyContent::END),
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

pub(crate) fn button(
    theme: &ArtisanTheme,
    id: impl Into<String>,
    label: impl Into<String>,
    tone: ButtonTone,
    disabled: bool,
    on_activate: impl Fn() + 'static,
) -> Element {
    Button::text(id.into(), *theme, tone, label)
        .disabled(disabled)
        .on_activate(move |_| on_activate())
        .build()
}

pub(crate) fn header(theme: &ArtisanTheme, title: &str, description: &str) -> Element {
    stack(Space::S8)
        .child(text(theme, TextRole::Heading, Ink::Plain, title).role(aqua_core::Role::Heading(1)))
        .child(text(theme, TextRole::Label, Ink::Muted, description))
        .build()
}

pub(crate) fn section(
    theme: &ArtisanTheme,
    title: &str,
    intro: Option<&str>,
    action: Option<Element>,
    body: Element,
) -> Element {
    let mut heading = stack(Space::S4)
        .style(Style::new().flex_grow(1.0).min_width(Length::Pct(0.0)))
        .child(text(theme, TextRole::Heading, Ink::Plain, title).role(aqua_core::Role::Heading(2)));
    if let Some(intro) = intro {
        heading = heading.child(text(theme, TextRole::Caption, Ink::Muted, intro));
    }
    let head = UiBox::new()
        .spaced(Space::S16)
        .style(
            Style::new()
                .align(AlignItems::END)
                .justify(JustifyContent::SPACE_BETWEEN),
        )
        .child(heading.build())
        .children(action);
    stack(Space::S8).child(head.build()).child(body).build()
}

pub(crate) fn card(theme: &ArtisanTheme, blocks: Vec<Element>) -> Element {
    let bleed = CARD.padding_px() - Space::S2.px();
    let children = blocks
        .into_iter()
        .enumerate()
        .flat_map(|(index, block)| {
            let rule = (index != 0).then(|| Divider::Rule.build(theme, bleed));
            rule.into_iter().chain(std::iter::once(block))
        })
        .collect();
    CARD.build(theme, children)
}

pub(crate) fn row(
    theme: &ArtisanTheme,
    title: impl Into<String>,
    description: impl Into<String>,
    control: Option<Element>,
) -> Element {
    let title = title.into();
    let description = description.into();
    if description.is_empty() {
        return UiBox::new()
            .height(56.0)
            .padded(Space::S8)
            .spaced(Space::S16)
            .style(
                Style::new()
                    .align(AlignItems::CENTER)
                    .justify(JustifyContent::SPACE_BETWEEN),
            )
            .child(grow(line(theme, TextRole::Label, Ink::Plain, title)))
            .children(control)
            .build();
    }
    let item = RowItem::new(
        format!("settings-row-{title}"),
        *theme,
        RowKind::Form,
        CARD,
        title,
    );
    let item = item.detail(description);
    match control {
        Some(control) => item.trailing(control),
        None => item,
    }
    .build()
}

pub(crate) fn block(content: Element) -> Element {
    UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .padded(RowKind::Form.pad().y)
        .child(content)
        .build()
}

pub(crate) fn note(theme: &ArtisanTheme, value: &str) -> Element {
    block(text(theme, TextRole::Label, Ink::Muted, value))
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
