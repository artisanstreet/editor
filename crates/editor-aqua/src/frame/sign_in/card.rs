use crate::frame::sign_in::model::{Feedback, Link, Provider, View, page_of};
use crate::frame::sign_in::{
    CODE_PAGE, EMAIL_PAGE, Event, PAGE, REVEAL, SWAP, SignIn, Slot, pattern,
};
use crate::ui::chrome::card::{
    POLAR_BLUE, POLAR_EDGE, POLAR_HAZE, POLAR_HIGHLIGHT, POLAR_INK, POLAR_NAVY, POLAR_SHADE, POLAR_SKY,
    hex,
};
use crate::ui::kit::surface::{self, Tone, tint};
use crate::ui::kit::{
    Divider, HitArea, IconSize, Inset, Key as KitKey, KeyFace, KeySize, Radius, Space, Spaced, Surface,
    TextRole,
};
use crate::ui::theme::{ArtisanTheme, Oklch};
use aqua_core::{CursorIcon, DisplayItem, ImageRef, Role, Transform2D, image::ImageFit};
use aqua_text::StyledSpan;
use aqua_ui::element::{Box as UiBox, Canvas, Element, Media};
use aqua_ui::input::{EventPhase, InputKind, InputResponse, RoutedInput};
use aqua_ui::style::{Length, Style};
use aqua_ui::{AlignItems, JustifyContent};
use editor_assets::AssetId;
use std::{rc::Rc, sync::Arc, time::Instant};

pub const WIDTH: f32 = 400.0;
const CARD: Surface = Surface::Card;
const INNER: Surface = Surface::Inset;
const FIELD: Surface = Surface::Field;
const KEY: KeySize = KeySize::Lg;
const CARD_GAP: Inset = Inset::Frame;
const PAGE_PAD: Space = Space::S12;
const WORDMARK_HEIGHT: f32 = 72.0;
const WORDMARK_WIDTH: f32 = WORDMARK_HEIGHT * 136.675 / 225.15;
const SLIDE: f32 = 8.0;
const PAGE_BLUR: f32 = 3.0;
const ICON_BLUR: f32 = 2.0;
const ICON_START: f32 = 0.25;
const FEEDBACK_SHIFT: f32 = 4.0;
const FEEDBACK_BLUR: f32 = 2.0;
const PATTERN_ALPHA: f32 = 0.02;

const GOOGLE: &str = include_str!("providers/google.svg");
const GITHUB: &str = include_str!("providers/github.svg");

pub struct Props<'a> {
    pub view: &'a View,
    pub sign_in: &'a SignIn,
    pub email: Element,
    pub code: Element,
    pub busy: bool,
    pub width: f32,
    pub now: Instant,
    pub emit: Rc<dyn Fn(Event)>,
}

#[must_use]
pub fn card(theme: &ArtisanTheme, props: Props<'_>) -> Element {
    let header = header(AlignItems::START, vec![wordmark(theme), artwork(props.sign_in.art())]);
    let inner = vec![pattern_layer(theme), pages(theme, props)];
    UiBox::new()
        .layout("column")
        .width(WIDTH)
        .child(framed(theme, header, inner))
        .build()
        .with_key("artisan-sign-in-card")
}

#[must_use]
pub fn header(align: AlignItems, children: Vec<Element>) -> Element {
    UiBox::new()
        .layout("row")
        .padded(Space::S16)
        .spaced(Space::S24)
        .style(
            Style::new()
                .align(align)
                .justify(JustifyContent::SPACE_BETWEEN),
        )
        .children(children)
        .build()
}

#[must_use]
pub fn framed(theme: &ArtisanTheme, header: Element, inner: Vec<Element>) -> Element {
    let stack = UiBox::new()
        .layout("column")
        .spaced(Space::from(CARD_GAP))
        .child(header)
        .child(INNER.build(theme, inner))
        .build();
    CARD.build(theme, vec![stack])
}

#[must_use]
pub fn body(content: Vec<Element>) -> UiBox {
    UiBox::new()
        .layout("column")
        .padded(PAGE_PAD)
        .children(content)
}

