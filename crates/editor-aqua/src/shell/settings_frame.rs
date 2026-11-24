#![expect(
    clippy::disallowed_methods,
    reason = "the Settings frame follows the carved prototype's geometry, which has no kit tokens"
)]

use crate::shell::layout::{self, Size};
use crate::ui::carved::face::outline_layer;
use crate::ui::carved::glyph::{Icon, glyph};
use crate::ui::carved::{Carve, RADIUS_CARD, RADIUS_SHELL, RADIUS_SM, Type};
use crate::ui::chrome::card::hex;
use crate::ui::theme::ArtisanTheme;
use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Element},
    style::{Length, Style},
};


pub fn settings_dialog(
    theme: &ArtisanTheme,
    viewport: Size,
    sidebar: Element,
    page: Element,
    dismiss: std::rc::Rc<dyn Fn()>,
) -> crate::ui::dialog::Dialog {
    let dimensions = layout::settings_layout(viewport);
    let panel = settings_panel(theme, viewport, sidebar, page, &dismiss);
    crate::ui::dialog::Dialog::new("settings-dialog", *theme, "Settings", panel)
        .bare()
        .soft_backdrop()
        .max_width(dimensions.size.width)
        .on_dismiss(move |_| dismiss())
}

pub fn settings_panel(
    theme: &ArtisanTheme,
    viewport: Size,
    sidebar: Element,
    page: Element,
    dismiss: &std::rc::Rc<dyn Fn()>,
) -> Element {
    settings_panel_with_title(
        theme,
        viewport,
        sidebar,
        page,
        UiBox::new().build(),
        dismiss,
    )
}

pub(crate) fn settings_rail(theme: &ArtisanTheme, search: Element, navigation: Element) -> Element {
    let carve = Carve::of(theme);
    let title = Type::heading(22.0, 26.4, 500)
        .text(theme, carve.ink, "Settings")
        .wrap(false)
        .build()
        .role(aqua_core::Role::Heading(1));
    let brand = UiBox::new()
        .style(Style::new().padding_insets(aqua_core::Insets {
            left: 10.0,
            right: 8.0,
            top: 8.0,
            bottom: 4.0,
        }))
        .child(title)
        .build();
    let field = UiBox::new()
        .width(Length::Pct(100.0))
        .height(36.0)
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .radius(Length::Px(RADIUS_SM))
                .background(carve.bg.to_paint())
                .shadows(carve.rest())
                .padding_insets(aqua_core::Insets {
                    left: 28.0,
                    right: 6.0,
                    top: 0.0,
                    bottom: 1.0,
                }),
        )
        .child(outline_layer(&carve, RADIUS_SM))
        .child(
            UiBox::new()
                .style(Style::new().absolute_at(Some(11.0), Some(10.5), None, None))
                .child(glyph(Icon::Search, 15.0, carve.ink_dim))
                .build()
                .exclude_from_hit_testing(),
        )
        .child(
            UiBox::new()
                .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                .child(search)
                .build(),
        )
        .build();
    let head = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .style(Style::new().gap(12.0).padding_insets(aqua_core::Insets {
            left: 16.0,
            right: 0.0,
            top: 0.0,
            bottom: 0.0,
        }))
        .child(brand)
        .child(field)
        .build();
    UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .style(Style::new().gap(12.0).padding_insets(aqua_core::Insets {
            left: 0.0,
            right: 12.0,
            top: 6.0,
            bottom: 6.0,
        }))
        .child(head)
        .child(
            UiBox::new()
                .style(Style::new().flex_grow(1.0).min_height(Length::Pct(0.0)))
                .child(navigation)
                .build(),
        )
        .build()
}

const LIP_HEIGHT: f32 = 34.0;
const STRIP_HEIGHT: f32 = 38.0;
const FILLET: f32 = 10.0;

fn fillet(carve: &Carve) -> Element {
    let source = {
        let (bg, line) = (hex(carve.bg), hex(carve.line));
        let circle = |r: f32| format!("M{} 0A{r} {r} 0 1 0 {} 0A{r} {r} 0 1 0 {} 0Z", FILLET - r, FILLET + r, FILLET - r);
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><defs><clipPath id="s"><rect width="10" height="10"/></clipPath></defs><path clip-path="url(#s)" fill-rule="evenodd" fill="{bg}" d="M0 0H10V10H0Z {}"/><path clip-path="url(#s)" fill-rule="evenodd" fill="{line}" d="{} {}"/></svg>"#,
            circle(10.2),
            circle(10.2),
            circle(9.2),
        )
    };
    let source: std::sync::Arc<str> = source.into();
    aqua_ui::element::Canvas::new()
        .width(FILLET)
        .height(FILLET)
        .style(Style::new().absolute_at(None, None, Some(-FILLET), Some(0.0)))
        .scroll_stable()
        .on_paint(move |bounds| {
            vec![aqua_core::DisplayItem::GpuiSvg {
                bounds,
                source: std::sync::Arc::clone(&source),
                tint: None,
                opacity: 1.0,
            }]
        })
        .build()
        .exclude_from_hit_testing()
}

