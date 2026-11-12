use crate::ui::chrome::card::{
    self, POLAR_BLUE, POLAR_EDGE, POLAR_HAZE, POLAR_HIGHLIGHT, POLAR_INK, POLAR_NAVY, POLAR_SKY, Surfaces as Palette, hex,
    outlined, radial_glow, rest_shadows,
};
use crate::frame::sign_in::model::{Feedback, Link, Provider, View, page_of};
use crate::frame::sign_in::{
    CODE_PAGE, EMAIL_PAGE, Event, PAGE, REVEAL, SWAP, SignIn, Slot, pattern,
};
use crate::ui::theme::{ArtisanTheme, Oklch};
use aqua::{Aqua, ShadowLayer, Style, pct, px};
use aqua_core::{
    CursorIcon, DisplayItem, GpuiBackground, ImageRef, Insets, Role, Transform2D, image::ImageFit,
};
use aqua_text::{FontFamily, FontWeight, StyledSpan, TextStyle};
use aqua_ui::element::{Element, Media};
use aqua_ui::input::{EventPhase, InputKind, InputResponse, RoutedInput};
use aqua_ui::{AlignItems, AlignSelf, JustifyContent};
use editor_assets::AssetId;
use std::{rc::Rc, sync::Arc, time::Instant};

pub const WIDTH: f32 = 400.0;
const INSET: f32 = 8.0;
const OUTER_RADIUS: f32 = card::RADIUS;
const FIELD_INSET: f32 = 16.0;
const FIELD_RADIUS: f32 = 0.4 * 16.0;
const FIELD_HEIGHT: f32 = 36.0;
const HEADER_PADDING: f32 = 24.0;
const WORDMARK_HEIGHT: f32 = 72.0;
const WORDMARK_WIDTH: f32 = WORDMARK_HEIGHT * 136.675 / 225.15;
const SLIDE: f32 = 8.0;
const PAGE_BLUR: f32 = 3.0;
const ICON_BLUR: f32 = 2.0;
const ICON_START: f32 = 0.25;
const FEEDBACK_SHIFT: f32 = 4.0;
const FEEDBACK_BLUR: f32 = 2.0;

const BLACK: Oklch = Oklch::new(0.0, 0.0, 0.0);
const WHITE: Oklch = Oklch::new(1.0, 0.0, 0.0);
const LINK: Oklch = Oklch::new(0.676, 0.075, 250.22);
const ERROR: Oklch = Oklch::new(0.66, 0.2, 22.0);
const SUCCESS: Oklch = Oklch::new(0.74, 0.17, 152.0);

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
    let palette = Palette::of(theme);
    let art = props.sign_in.art();
    let header = Aqua::Box()
        .layout("row")
        .styles([Aqua::Style()
            .padding(px(HEADER_PADDING))
            .gap(HEADER_PADDING)
            .align(AlignItems::START)
            .justify(JustifyContent::SPACE_BETWEEN)])
        .child(wordmark(palette.ink))
        .child(artwork(art))
        .build();
    let inner = card::inset(theme, vec![pattern_layer(&palette), pages(theme, &palette, props)]);
    let stack = Aqua::Box()
        .layout("column")
        .styles([Aqua::Style().flex_grow(1.0)])
        .child(header)
        .child(
            Aqua::Box()
                .layout("column")
                .styles([Aqua::Style().margin_insets(Insets {
                    left: INSET,
                    right: INSET,
                    bottom: INSET,
                    ..Insets::default()
                })])
                .child(inner)
                .build(),
        )
        .build();
    Aqua::Box()
        .layout("column")
        .styles([Aqua::Style().width(px(WIDTH))])
        .child(outlined(palette.ink, palette.bg, OUTER_RADIUS, rest_shadows(), vec![stack]))
        .build()
        .with_key("artisan-sign-in-card")
}