#[must_use]
pub fn wordmark(theme: &ArtisanTheme) -> Element {
    let source: Arc<str> = editor_assets::get(AssetId::ARTISAN_WORDMARK_STACKED).source.into();
    let tint = theme.colors.text.to_reference_paint();
    Canvas::new()
        .style(
            Style::new()
                .width(Length::Px(WORDMARK_WIDTH))
                .height(Length::Px(WORDMARK_HEIGHT))
                .flex_shrink(0.0),
        )
        .on_paint(move |bounds| {
            vec![DisplayItem::GpuiSvg {
                bounds,
                source: Arc::clone(&source),
                tint: Some(tint),
                opacity: 1.0,
            }]
        })
        .build()
        .with_key("artisan-sign-in-wordmark")
        .aria_label("Artisan")
}

fn artwork(art: Option<Arc<aqua::images::ImageHandle>>) -> Element {
    let (width, height) = crate::frame::sign_in::ART_BOX;
    let frame = UiBox::new().width(width).height(height).style(Style::new().absolute_at(
        None,
        None,
        Some(-CARD.padding_px()),
        Some(-CARD_GAP.px()),
    ));
    match art {
        Some(image) => {
            let lease: Arc<dyn std::any::Any + Send + Sync> = image.clone();
            frame
                .child(
                    Media::new(ImageRef::with_lease(image.id(), lease))
                        .width(width)
                        .height(height)
                        .fit(ImageFit::Fill)
                        .build(),
                )
                .build()
        }
        None => frame.build(),
    }
    .exclude_from_hit_testing()
}

#[expect(
    clippy::disallowed_methods,
    reason = "the sign-in pattern is a painted texture whose cells fade by strength"
)]
fn pattern_layer(theme: &ArtisanTheme) -> Element {
    let ink = theme.colors.text;
    Canvas::new()
        .style(
            Style::new()
                .absolute_at(Some(0.0), None, Some(0.0), Some(0.0))
                .height(Length::Pct(pattern::COVERAGE * 100.0)),
        )
        .on_paint(move |bounds| {
            pattern::cells(bounds.w, bounds.h)
                .into_iter()
                .map(|cell| DisplayItem::Rect {
                    bounds: aqua_core::Rect::from_edges(
                        bounds.x + cell.x,
                        bounds.y + cell.y,
                        (bounds.x + cell.x + pattern::CELL.0).min(bounds.x + bounds.w),
                        bounds.y + cell.y + pattern::CELL.1,
                    ),
                    radius: 0.0,
                    fill: ink.with_alpha(PATTERN_ALPHA * cell.strength).to_paint(),
                    border_width: 0.0,
                    border_color: aqua_core::Oklch::TRANSPARENT,
                    shadow: None,
                })
                .collect()
        })
        .build()
        .exclude_from_hit_testing()
}

fn pages(theme: &ArtisanTheme, props: Props<'_>) -> Element {
    let now = props.now;
    let progress = props.sign_in.motion.page.value(now, PAGE);
    let code_shown = matches!(page_of(props.view), crate::frame::sign_in::model::Page::Code);
    let feedback = crate::frame::sign_in::model::feedback_of(props.view);
    let (email_feedback, code_feedback) = match code_shown {
        true => (Feedback::None, feedback),
        false => (feedback, Feedback::None),
    };
    let busy_swap = Key {
        swap: props.sign_in.motion.busy.value(now, SWAP),
        spin: props.sign_in.motion.spin,
    };
    let email_reveal = props.sign_in.motion.email_feedback.value(now, REVEAL);
    let code_reveal = props.sign_in.motion.code_feedback.value(now, REVEAL);
    let email = page(
        EMAIL_PAGE,
        !code_shown,
        1.0 - progress,
        -SLIDE * progress,
        email_page(
            theme,
            props.email,
            busy_swap,
            props.busy,
            crate::frame::sign_in::model::cancellable(props.view),
            slot(props.sign_in, Slot::Email, email_reveal, &email_feedback),
            &props.emit,
        ),
    );
    let code = page(
        CODE_PAGE,
        code_shown,
        progress,
        SLIDE * (1.0 - progress),
        code_page(
            theme,
            props.code,
            busy_swap,
            props.busy,
            slot(props.sign_in, Slot::Code, code_reveal, &code_feedback),
            &props.emit,
        ),
    );
    let mut container = Style::new().clip(true);
    if let Some(height) = props.sign_in.height(now) {
        container = container.height(Length::Px(height));
    }
    UiBox::new()
        .layout("column")
        .style(container)
        .child(email)
        .child(code)
        .build()
}

