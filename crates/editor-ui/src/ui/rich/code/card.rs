use crate::ui::kit::{IconButtonSize, Space, Spaced, Surface, TextRole};
use crate::ui::rich::narrative::callout::text;
use crate::ui::theme::ArtisanTheme;
use aqua_ui::AlignItems;
use aqua_ui::element::{Element, ui};
use aqua_ui::style::{Length, Style};

#[derive(Clone, Copy)]
pub(crate) enum CopyPlacement {
    TopRight,
    CenterRight,
}

pub(crate) fn envelope(
    theme: &ArtisanTheme,
    title: Option<&str>,
    subtitle: Option<&str>,
    caption: Option<&str>,
    content: Element,
) -> Element {
    let mut body = ui::Box::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .spaced(Space::S12);
    if title.is_some() || subtitle.is_some() {
        let mut heading = ui::Box::new().layout("column").spaced(Space::S2);
        if let Some(title) = title {
            heading = heading.child(text(theme, title, TextRole::Title, theme.colors.text));
        }
        if let Some(subtitle) = subtitle {
            heading = heading.child(text(
                theme,
                subtitle,
                TextRole::Label,
                theme.colors.text_muted,
            ));
        }
        body = body.child(heading.build());
    }
    body = body.child(content);
    if let Some(caption) = caption {
        body = body.child(text(
            theme,
            caption,
            TextRole::Caption,
            theme.colors.text_muted,
        ));
    }
    body.build()
}

fn copy_slot(action: Element, placement: CopyPlacement) -> Element {
    let align = match placement {
        CopyPlacement::TopRight => AlignItems::START,
        CopyPlacement::CenterRight => AlignItems::CENTER,
    };
    ui::Box::new()
        .layout("row")
        .style(Style::new().flex_shrink(0.0).align(align))
        .child(action)
        .build()
}

pub(crate) fn render(
    theme: &ArtisanTheme,
    content: Element,
    copy: Option<(Element, CopyPlacement)>,
    balanced: bool,
) -> Element {
    let body = ui::Box::new()
        .style(
            Style::new()
                .flex_grow(1.0)
                .flex_shrink(1.0)
                .min_width(Length::Px(0.0))
                .gpui_source_text_color(theme.colors.text.to_reference_text_color()),
        )
        .child(content)
        .build();
    let mut row = ui::Box::new()
        .layout("row")
        .spaced(Space::S8)
        .width(Length::Pct(100.0))
        .style(Style::new().min_width(Length::Px(0.0)));
    match copy {
        Some((action, placement)) => {
            let slot = copy_slot(action, placement);
            if balanced {
                row = row.child(
                    ui::Box::new()
                        .width(IconButtonSize::Md.edge())
                        .style(Style::new().flex_shrink(0.0))
                        .build(),
                );
            }
            row = row.child(body).child(slot);
        }
        None => row = row.child(body),
    }
    Surface::Card.build(theme, vec![row.build()]).style(
        Style::new()
            .width(Length::Pct(100.0))
            .min_width(Length::Px(0.0)),
    )
}