fn wordmark(ink: Oklch) -> Element {
    let source: Arc<str> = editor_assets::get(AssetId::ARTISAN_WORDMARK_STACKED).source.into();
    let tint = ink.to_reference_paint();
    Aqua::Canvas()
        .styles([Aqua::Style()
            .width(px(WORDMARK_WIDTH))
            .height(px(WORDMARK_HEIGHT))
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
        .with_key("artisan-sign-in-wordmark")
        .aria_label("Artisan")
}

fn artwork(art: Option<Arc<aqua::images::ImageHandle>>) -> Element {
    let frame = Aqua::Box().styles([Aqua::Style()
        .width(px(crate::frame::sign_in::ART_BOX.0))
        .height(px(crate::frame::sign_in::ART_BOX.1))
        .flex_shrink(0.0)
        .align_self(AlignSelf::END)
        .margin_insets(Insets {
            right: -HEADER_PADDING,
            bottom: -HEADER_PADDING,
            ..Insets::default()
        })]);
    match art {
        Some(image) => {
            let lease: Arc<dyn std::any::Any + Send + Sync> = image.clone();
            frame
                .child(
                    Media::new(ImageRef::with_lease(image.id(), lease))
                        .width(crate::frame::sign_in::ART_BOX.0)
                        .height(crate::frame::sign_in::ART_BOX.1)
                        .fit(ImageFit::Fill)
                        .build(),
                )
                .build()
        }
        None => frame.build(),
    }
    .exclude_from_hit_testing()
}

fn pattern_layer(palette: &Palette) -> Element {
    let tint = palette.ink.with_alpha(0.02);
    Aqua::Canvas()
        .styles([Style::new()
            .absolute_at(Some(0.0), None, Some(0.0), Some(0.0))
            .height(pct(pattern::COVERAGE * 100.0))])
        .on_paint(move |bounds| {
            pattern::cells(bounds.w, bounds.h)
                .into_iter()
                .map(|cell| DisplayItem::GpuiBackground {
                    bounds: aqua_core::Rect::from_edges(
                        bounds.x + cell.x,
                        bounds.y + cell.y,
                        (bounds.x + cell.x + pattern::CELL.0).min(bounds.x + bounds.w),
                        bounds.y + cell.y + pattern::CELL.1,
                    ),
                    corners: aqua_core::geometry::CornerRadii::uniform(0.0),
                    background: GpuiBackground::Solid(
                        tint.with_alpha(tint.alpha() * cell.strength).to_reference_paint(),
                    ),
                })
                .collect()
        })
        .build()
        .exclude_from_hit_testing()
}

fn pages(theme: &ArtisanTheme, palette: &Palette, props: Props<'_>) -> Element {
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
            palette,
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
            palette,
            props.code,
            busy_swap,
            props.busy,
            slot(props.sign_in, Slot::Code, code_reveal, &code_feedback),
            &props.emit,
        ),
    );
    let mut container = Aqua::Style().clip(true);
    if let Some(height) = props.sign_in.height(now) {
        container = container.height(px(height));
    }
    Aqua::Box()
        .layout("column")
        .styles([container])
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
    let element = Aqua::Box()
        .layout("column")
        .styles([Aqua::Style().flex_shrink(0.0).padding(px(FIELD_INSET)), style])
        .child(content)
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
    palette: &Palette,
    editor: Element,
    key: Key,
    busy: bool,
    cancellable: bool,
    feedback: FeedbackSlot<'_>,
    emit: &Rc<dyn Fn(Event)>,
) -> Element {
    let submit = Rc::clone(emit);
    let note = match cancellable {
        true => cancel_text(theme, palette, emit),
        false => legal(theme, palette, emit),
    };
    let mut column = Aqua::Box()
        .layout("column")
        .child(field(palette, editor, key, busy, Rc::new(move || submit(Event::SubmitEmail))))
        .child(feedback_line(theme, palette, "email", feedback))
        .child(divider(theme, palette))
        .child(
            Aqua::Box()
                .layout("row")
                .styles([Aqua::Style().gap(12.0)])
                .children(Provider::ALL.map(|provider| provider_key(palette, provider, busy, emit)))
                .build(),
        );
    column = column.child(
        Aqua::Box()
            .layout("column")
            .styles([Aqua::Style().margin_insets(Insets {
                top: 48.0,
                ..Insets::default()
            })])
            .child(well(palette, note))
            .build(),
    );
    column.build()
}