fn page(key: &'static str, active: bool, opacity: f32, shift: f32, content: Element) -> Element {
    let position = match active {
        true => Style::new(),
        false => Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), None),
    };
    let blur = PAGE_BLUR * (1.0 - opacity);
    let mut style = position
        .opacity(opacity)
        .transform(Transform2D::translate(shift, 0.0));
    if blur > 0.01 {
        style = style.content_blur(blur);
    }
    let element = body(vec![content])
        .style(Style::new().flex_shrink(0.0))
        .style(style)
        .build()
        .with_key(key)
        .debug_selector(key);
    match active {
        true => element,
        false => element.exclude_from_hit_testing(),
    }
}

struct FeedbackSlot<'a> {
    last: &'a Feedback,
    current: &'a Feedback,
    reveal: f32,
    height: f32,
}

fn slot<'a>(sign_in: &'a SignIn, slot: Slot, reveal: f32, current: &'a Feedback) -> FeedbackSlot<'a> {
    FeedbackSlot {
        last: sign_in.last(slot),
        current,
        reveal,
        height: sign_in.feedback_height(slot),
    }
}

fn email_page(
    theme: &ArtisanTheme,
    editor: Element,
    key: Key,
    busy: bool,
    cancellable: bool,
    feedback: FeedbackSlot<'_>,
    emit: &Rc<dyn Fn(Event)>,
) -> Element {
    let submit = Rc::clone(emit);
    let note = match cancellable {
        true => cancel_text(theme, emit),
        false => legal(theme, emit),
    };
    let entry = UiBox::new()
        .layout("column")
        .spaced(Space::S12)
        .child(
            UiBox::new()
                .layout("column")
                .child(field(theme, editor, key, busy, Rc::new(move || submit(Event::SubmitEmail))))
                .child(feedback_line(theme, "email", feedback))
                .build(),
        )
        .child(
            Divider::Labelled("or".to_owned())
                .build(theme, 0.0)
                .role(Role::Separator),
        )
        .child(
            UiBox::new()
                .layout("row")
                .spaced(Space::S12)
                .children(Provider::ALL.map(|provider| provider_key(theme, provider, busy, emit)))
                .build(),
        )
        .build();
    UiBox::new()
        .layout("column")
        .spaced(Space::S32)
        .child(entry)
        .child(well(theme, note))
        .build()
}

fn code_page(
    theme: &ArtisanTheme,
    editor: Element,
    key: Key,
    busy: bool,
    feedback: FeedbackSlot<'_>,
    emit: &Rc<dyn Fn(Event)>,
) -> Element {
    let submit = Rc::clone(emit);
    let heading = UiBox::new()
        .layout("column")
        .spaced(Space::S2)
        .child(TextRole::Title.text(theme, "Two-step verification").wrap(true).build())
        .child(
            TextRole::Caption
                .text(theme, "Enter the 6-digit code from your authenticator app.")
                .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
                .wrap(true)
                .build(),
        )
        .build();
    UiBox::new()
        .layout("column")
        .spaced(Space::S16)
        .child(heading)
        .child(
            UiBox::new()
                .layout("column")
                .child(field(theme, editor, key, busy, Rc::new(move || submit(Event::SubmitCode))))
                .child(feedback_line(theme, "code", feedback))
                .build(),
        )
        .child(well(theme, cancel_text(theme, emit)))
        .build()
}

fn field(
    theme: &ArtisanTheme,
    editor: Element,
    key: Key,
    busy: bool,
    submit: Rc<dyn Fn()>,
) -> Element {
    keyed_field(theme, editor, continue_key(key, busy, submit))
}

#[must_use]
pub fn keyed_field(theme: &ArtisanTheme, content: Element, key: Element) -> Element {
    let content = UiBox::new()
        .layout("column")
        .style(
            Style::new()
                .min_height(Length::Px(KEY.edge() - 2.0 * FIELD.padding_px()))
                .justify(JustifyContent::CENTER),
        )
        .child(content)
        .build();
    UiBox::new()
        .layout("row")
        .style(Style::new().align(AlignItems::STRETCH))
        .child(
            UiBox::new()
                .layout("column")
                .style(
                    Radius::Field
                        .style()
                        .flex_grow(1.0)
                        .min_width(Length::Pct(0.0))
                        .clip(true),
                )
                .child(FIELD.build(theme, vec![content]))
                .build(),
        )
        .child(key)
        .build()
}

