use crate::frame::sidebar::{Command, Event};
use crate::frame::sign_in::card::{field_key, key_lip, keyed_field};
use crate::ui::chrome::card::POLAR_INK;
use crate::ui::kit::{HitArea, IconSize, KeySize, Radius, Space, Spaced, Surface};
use crate::ui::theme::{ArtisanTheme, Opacity};
use aqua_core::{
    CursorIcon, DisplayItem, InputEvent, Insets, Key as Pressed, NamedKey, Oklch, PointerButton,
    Role, Transform2D,
};
use aqua_ui::element::{Box as UiBox, Canvas, Element};
use aqua_ui::input::{EventPhase, InputKind, InputResponse, RoutedInput};
use aqua_ui::style::{Length, Style};
use aqua_ui::{AlignItems, JustifyContent};
use editor_assets::AssetId;
use std::rc::Rc;

pub const OVERLAY: &str = "artisan-frame-search-overlay";
pub const QUERY: &str = "sidebar-thread-search";
pub const RAIL_KEY: KeySize = KeySize::Md;
const BAR: KeySize = KeySize::Lg;
const BLUR: f32 = 4.0;
const RISE: f32 = 8.0;
const RESULTS_BLUR: f32 = 2.0;
const RESULTS_DELAY: f32 = 0.3;
const BAR_FADE: f32 = 0.4;

#[must_use]
pub fn bar(theme: &ArtisanTheme, editor: Element, emit: Rc<dyn Fn(Event)>) -> Element {
    let content = UiBox::new()
        .layout("row")
        .spaced(Space::S8)
        .style(Style::new().align(AlignItems::CENTER))
        .child(
            UiBox::new()
                .style(Style::new().flex_shrink(0.0))
                .child(IconSize::Md.glyph(
                    AssetId::TABLER_SEARCH,
                    theme.colors.text_muted.to_reference_paint(),
                ))
                .build()
                .exclude_from_hit_testing(),
        )
        .child(
            UiBox::new()
                .layout("column")
                .style(Style::new().flex_grow(1.0).min_width(Length::Pct(0.0)))
                .child(editor)
                .build(),
        )
        .build();
    keyed_field(theme, content, new_thread_key(emit))
}

fn new_thread_key(emit: Rc<dyn Fn(Event)>) -> Element {
    field_key(
        "artisan-sidebar-new-thread",
        "New thread",
        BAR,
        CursorIcon::Pointer,
        IconSize::Md.glyph(AssetId::TABLER_PENCIL_PLUS, POLAR_INK.to_reference_paint()),
        Rc::new(move || emit(Event::Navigate(Command::New))),
    )
}

#[must_use]
pub fn rail(theme: &ArtisanTheme, emit: &Rc<dyn Fn(Event)>) -> Element {
    let edge = RAIL_KEY.edge();
    let lip = key_lip(RAIL_KEY);
    let search = Rc::clone(emit);
    let create = Rc::clone(emit);
    let glyph = UiBox::new()
        .style(
            Style::new()
                .flex_grow(1.0)
                .align(AlignItems::CENTER)
                .justify(JustifyContent::CENTER)
                .padding_insets(Insets {
                    left: 0.0,
                    right: lip,
                    top: 0.0,
                    bottom: 0.0,
                }),
        )
        .child(IconSize::Md.glyph(
            AssetId::TABLER_SEARCH,
            theme.colors.text_muted.to_reference_paint(),
        ))
        .build()
        .exclude_from_hit_testing();
    let field = UiBox::new()
        .width(edge + lip)
        .height(edge)
        .style(Style::new().flex_shrink(0.0))
        .child(
            HitArea::new(
                "artisan-sidebar-rail-search",
                Role::Button,
                "Search threads",
            )
            .child(Surface::Field.build(theme, vec![glyph]))
            .on_activate(move || search(Event::Search(true)))
            .build()
            .cursor(CursorIcon::Pointer),
        )
        .build();
    UiBox::new()
        .layout("row")
        .child(field)
        .child(field_key(
            "artisan-sidebar-rail-new-thread",
            "New thread",
            RAIL_KEY,
            CursorIcon::Pointer,
            IconSize::Md.glyph(AssetId::TABLER_PENCIL_PLUS, POLAR_INK.to_reference_paint()),
            Rc::new(move || create(Event::Navigate(Command::New))),
        ))
        .build()
}