fn code_page(
    theme: &ArtisanTheme,
    palette: &Palette,
    editor: Element,
    key: Key,
    busy: bool,
    feedback: FeedbackSlot<'_>,
    emit: &Rc<dyn Fn(Event)>,
) -> Element {
    let submit = Rc::clone(emit);
    let heading = Aqua::Box()
        .layout("column")
        .styles([Aqua::Style().gap(2.0).margin_insets(Insets {
            bottom: 16.0,
            ..Insets::default()
        })])
        .child(text(theme, "Two-step verification", 15.0, 20.0, palette.ink, FontWeight(600)))
        .child(text(
            theme,
            "Enter the 6-digit code from your authenticator app.",
            12.0,
            18.0,
            palette.ink_dim,
            FontWeight::NORMAL,
        ))
        .build();
    let cancel = Aqua::Box()
        .layout("column")
        .styles([Aqua::Style().margin_insets(Insets {
            top: 16.0,
            ..Insets::default()
        })])
        .child(well(palette, cancel_text(theme, palette, emit)))
        .build();
    Aqua::Box()
        .layout("column")
        .child(heading)
        .child(field(palette, editor, key, busy, Rc::new(move || submit(Event::SubmitCode))))
        .child(feedback_line(theme, palette, "code", feedback))
        .child(cancel)
        .build()
}

fn field(
    palette: &Palette,
    editor: Element,
    key: Key,
    busy: bool,
    submit: Rc<dyn Fn()>,
) -> Element {
    keyed_field(palette, editor, continue_key(key, busy, submit))
}