#[derive(Clone, Copy)]
struct Key {
    swap: f32,
    spin: u32,
}

fn continue_key(Key { swap, spin }: Key, busy: bool, submit: Rc<dyn Fn()>) -> Element {
    let arrow: Arc<str> = continue_arrow().into();
    let spinner: Arc<str> = continue_spinner(spin).into();
    let glyph = KEY.edge() * 2.0 / 3.0;
    let icon = |source: Arc<str>, visible: f32, key: &'static str| {
        let scale = (1.0 - ICON_START).mul_add(visible, ICON_START);
        let center = glyph / 2.0;
        let mut style = Style::new()
            .absolute_at(Some(0.0), Some(0.0), None, None)
            .opacity(visible)
            .transform(
                Transform2D::translate(-center, -center)
                    .then(Transform2D::scale(scale, scale))
                    .then(Transform2D::translate(center, center)),
            );
        if visible < 0.99 {
            style = style.content_blur(ICON_BLUR * (1.0 - visible));
        }
        Canvas::new()
            .style(
                style
                    .width(Length::Px(glyph))
                    .height(Length::Px(glyph)),
            )
            .on_paint(move |bounds| {
                vec![DisplayItem::GpuiSvg {
                    bounds,
                    source: Arc::clone(&source),
                    tint: None,
                    opacity: 1.0,
                }]
            })
            .build()
            .with_key(key)
    };
    let glyphs = UiBox::new()
        .width(glyph)
        .height(glyph)
        .child(icon(arrow, 1.0 - swap, "artisan-sign-in-arrow"))
        .child(icon(spinner, swap, "artisan-sign-in-spinner"))
        .build();
    field_key(
        "artisan-sign-in-continue",
        "Continue",
        KEY,
        match busy {
            true => CursorIcon::Progress,
            false => CursorIcon::Pointer,
        },
        glyphs,
        Rc::new(move || {
            if !busy {
                submit();
            }
        }),
    )
}

#[must_use]
pub const fn key_lip(size: KeySize) -> f32 {
    size.edge() * 8.0 / 45.0
}

#[must_use]
pub fn field_key(
    key: &'static str,
    label: &'static str,
    size: KeySize,
    cursor: CursorIcon,
    glyph: Element,
    activate: Rc<dyn Fn()>,
) -> Element {
    let surface: Arc<str> = continue_surface().into();
    let edge = size.edge();
    let surface_width = edge * 53.0 / 45.0;
    let surface_height = edge * 49.0 / 45.0;
    let face = Canvas::new()
        .style(
            Style::new()
                .absolute_at(None, Some(0.0), Some(0.0), None)
                .width(Length::Px(surface_width))
                .height(Length::Px(surface_height)),
        )
        .on_paint(move |bounds| {
            vec![DisplayItem::GpuiSvg {
                bounds,
                source: Arc::clone(&surface),
                tint: None,
                opacity: 1.0,
            }]
        })
        .build()
        .exclude_from_hit_testing();
    let area = HitArea::new(key, Role::Button, label)
        .child(
            UiBox::new()
                .style(
                    Style::new()
                        .width(Length::Pct(100.0))
                        .align(AlignItems::CENTER)
                        .justify(JustifyContent::CENTER),
                )
                .child(face)
                .child(glyph.exclude_from_hit_testing())
                .build(),
        )
        .on_activate(move || activate())
        .build()
        .cursor(cursor);
    UiBox::new()
        .width(edge)
        .height(edge)
        .style(Style::new().flex_shrink(0.0))
        .child(area)
        .build()
}