#[must_use]
pub const fn rail_width() -> f32 {
    2.0 * RAIL_KEY.edge() + key_lip(RAIL_KEY)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub bottom: f32,
}

pub struct Overlay {
    pub progress: f32,
    pub anchor: Anchor,
    pub bar: Option<Element>,
    pub results: Option<Element>,
    pub emit: Rc<dyn Fn(Event)>,
}

#[must_use]
pub fn overlay(theme: &ArtisanTheme, overlay: Overlay) -> Element {
    let root = UiBox::new()
        .layout("column")
        .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)));
    let Overlay {
        progress,
        anchor,
        bar,
        results,
        emit,
    } = overlay;
    if bar.is_none() {
        return root.build().with_key(OVERLAY).exclude_from_hit_testing();
    }
    let height = BAR.edge();
    let top = (RAIL_KEY.edge() - height).mul_add(0.5, anchor.top);
    let reveal = ((progress - RESULTS_DELAY) / (1.0 - RESULTS_DELAY)).clamp(0.0, 1.0);
    let fill = (progress / BAR_FADE).min(1.0);
    let field = UiBox::new()
        .width((anchor.width - rail_width()).mul_add(progress, rail_width()))
        .height(height)
        .style(
            Style::new()
                .absolute_at(Some(anchor.left), Some(top), None, None)
                .radius(Length::Px(Radius::Field.px()))
                .clip(true),
        )
        .child(
            UiBox::new()
                .layout("column")
                .width(anchor.width)
                .style(
                    Style::new()
                        .absolute_at(Some(0.0), Some(0.0), None, None)
                        .opacity(fill),
                )
                .children(bar)
                .build(),
        )
        .build();
    let mut list = Style::new()
        .absolute_at(
            Some(anchor.left),
            Some(top + height + Space::S16.px()),
            None,
            Some(anchor.bottom),
        )
        .opacity(reveal)
        .transform(Transform2D::translate(0.0, RISE * (1.0 - reveal)));
    if reveal < 0.99 {
        list = list.content_blur(RESULTS_BLUR * (1.0 - reveal));
    }
    let list = UiBox::new()
        .layout("column")
        .width(anchor.width)
        .style(list)
        .child(
            Surface::Card
                .build(
                    theme,
                    vec![
                        UiBox::new()
                            .layout("column")
                            .style(Style::new().flex_grow(1.0).min_height(Length::Pct(0.0)))
                            .children(results)
                            .build(),
                    ],
                )
                .style(Style::new().flex_grow(1.0).min_height(Length::Pct(0.0))),
        )
        .build();
    let escape = Rc::clone(&emit);
    root.child(scrim(theme, progress, emit))
        .child(list)
        .child(field)
        .build()
        .with_key(OVERLAY)
        .on_input(
            InputKind::KeyDown,
            EventPhase::Bubble,
            move |event| match &event.input {
                RoutedInput::Native(InputEvent::KeyDown(key))
                    if key.key == Pressed::Named(NamedKey::Escape) =>
                {
                    escape(Event::Search(false));
                    InputResponse::handled()
                }
                _ => InputResponse::default(),
            },
        )
}

fn scrim(theme: &ArtisanTheme, progress: f32, emit: Rc<dyn Fn(Event)>) -> Element {
    let shade = theme.colors.black.at(Opacity::SettingsBackdrop).to_paint();
    let shade = Oklch {
        alpha: shade.alpha * progress,
        ..shade
    };
    let radius = BLUR * progress;
    Canvas::new()
        .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
        .on_paint(move |bounds| {
            vec![
                DisplayItem::BackdropBlur { bounds, radius },
                DisplayItem::Rect {
                    bounds,
                    radius: 0.0,
                    fill: shade,
                    border_width: 0.0,
                    border_color: Oklch::TRANSPARENT,
                    shadow: None,
                },
            ]
        })
        .build()
        .on_input(InputKind::PointerDown, EventPhase::Bubble, move |event| {
            if matches!(
                &event.input,
                RoutedInput::Native(InputEvent::PointerDown(_, PointerButton::Primary))
            ) {
                emit(Event::Search(false));
                return InputResponse::handled();
            }
            InputResponse::default()
        })
}