#[must_use]
pub fn keyed_field(palette: &Palette, content: Element, key: Element) -> Element {
    let well = outlined(
        palette.ink,
        palette.bg,
        FIELD_RADIUS,
        Vec::new(),
        vec![
            Aqua::Box()
                .styles([Aqua::Style()
                    .absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                    .radius(px(FIELD_RADIUS))
                    .shadows(vec![
                        layer(7.0, 0.0, 9.0, -5.0, true, WHITE.with_alpha(0.1)),
                        layer(-7.0, 0.0, 9.0, -5.0, true, WHITE.with_alpha(0.1)),
                    ])])
                .build()
                .exclude_from_hit_testing(),
            Aqua::Box()
                .layout("column")
                .styles([Aqua::Style()
                    .min_height(px(FIELD_HEIGHT))
                    .justify(JustifyContent::CENTER)
                    .padding_insets(Insets {
                        left: 12.0,
                        right: 12.0 + FIELD_HEIGHT * 53.0 / 45.0 - FIELD_HEIGHT,
                        top: 4.0,
                        bottom: 4.0,
                    })])
                .child(content)
                .build(),
        ],
    );
    Aqua::Box()
        .layout("row")
        .styles([Aqua::Style().align(AlignItems::STRETCH)])
        .child(
            Aqua::Box()
                .layout("column")
                .styles([Aqua::Style()
                    .flex_grow(1.0)
                    .min_width(px(0.0))
                    .clip(true)
                    .radius(px(FIELD_RADIUS))])
                .child(well)
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
    let glyph = KEY_GLYPH;
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
        Aqua::Canvas()
            .styles([Aqua::Style().width(px(glyph)).height(px(glyph)), style])
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
    field_key(
        "artisan-sign-in-continue",
        "Continue",
        if busy {
            CursorIcon::Progress
        } else {
            CursorIcon::Pointer
        },
        Aqua::Box()
            .styles([Aqua::Style().width(px(glyph)).height(px(glyph))])
            .child(icon(arrow, 1.0 - swap, "artisan-sign-in-arrow"))
            .child(icon(spinner, swap, "artisan-sign-in-spinner"))
            .build(),
        Rc::new(move || {
            if !busy {
                submit();
            }
        }),
    )
}

pub const KEY_GLYPH: f32 = FIELD_HEIGHT * 2.0 / 3.0;

#[must_use]
pub fn field_key(
    key: &'static str,
    label: &'static str,
    cursor: CursorIcon,
    glyph: Element,
    activate: Rc<dyn Fn()>,
) -> Element {
    let surface: Arc<str> = continue_surface().into();
    let surface_width = FIELD_HEIGHT * 53.0 / 45.0;
    let surface_height = FIELD_HEIGHT * 49.0 / 45.0;
    Aqua::Button()
        .styles([Aqua::Style()
            .width(px(FIELD_HEIGHT))
            .height(px(FIELD_HEIGHT))
            .flex_shrink(0.0)
            .align(AlignItems::CENTER)
            .justify(JustifyContent::CENTER)])
        .child(
            Aqua::Canvas()
                .styles([Style::new()
                    .absolute_at(None, Some(0.0), Some(0.0), None)
                    .width(px(surface_width))
                    .height(px(surface_height))])
                .on_paint(move |bounds| {
                    vec![DisplayItem::GpuiSvg {
                        bounds,
                        source: Arc::clone(&surface),
                        tint: None,
                        opacity: 1.0,
                    }]
                })
                .build()
                .exclude_from_hit_testing(),
        )
        .child(glyph.exclude_from_hit_testing())
        .on_activate(move || activate())
        .build()
        .with_key(key)
        .aria_label(label)
        .cursor(cursor)
}

fn continue_surface() -> String {
    let highlight = POLAR_NAVY.animate_to(POLAR_HIGHLIGHT, 0.28);
    let shade = POLAR_EDGE.animate_to(BLACK, 0.22);
    let shape = "M0 0H45A8 8 0 0 1 53 8V37A8 8 0 0 1 45 45H0A8 8 0 0 0 8 37V8A8 8 0 0 0 0 0Z";
    let face = "M0 2H45A6 6 0 0 1 51 8V37A6 6 0 0 1 45 43H0Z";
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 53 49"><defs><clipPath id="c"><path d="{shape}"/></clipPath><linearGradient id="r" gradientUnits="userSpaceOnUse" x1="0" y1="0" x2="0" y2="45"><stop offset="0" stop-color="{edge}"/><stop offset="1" stop-color="{shade}"/></linearGradient><linearGradient id="h" gradientUnits="userSpaceOnUse" x1="0" y1="0" x2="0" y2="45"><stop offset="0" stop-color="{hl}"/><stop offset="1" stop-color="{navy}"/></linearGradient><radialGradient id="f" gradientUnits="userSpaceOnUse" cx="25.5" cy="53.25" r="66.3" gradientTransform="translate(25.5 53.25) scale(1 0.866) translate(-25.5 -53.25)"><stop offset="0" stop-color="{haze}"/><stop offset="0.25" stop-color="{sky}"/><stop offset="0.58" stop-color="{blue}"/><stop offset="0.92" stop-color="{navy}"/></radialGradient><filter id="d" x="-20%" y="-20%" width="140%" height="140%"><feDropShadow dx="0" dy="1.875" stdDeviation="0.9375" flood-color="#000" flood-opacity="0.3"/></filter></defs><g filter="url(#d)"><path d="{shape}" fill="url(#r)"/><g clip-path="url(#c)"><path d="M0 1H45A7 7 0 0 1 52 8V37A7 7 0 0 1 45 44H0Z" fill="url(#h)"/><path d="{face}" fill="url(#f)"/></g></g></svg>"##,
        edge = hex(POLAR_EDGE),
        shade = hex(shade),
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

fn provider_key(palette: &Palette, provider: Provider, busy: bool, emit: &Rc<dyn Fn(Event)>) -> Element {
    let pick = Rc::clone(emit);
    let source = match provider {
        Provider::Google => GOOGLE,
        Provider::GitHub => GITHUB,
    };
    let icon: Arc<str> = source.into();
    let tint = palette.ink_dim.to_reference_paint();
    let face = card::key_face(palette, FIELD_RADIUS, FIELD_HEIGHT);
    let mark = Aqua::Box()
        .layout("row")
        .styles([Style::new()
            .absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .align(AlignItems::CENTER)
            .justify(JustifyContent::CENTER)])
        .child(
            Aqua::Canvas()
                .styles([Aqua::Style().width(px(18.0)).height(px(18.0)).flex_shrink(0.0)])
                .on_paint(move |bounds| {
                    vec![DisplayItem::GpuiSvg {
                        bounds,
                        source: Arc::clone(&icon),
                        tint: Some(tint),
                        opacity: 1.0,
                    }]
                })
                .build(),
        )
        .build()
        .exclude_from_hit_testing();
    Aqua::Button()
        .styles([Aqua::Style()
            .flex_grow(1.0)
            .min_width(px(0.0))
            .radius(px(FIELD_RADIUS))])
        .child(face)
        .child(mark)
        .on_activate(move || {
            if !busy {
                pick(Event::Provider(provider));
            }
        })
        .build()
        .pressed_style(Style::new().transform(Transform2D::translate(0.0, 1.5)))
        .with_key(format!("artisan-sign-in-{}", provider.name()))
        .aria_label(format!("Continue with {}", provider.name()))
        .cursor(CursorIcon::Pointer)
}

fn glow_layer(color: Oklch, alpha: f32) -> Element {
    Aqua::Canvas()
        .styles([Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0))])
        .on_paint(move |bounds| vec![radial_glow(bounds, color, alpha)])
        .build()
        .exclude_from_hit_testing()
}