fn continue_surface() -> String {
    let highlight = POLAR_NAVY.animate_to(POLAR_HIGHLIGHT, 0.28);
    let shape = "M0 0H45A8 8 0 0 1 53 8V37A8 8 0 0 1 45 45H0A8 8 0 0 0 8 37V8A8 8 0 0 0 0 0Z";
    let face = "M0 2H45A6 6 0 0 1 51 8V37A6 6 0 0 1 45 43H0Z";
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 53 49"><defs><clipPath id="c"><path d="{shape}"/></clipPath><linearGradient id="r" gradientUnits="userSpaceOnUse" x1="0" y1="0" x2="0" y2="45"><stop offset="0" stop-color="{edge}"/><stop offset="1" stop-color="{shade}"/></linearGradient><linearGradient id="h" gradientUnits="userSpaceOnUse" x1="0" y1="0" x2="0" y2="45"><stop offset="0" stop-color="{hl}"/><stop offset="1" stop-color="{navy}"/></linearGradient><radialGradient id="f" gradientUnits="userSpaceOnUse" cx="25.5" cy="53.25" r="66.3" gradientTransform="translate(25.5 53.25) scale(1 0.866) translate(-25.5 -53.25)"><stop offset="0" stop-color="{haze}"/><stop offset="0.25" stop-color="{sky}"/><stop offset="0.58" stop-color="{blue}"/><stop offset="0.92" stop-color="{navy}"/></radialGradient><filter id="d" x="-20%" y="-20%" width="140%" height="140%"><feDropShadow dx="0" dy="1.875" stdDeviation="0.9375" flood-color="#000" flood-opacity="0.3"/></filter></defs><g filter="url(#d)"><path d="{shape}" fill="url(#r)"/><g clip-path="url(#c)"><path d="M0 1H45A7 7 0 0 1 52 8V37A7 7 0 0 1 45 44H0Z" fill="url(#h)"/><path d="{face}" fill="url(#f)"/></g></g></svg>"##,
        edge = hex(POLAR_EDGE),
        shade = hex(POLAR_SHADE),
        hl = hex(highlight),
        haze = hex(POLAR_HAZE),
        sky = hex(POLAR_SKY),
        blue = hex(POLAR_BLUE),
        navy = hex(POLAR_NAVY),
    )
}

fn continue_arrow() -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 30 30" fill="none" stroke="{ink}" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><path d="M6.5 15.5H23.5M18.5 10.5l5 5-5 5"/></svg>"#,
        ink = hex(POLAR_INK),
    )
}

fn continue_spinner(spin: u32) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 30 30" fill="none" stroke="{ink}" stroke-width="3" stroke-linecap="round"><circle cx="15" cy="15" r="8.5" stroke-opacity="0.18"/><path transform="rotate({angle} 15 15)" d="M15 6.5a8.5 8.5 0 0 1 8.5 8.5"/></svg>"#,
        ink = hex(POLAR_INK),
        angle = spin * 360 / crate::frame::sign_in::SPIN_STEPS,
    )
}

fn provider_key(theme: &ArtisanTheme, provider: Provider, busy: bool, emit: &Rc<dyn Fn(Event)>) -> Element {
    let pick = Rc::clone(emit);
    let source = match provider {
        Provider::Google => GOOGLE,
        Provider::GitHub => GITHUB,
    };
    let icon: Arc<str> = source.into();
    let tint = theme.colors.text_muted.to_reference_paint();
    let edge = IconSize::Md.px();
    let mark = Canvas::new()
        .style(
            Style::new()
                .width(Length::Px(edge))
                .height(Length::Px(edge))
                .flex_shrink(0.0),
        )
        .on_paint(move |bounds| {
            vec![DisplayItem::GpuiSvg {
                bounds,
                source: Arc::clone(&icon),
                tint: Some(tint),
                opacity: 1.0,
            }]
        })
        .build();
    UiBox::new()
        .style(Style::new().flex_grow(1.0).min_width(Length::Pct(0.0)))
        .child(
            KitKey::new(
                format!("artisan-sign-in-{}", provider.name()),
                *theme,
                format!("Continue with {}", provider.name()),
                KEY,
                KeyFace::Content(mark),
            )
            .wide()
            .on_activate(move |_| {
                if !busy {
                    pick(Event::Provider(provider));
                }
            })
            .build(),
        )
        .build()
}