pub(crate) fn settings_title(
    theme: &ArtisanTheme,
    page: &str,
    name: Option<&str>,
    account: Option<&crate::ui::chrome::account::Account>,
) -> Element {
    let carve = Carve::of(theme);
    let words = |text: &str, color| {
        Type::sans(14.0, 21.0, 400)
            .text(theme, color, text.to_owned())
            .wrap(false)
            .build()
    };
    let scope = name.map(|name| {
        let avatar = crate::ui::carved::avatar::avatar(
            theme,
            account,
            name,
            crate::ui::carved::avatar::Avatar {
                edge: 18.0,
                radius: 9.0,
                initial: Type::sans(10.0, 15.0, 600).untracked(),
                ring: true,
            },
        );
        UiBox::new()
            .style(Style::new().gap(7.0).align(AlignItems::CENTER))
            .child(avatar)
            .child(words(name, carve.ink_dim))
            .build()
    });
    let separator = name.map(|_| words("/", carve.ink_faint));
    UiBox::new()
        .height(LIP_HEIGHT + 1.0)
        .style(
            Style::new()
                .gap(8.0)
                .align(AlignItems::CENTER)
                .flex_shrink(0.0)
                .padding_insets(aqua_core::Insets {
                    left: 14.0,
                    right: 16.0,
                    top: 0.0,
                    bottom: 1.0,
                })
                .background(carve.bg.to_paint())
                .corners(aqua_core::CornerRadii {
                    top_left: 10.0,
                    top_right: 10.0,
                    bottom_right: 0.0,
                    bottom_left: 0.0,
                })
                .border_edges(
                    aqua_core::Insets {
                        left: 1.0,
                        right: 1.0,
                        top: 1.0,
                        bottom: 0.0,
                    },
                    carve.line.to_paint(),
                ),
        )
        .children(scope)
        .children(separator)
        .child(words(page, carve.ink).role(aqua_core::Role::Heading(1)))
        .child(fillet(&carve))
        .build()
}

fn strip(theme: &ArtisanTheme, title: Element, dismiss: &std::rc::Rc<dyn Fn()>) -> Element {
    let close = std::rc::Rc::clone(dismiss);
    let close = crate::ui::carved::button::CarvedButton::new(
        "settings-close",
        *theme,
        crate::ui::carved::button::Tone::Quiet,
        crate::ui::carved::button::Content::Icon(Icon::Close, "Close settings".to_owned()),
    )
    .size(34.0, 30.0)
    .on_activate(move |_| close())
    .build()
    .debug_selector("settings-close");
    UiBox::new()
        .width(Length::Pct(100.0))
        .height(STRIP_HEIGHT)
        .style(
            Style::new()
                .flex_shrink(0.0)
                .align(AlignItems::END)
                .justify(JustifyContent::SPACE_BETWEEN),
        )
        .child(
            UiBox::new()
                .style(Style::new().margin_insets(aqua_core::Insets {
                    left: 0.0,
                    right: 0.0,
                    top: 0.0,
                    bottom: -1.0,
                }))
                .child(title)
                .build(),
        )
        .child(
            UiBox::new()
                .style(Style::new().padding_insets(aqua_core::Insets {
                    left: 0.0,
                    right: 0.0,
                    top: 0.0,
                    bottom: 4.0,
                }))
                .child(close)
                .build(),
        )
        .build()
}

pub(crate) fn settings_panel_with_title(
    theme: &ArtisanTheme,
    viewport: Size,
    sidebar: Element,
    page: Element,
    title: Element,
    dismiss: &std::rc::Rc<dyn Fn()>,
) -> Element {
    let carve = Carve::of(theme);
    let dimensions = layout::settings_layout(viewport);
    let card = UiBox::new()
        .style(
            Style::new()
                .absolute_at(Some(0.0), Some(STRIP_HEIGHT), Some(0.0), Some(0.0))
                .background(carve.bg.to_paint())
                .border_of(1.0, carve.line.to_paint())
                .corners(aqua_core::CornerRadii {
                    top_left: 0.0,
                    top_right: RADIUS_CARD,
                    bottom_right: RADIUS_CARD,
                    bottom_left: RADIUS_CARD,
                })
                .shadows(carve.rest()),
        )
        .clip(true)
        .child(page)
        .build();
    let strip = strip(theme, title, dismiss);
    UiBox::new()
        .layout("row")
        .width(Length::Pct(100.0))
        .height(dimensions.size.height)
        .clip(true)
        .style(
            Style::new()
                .radius(Length::Px(RADIUS_SHELL))
                .background(carve.frame.to_paint())
                .border_of(1.0, carve.line.to_paint())
                .shadows(carve.overlay())
                .padding_insets(aqua_core::Insets {
                    left: 0.0,
                    right: 10.0,
                    top: 6.0,
                    bottom: 10.0,
                }),
        )
        .child(
            UiBox::new()
                .width(dimensions.navigation_width)
                .height(Length::Pct(100.0))
                .style(Style::new().flex_shrink(0.0))
                .child(sidebar)
                .build(),
        )
        .child(
            UiBox::new()
                .layout("column")
                .height(Length::Pct(100.0))
                .style(
                    Style::new()
                        .flex_grow(1.0)
                        .min_width(Length::Pct(0.0))
                        .min_height(Length::Pct(0.0)),
                )
                .child(card)
                .child(strip)
                .build(),
        )
        .build()
}