fn divider(theme: &ArtisanTheme, palette: &Palette) -> Element {
    let line = |grow: f32| {
        Aqua::Box()
            .styles([Aqua::Style()
                .flex_grow(grow)
                .height(px(1.0))
                .gpui_background(GpuiBackground::Solid(
                    palette.ink.with_alpha(0.18).to_reference_paint(),
                ))])
            .build()
    };
    Aqua::Box()
        .layout("row")
        .styles([Aqua::Style()
            .align(AlignItems::CENTER)
            .gap(12.0)
            .margin_insets(Insets {
                top: 16.0,
                bottom: 16.0,
                ..Insets::default()
            })])
        .child(line(1.0))
        .child(text(theme, "or", 12.0, 16.0, palette.ink_dim, FontWeight::NORMAL))
        .child(line(3.0))
        .build()
        .role(Role::Separator)
}

fn feedback_line(
    theme: &ArtisanTheme,
    palette: &Palette,
    key: &'static str,
    feedback: FeedbackSlot<'_>,
) -> Element {
    let reveal = feedback.reveal;
    let shown = match feedback.current {
        Feedback::None => feedback.last,
        current => current,
    };
    let (message, tone) = match shown {
        Feedback::None => (String::new(), palette.ink),
        Feedback::Error(message) => (message.clone(), ERROR),
        Feedback::Success(message) => (message.clone(), SUCCESS),
    };
    let color = tone.animate_to(palette.ink, 0.15);
    let mut words = Style::new()
        .opacity(reveal)
        .transform(Transform2D::translate(0.0, -FEEDBACK_SHIFT * (1.0 - reveal)));
    if reveal < 0.99 {
        words = words.content_blur(FEEDBACK_BLUR * (1.0 - reveal));
    }
    let content = Aqua::Box()
        .layout("column")
        .styles([
            Aqua::Style().flex_shrink(0.0).padding_insets(Insets {
                top: 8.0,
                ..Insets::default()
            }),
            words,
        ])
        .child(text(theme, &message, 12.0, 18.0, color, FontWeight::NORMAL))
        .build()
        .with_key(format!("artisan-sign-in-feedback-{key}"))
        .debug_selector(match key {
            "email" => crate::frame::sign_in::EMAIL_FEEDBACK,
            _ => crate::frame::sign_in::CODE_FEEDBACK,
        });
    Aqua::Box()
        .layout("column")
        .styles([Aqua::Style().clip(true).height(px(reveal * feedback.height))])
        .child(content)
        .build()
}