fn feedback_line(theme: &ArtisanTheme, key: &'static str, feedback: FeedbackSlot<'_>) -> Element {
    let reveal = feedback.reveal;
    let shown = match feedback.current {
        Feedback::None => feedback.last,
        current => current,
    };
    let (message, color) = match shown {
        Feedback::None => (String::new(), theme.colors.text),
        Feedback::Error(message) => (message.clone(), tint(theme, Tone::Danger)),
        Feedback::Success(message) => (message.clone(), tint(theme, Tone::Success)),
    };
    let mut words = Style::new()
        .flex_shrink(0.0)
        .opacity(reveal)
        .transform(Transform2D::translate(0.0, -FEEDBACK_SHIFT * (1.0 - reveal)));
    if reveal < 0.99 {
        words = words.content_blur(FEEDBACK_BLUR * (1.0 - reveal));
    }
    let content = UiBox::new()
        .layout("column")
        .padded(FIELD.padding().unwrap_or(Space::S8))
        .style(words)
        .child(
            TextRole::Caption
                .text(theme, message)
                .gpui_source_color(color.to_reference_text_color())
                .wrap(true)
                .build(),
        )
        .build()
        .with_key(format!("artisan-sign-in-feedback-{key}"))
        .debug_selector(match key {
            "email" => crate::frame::sign_in::EMAIL_FEEDBACK,
            _ => crate::frame::sign_in::CODE_FEEDBACK,
        });
    UiBox::new()
        .layout("column")
        .style(
            Style::new()
                .clip(true)
                .height(Length::Px(reveal * feedback.height)),
        )
        .child(content)
        .build()
}

fn well(theme: &ArtisanTheme, content: Element) -> Element {
    FIELD.build(theme, vec![content])
}

fn legal(theme: &ArtisanTheme, emit: &Rc<dyn Fn(Event)>) -> Element {
    linked(
        theme,
        &[
            ("By using Artisan Street services, you agree to our ", None),
            ("Terms of Service", Some(Event::Open(Link::Terms))),
            (" and ", None),
            ("Privacy Policy", Some(Event::Open(Link::Privacy))),
            (". Click ", None),
            ("here", Some(Event::Open(Link::Recover))),
            (" to recover accounts.", None),
        ],
        Rc::clone(emit),
    )
}

fn cancel_text(theme: &ArtisanTheme, emit: &Rc<dyn Fn(Event)>) -> Element {
    linked(
        theme,
        &[
            ("Click ", None),
            ("here", Some(Event::Cancel)),
            (" if you want to cancel.", None),
        ],
        Rc::clone(emit),
    )
}

fn linked(theme: &ArtisanTheme, parts: &[(&str, Option<Event>)], emit: Rc<dyn Fn(Event)>) -> Element {
    let base = TextRole::Caption.style(theme);
    let mut underlined = base.clone();
    underlined.underline = true;
    let hsla = |color: Oklch| {
        let paint = color.to_reference_paint();
        [paint.h, paint.s, paint.l, paint.a]
    };
    let events: Vec<Event> = parts.iter().filter_map(|(_, event)| *event).collect();
    let mut index = 0_u64;
    let spans = parts
        .iter()
        .map(|(words, event)| match event {
            Some(_) => {
                index += 1;
                StyledSpan::new(*words, underlined.clone())
                    .gpui_color(hsla(theme.colors.link))
                    .link(index - 1)
            }
            None => StyledSpan::new(*words, base.clone()).gpui_color(hsla(theme.colors.text_muted)),
        })
        .collect();
    TextRole::Caption
        .text(theme, String::new())
        .spans(spans)
        .wrap(true)
        .build()
        .on_input(InputKind::TextRangeActivated, EventPhase::Bubble, move |routed| {
            if let RoutedInput::TextRangeActivated { id, .. } = &routed.input {
                if let Some(event) = usize::try_from(*id).ok().and_then(|at| events.get(at)) {
                    emit(*event);
                }
            }
            InputResponse::default()
        })
}

#[must_use]
pub fn screen(theme: &ArtisanTheme, titlebar: Element, card: Element) -> Element {
    UiBox::new()
        .layout("column")
        .style(
            surface::frame(theme)
                .width(Length::Pct(100.0))
                .height(Length::Pct(100.0)),
        )
        .style(TextRole::Label.inherit(theme))
        .child(titlebar)
        .child(
            UiBox::new()
                .layout("column")
                .style(
                    Style::new()
                        .flex_grow(1.0)
                        .min_height(Length::Pct(0.0))
                        .align(AlignItems::CENTER)
                        .justify(JustifyContent::CENTER),
                )
                .child(card)
                .child(UiBox::new().height(crate::ui::chrome::TITLEBAR_HEIGHT).build())
                .build(),
        )
        .build()
}