fn well(palette: &Palette, content: Element) -> Element {
    let ink = palette.ink;
    Aqua::Box()
        .layout("column")
        .styles([Aqua::Style()
            .padding(px(16.0))
            .radius(px(FIELD_RADIUS))
            .gpui_background(GpuiBackground::Solid(palette.bg.to_reference_paint()))
            .shadows(vec![
                layer(0.0, 1.0, 1.0, -0.5, true, BLACK.with_alpha(0.06)),
                layer(0.0, 3.0, 3.0, -1.5, true, BLACK.with_alpha(0.06)),
                layer(0.0, 6.0, 6.0, -3.0, true, BLACK.with_alpha(0.06)),
                layer(0.0, -0.5, 0.0, 0.0, true, WHITE.with_alpha(0.08)),
                layer(0.0, 0.0, 0.0, 0.5, true, ink.with_alpha(0.08)),
            ])])
        .child(content)
        .build()
}

fn legal(theme: &ArtisanTheme, palette: &Palette, emit: &Rc<dyn Fn(Event)>) -> Element {
    linked(
        theme,
        palette,
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

fn cancel_text(theme: &ArtisanTheme, palette: &Palette, emit: &Rc<dyn Fn(Event)>) -> Element {
    linked(
        theme,
        palette,
        &[
            ("Click ", None),
            ("here", Some(Event::Cancel)),
            (" if you want to cancel.", None),
        ],
        Rc::clone(emit),
    )
}

fn linked(
    theme: &ArtisanTheme,
    palette: &Palette,
    parts: &[(&str, Option<Event>)],
    emit: Rc<dyn Fn(Event)>,
) -> Element {
    let base = style(theme, 12.0, 18.0, FontWeight::NORMAL);
    let link_style = TextStyle {
        underline: true,
        ..base.clone()
    };
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
                StyledSpan::new(*words, link_style.clone())
                    .gpui_color(hsla(LINK))
                    .link(index - 1)
            }
            None => StyledSpan::new(*words, base.clone()).gpui_color(hsla(palette.ink_dim)),
        })
        .collect();
    Aqua::Text(String::new())
        .text_style(base)
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

fn style(theme: &ArtisanTheme, size: f32, line: f32, weight: FontWeight) -> TextStyle {
    TextStyle {
        families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
        size_px: size,
        line_height_px: line,
        weight,
        ..TextStyle::default()
    }
}

fn text(
    theme: &ArtisanTheme,
    words: &str,
    size: f32,
    line: f32,
    color: Oklch,
    weight: FontWeight,
) -> Element {
    Aqua::Text(words.to_owned())
        .text_style(style(theme, size, line, weight))
        .gpui_color(color.to_reference_paint())
        .wrap(true)
        .build()
}

fn layer(dx: f32, dy: f32, blur: f32, spread: f32, inset: bool, color: Oklch) -> ShadowLayer {
    ShadowLayer {
        dx,
        dy,
        blur,
        spread,
        inset,
        color: color.to_paint(),
    }
}

#[must_use]
pub fn screen(theme: &ArtisanTheme, titlebar: Element, card: Element) -> Element {
    Aqua::Box()
        .layout("column")
        .styles([Aqua::Style()
            .width(pct(100.0))
            .height(pct(100.0))
            .gpui_background(GpuiBackground::Solid(theme.colors.frame.to_reference_paint()))
            .text_style(style(theme, 14.0, 16.8, FontWeight::NORMAL))])
        .child(titlebar)
        .child(
            Aqua::Box()
                .layout("column")
                .styles([Aqua::Style()
                    .flex_grow(1.0)
                    .min_height(px(0.0))
                    .align(AlignItems::CENTER)
                    .justify(JustifyContent::CENTER)
                    .padding_insets(Insets {
                        bottom: crate::ui::chrome::TITLEBAR_HEIGHT,
                        ..Insets::default()
                    })])
                .child(card)
                .build(),
        )
        .build()
}
