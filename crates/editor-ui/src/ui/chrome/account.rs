use crate::ui::chrome::card::{POLAR_EDGE, POLAR_HAZE, POLAR_HIGHLIGHT, POLAR_INK, polar_face};
use crate::ui::chrome::limits::{EngineLimits, Limit};
use crate::ui::chrome::menu::{Event, Highlight, Menu, Spot};
use crate::ui::engine::engine_icon;
use crate::ui::asset::source_asset_glyph;
use crate::ui::theme::{ArtisanTheme, Oklch};
use aqua::images::ImageHandle;
use aqua::{Aqua, ShadowLayer, Style, pct, px};
use aqua_core::{
    CursorIcon, DisplayItem, GpuiBackground, GpuiGradientColorSpace, GpuiGradientStop, ImageRef,
    Insets, Role, Transform2D, image::ImageFit,
};
use aqua_text::{FontFamily, FontWeight, TextStyle};
use aqua_ui::element::{Element, Media};
use aqua_ui::{AlignItems, JustifyContent};
use editor_assets::AssetId;
use std::{rc::Rc, sync::Arc, time::Instant};

pub const TRIGGER: &str = "artisan-account-trigger";
pub const TRIGGER_HEIGHT: f32 = 40.0;
pub const AVATAR: f32 = 20.0;
pub const AVATAR_INSET: f32 = (TRIGGER_HEIGHT - AVATAR) / 2.0;
pub const MENU_GAP: f32 = 8.0;

const RADIUS: f32 = 0.65 * 16.0;
const MENU_PADDING: f32 = 6.0;
const ITEM_RADIUS: f32 = RADIUS - MENU_PADDING;
const ITEM_PAD_X: f32 = 12.0;
const ITEM_PAD_Y: f32 = 8.0;
const LINE: f32 = 20.0;
const TEXT: f32 = 14.0;
const ITEM_HEIGHT: f32 = LINE + 2.0 * ITEM_PAD_Y;
const BADGE: f32 = 40.0;
const BADGE_RADIUS: f32 = 10.4;
const HEADER_HEIGHT: f32 = BADGE + 2.0 * ITEM_PAD_Y;
const GLYPH: f32 = 16.0;
const FALLBACK_GLYPH: f32 = 18.0;
const CHEVRON: f32 = 12.0;
const UPGRADE_HEIGHT: f32 = ITEM_HEIGHT + 2.0;
const LIMIT_TEXT: f32 = 12.0;
const LIMIT_LINE: f32 = 16.0;
const LIMIT_ROW: f32 = 24.0;
const LIMIT_LABEL: f32 = 52.0;
const BLUR: f32 = 20.0;
const HOVER_LIFT: f32 = 0.06;

const CHEVRON_RIGHT: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M4.5 3 7.5 6l-3 3"/></svg>"#;

const WHITE: Oklch = Oklch::new(1.0, 0.0, 0.0);
const BLACK: Oklch = Oklch::new(0.0, 0.0, 0.0);
const GRAY_900: Oklch = Oklch::new(0.21, 0.034, 264.665);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Identity {
    Named { name: String, detail: String },
    Unnamed { detail: String },
}

impl Identity {
    #[must_use]
    pub fn of(display_name: &str, email: &str, username: Option<&str>) -> Self {
        let detail = username
            .map(str::trim)
            .map(|username| username.trim_start_matches('@'))
            .filter(|username| !username.is_empty())
            .map_or_else(|| email.to_owned(), |username| format!("@{username}"));
        match display_name.trim() {
            "" => Self::Unnamed { detail },
            name => Self::Named {
                name: name.to_owned(),
                detail,
            },
        }
    }

    #[must_use]
    pub fn label(&self) -> &str {
        match self {
            Self::Named { name, .. } => name,
            Self::Unnamed { detail } => detail,
        }
    }
}

pub struct Account {
    pub identity: Identity,
    pub avatar: Option<Arc<ImageHandle>>,
    pub limits: Vec<EngineLimits>,
}

struct Shadow(f32, f32, f32, Oklch);

struct Palette {
    dark: bool,
    ink: Oklch,
    ink_dim: Oklch,
    glass_near: Oklch,
    glass_far: Oklch,
    rim_shade: Oklch,
    hover: (f32, f32),
    hover_edge: f32,
    press: Shadow,
    overlay: [Shadow; 4],
}

impl Palette {
    fn of(theme: &ArtisanTheme) -> Self {
        if theme.colors.mode.is_dark() {
            Self {
                dark: true,
                ink: WHITE,
                ink_dim: Oklch::new(0.709, 0.0, 0.0),
                glass_near: Oklch::new(0.2801, 0.0, 0.0),
                glass_far: Oklch::new(0.2484, 0.0, 0.0),
                rim_shade: BLACK.with_alpha(0.22),
                hover: (0.09, 0.03),
                hover_edge: 0.07,
                press: Shadow(1.5, 1.5, 0.0, BLACK.with_alpha(0.3)),
                overlay: [
                    Shadow(1.0, 2.0, 0.0, BLACK.with_alpha(0.25)),
                    Shadow(4.0, 8.0, -2.0, BLACK.with_alpha(0.3)),
                    Shadow(12.0, 24.0, -6.0, BLACK.with_alpha(0.35)),
                    Shadow(24.0, 48.0, -12.0, BLACK.with_alpha(0.35)),
                ],
            }
        } else {
            Self {
                dark: false,
                ink: BLACK,
                ink_dim: Oklch::new(0.551, 0.027, 264.364),
                glass_near: Oklch::new(0.872, 0.01, 258.338),
                glass_far: Oklch::new(0.928, 0.006, 264.531),
                rim_shade: BLACK.with_alpha(0.05),
                hover: (0.02, 0.06),
                hover_edge: 0.05,
                press: Shadow(1.0, 1.5, 0.0, GRAY_900.with_alpha(0.12)),
                overlay: [
                    Shadow(1.0, 2.0, 0.0, GRAY_900.with_alpha(0.03)),
                    Shadow(4.0, 8.0, -2.0, GRAY_900.with_alpha(0.04)),
                    Shadow(12.0, 24.0, -6.0, GRAY_900.with_alpha(0.04)),
                    Shadow(24.0, 48.0, -12.0, GRAY_900.with_alpha(0.05)),
                ],
            }
        }
    }
}

pub struct Trigger<'a> {
    pub account: &'a Account,
    pub menu: &'a Menu,
    pub now: Instant,
    pub emit: Rc<dyn Fn(Event)>,
}

#[must_use]
pub fn trigger(theme: &ArtisanTheme, trigger: Trigger<'_>) -> Element {
    let palette = Palette::of(theme);
    let hover = Rc::clone(&trigger.emit);
    let pick = Rc::clone(&trigger.emit);
    Aqua::Button()
        .styles([Aqua::Style()
            .width(pct(100.0))
            .height(px(TRIGGER_HEIGHT))
            .align(AlignItems::CENTER)
            .gap(10.0)
            .radius(px(RADIUS))
            .padding_insets(Insets {
                left: AVATAR_INSET,
                right: ITEM_PAD_X,
                ..Insets::default()
            })])
        .child(highlight(
            &palette,
            trigger.menu.highlight(Spot::Trigger),
            trigger.now,
            TRIGGER_HEIGHT,
            RADIUS,
        ))
        .child(portrait(trigger.account, AVATAR, AVATAR / 2.0, 14.0))
        .child(
            Aqua::Box()
                .styles([Aqua::Style()
                    .flex_grow(1.0)
                    .min_width(px(0.0))
                    .clip(true)])
                .child(label(
                    theme,
                    trigger.account.identity.label(),
                    palette.ink,
                    FontWeight::NORMAL,
                ))
                .build(),
        )
        .on_activate(move || pick(Event::Pick(Spot::Trigger)))
        .build()
        .with_key(TRIGGER)
        .debug_selector(TRIGGER)
        .aria_label(trigger.account.identity.label().to_owned())
        .aria_expanded(trigger.menu.open())
        .cursor(CursorIcon::Pointer)
        .on_hover_change(move |inside| hover(Event::Hover(Spot::Trigger, inside)))
}

pub struct Sheet<'a> {
    pub account: &'a Account,
    pub menu: &'a Menu,
    pub width: f32,
    pub now: Instant,
    pub emit: Rc<dyn Fn(Event)>,
}

#[must_use]
pub fn menu_surface(theme: &ArtisanTheme, children: Vec<Element>) -> Element {
    let palette = Palette::of(theme);
    let mut shadows = vec![layer(0.0, -1.0, 0.0, 0.0, false, WHITE.with_alpha(0.14))];
    shadows.extend(
        palette
            .overlay
            .iter()
            .map(|Shadow(dy, blur, spread, color)| layer(0.0, *dy, *blur, *spread, false, *color)),
    );
    Aqua::Box()
        .layout("column")
        .styles([
            Aqua::Style().radius(px(RADIUS)).clip(true),
            Style::new().shadows(shadows),
        ])
        .child(glass(&palette))
        .children(children)
        .build()
}

pub const fn menu_radius() -> f32 {
    RADIUS
}

pub fn hover_style(theme: &ArtisanTheme) -> Style {
    hover_material(&Palette::of(theme))
}

fn hover_material(palette: &Palette) -> Style {
    let (top, bottom) = palette.hover;
    Style::new()
        .gpui_background(GpuiBackground::LinearGradient {
            angle: 180.0,
            color_space: GpuiGradientColorSpace::Oklab,
            stops: [
                stop(0.0, palette.ink.with_alpha(top)),
                stop(1.0, palette.ink.with_alpha(bottom)),
            ],
        })
        .shadows(vec![layer(
            0.0,
            0.0,
            0.0,
            0.5,
            true,
            palette.ink.with_alpha(palette.hover_edge),
        )])
}

pub fn menu(theme: &ArtisanTheme, sheet: Sheet<'_>) -> Element {
    let palette = Palette::of(theme);
    let lit = |spot| sheet.menu.highlight(spot);
    let entry = |spot, title: &str, icon| {
        item(
            theme,
            &palette,
            spot,
            title,
            icon,
            lit(spot),
            sheet.now,
            &sheet.emit,
        )
    };
    let theme_icon = if palette.dark {
        AssetId::TABLER_MOON
    } else {
        AssetId::TABLER_SUN
    };
    let content = Aqua::Box()
        .layout("column")
        .child(header(theme, &palette, &sheet))
        .child(crate::ui::chrome::card::double_rule(theme, MENU_PADDING))
        .child(entry(Spot::Theme, "Theme", theme_icon))
        .child(entry(Spot::Settings, "Settings", AssetId::TABLER_SETTINGS))
        .child(entry(
            Spot::Help,
            "Help & Feedback",
            AssetId::TABLER_MESSAGE_CIRCLE_QUESTION,
        ))
        .child(upgrade(theme, &palette, &sheet))
        .child(crate::ui::chrome::card::double_rule(theme, MENU_PADDING))
        .children(limits(theme, &palette, &sheet.account.limits))
        .child(entry(Spot::SignOut, "Sign out", AssetId::TABLER_LOGOUT))
        .build();
    menu_surface(theme, vec![content])
        .style(
            Style::new()
                .width(px(sheet.width))
                .padding(px(MENU_PADDING)),
        )
        .with_key("artisan-account-menu")
        .role(Role::Menu)
}

fn header(theme: &ArtisanTheme, palette: &Palette, sheet: &Sheet<'_>) -> Element {
    let hover = Rc::clone(&sheet.emit);
    let pick = Rc::clone(&sheet.emit);
    Aqua::Button()
        .styles([Aqua::Style()
            .width(pct(100.0))
            .align(AlignItems::CENTER)
            .gap(12.0)
            .padding(px(ITEM_PAD_Y))
            .radius(px(ITEM_RADIUS))])
        .child(highlight(
            palette,
            sheet.menu.highlight(Spot::Identity),
            sheet.now,
            HEADER_HEIGHT,
            ITEM_RADIUS,
        ))
        .child(portrait(sheet.account, BADGE, BADGE_RADIUS, FALLBACK_GLYPH))
        .child(
            Aqua::Box()
                .layout("column")
                .styles([Aqua::Style()
                    .flex_grow(1.0)
                    .min_width(px(0.0))
                    .clip(true)
                    .padding_insets(Insets {
                        right: ITEM_PAD_Y,
                        ..Insets::default()
                    })])
                .children(identity_lines(theme, palette, &sheet.account.identity))
                .build(),
        )
        .child(inline_glyph(CHEVRON_RIGHT, CHEVRON, palette.ink_dim))
        .on_activate(move || pick(Event::Pick(Spot::Identity)))
        .build()
        .with_key("artisan-account-identity")
        .role(Role::MenuItem)
        .aria_label(sheet.account.identity.label().to_owned())
        .cursor(CursorIcon::Pointer)
        .on_hover_change(move |inside| hover(Event::Hover(Spot::Identity, inside)))
}

fn identity_lines(theme: &ArtisanTheme, palette: &Palette, identity: &Identity) -> Vec<Element> {
    match identity {
        Identity::Named { name, detail } => vec![
            label(theme, name, palette.ink, FontWeight::NORMAL),
            label(theme, detail, palette.ink_dim, FontWeight::NORMAL),
        ],
        Identity::Unnamed { detail } => vec![label(theme, detail, palette.ink, FontWeight::NORMAL)],
    }
}

#[expect(clippy::too_many_arguments, reason = "one account menu item receives its presentation and event context")]
fn item(
    theme: &ArtisanTheme,
    palette: &Palette,
    spot: Spot,
    title: &str,
    icon: AssetId,
    lit: Highlight,
    now: Instant,
    emit: &Rc<dyn Fn(Event)>,
) -> Element {
    let hover = Rc::clone(emit);
    let pick = Rc::clone(emit);
    Aqua::Button()
        .styles([Aqua::Style()
            .width(pct(100.0))
            .align(AlignItems::CENTER)
            .gap(8.0)
            .radius(px(ITEM_RADIUS))
            .padding_insets(Insets {
                left: ITEM_PAD_X,
                right: ITEM_PAD_X,
                top: ITEM_PAD_Y,
                bottom: ITEM_PAD_Y,
            })])
        .child(highlight(palette, lit, now, ITEM_HEIGHT, ITEM_RADIUS))
        .child(
            Aqua::Box()
                .styles([Aqua::Style().flex_grow(1.0).min_width(px(0.0))])
                .child(label(theme, title, palette.ink, FontWeight::NORMAL))
                .build(),
        )
        .child(source_asset_glyph(icon, GLYPH, palette.ink_dim.to_reference_paint()))
        .on_activate(move || pick(Event::Pick(spot)))
        .build()
        .with_key(format!("artisan-account-{spot:?}"))
        .role(Role::MenuItem)
        .aria_label(title.to_owned())
        .cursor(CursorIcon::Pointer)
        .on_hover_change(move |inside| hover(Event::Hover(spot, inside)))
}

fn upgrade(theme: &ArtisanTheme, palette: &Palette, sheet: &Sheet<'_>) -> Element {
    let hover = Rc::clone(&sheet.emit);
    let pick = Rc::clone(&sheet.emit);
    let lift = if sheet.menu.highlight(Spot::Upgrade).lit() {
        HOVER_LIFT
    } else {
        0.0
    };
    let press = &palette.press;
    let inner = Aqua::Box()
        .styles([
            Aqua::Style()
                .absolute_at(Some(1.0), Some(1.0), Some(1.0), Some(1.0))
                .radius(px(ITEM_RADIUS - 1.0))
                .clip(true),
            Style::new().shadows(vec![
                layer(0.0, 1.0, 0.0, 0.0, true, POLAR_HIGHLIGHT.with_alpha(0.28)),
                layer(0.0, -1.0, 0.0, 0.0, true, BLACK.with_alpha(0.22)),
            ]),
        ])
        .child(polar(lift))
        .build()
        .exclude_from_hit_testing();
    Aqua::Button()
        .styles([
            Aqua::Style()
                .width(pct(100.0))
                .height(px(UPGRADE_HEIGHT))
                .align(AlignItems::CENTER)
                .justify(JustifyContent::CENTER)
                .gap(8.0)
                .radius(px(ITEM_RADIUS))
                .margin_insets(Insets {
                    top: 8.0,
                    bottom: 4.0,
                    ..Insets::default()
                }),
            Style::new()
                .gpui_background(vertical(POLAR_EDGE, POLAR_EDGE, 1.0))
                .shadows(vec![layer(0.0, press.0, press.1, press.2, false, press.3)]),
        ])
        .child(inner)
        .child(source_asset_glyph(
            AssetId::TABLER_CIRCLE_ARROW_UP,
            GLYPH,
            POLAR_INK.with_alpha(0.8).to_reference_paint(),
        ))
        .child(label(theme, "Upgrade to Go", POLAR_INK, FontWeight::MEDIUM))
        .on_activate(move || pick(Event::Pick(Spot::Upgrade)))
        .build()
        .with_key("artisan-account-upgrade")
        .role(Role::MenuItem)
        .aria_label("Upgrade to Go")
        .cursor(CursorIcon::Pointer)
        .on_hover_change(move |inside| hover(Event::Hover(Spot::Upgrade, inside)))
}

fn limits(theme: &ArtisanTheme, palette: &Palette, engines: &[EngineLimits]) -> Vec<Element> {
    match engines {
        [] => Vec::new(),
        engines => vec![
            Aqua::Box()
                .layout("column")
                .styles([Aqua::Style()
                    .width(pct(100.0))
                    .gap(2.0)
                    .padding_insets(Insets {
                        left: ITEM_PAD_X,
                        right: ITEM_PAD_X,
                        top: ITEM_PAD_Y,
                        bottom: ITEM_PAD_Y,
                    })])
                .children(engines.iter().flat_map(|engine| {
                    engine.limits.iter().enumerate().map(move |(index, limit)| {
                        limit_row(theme, palette, engine, (index == 0).then_some(engine.engine), limit)
                    })
                }))
                .build()
                .with_key("artisan-account-limits"),
            crate::ui::chrome::card::double_rule(theme, MENU_PADDING),
        ],
    }
}

fn limit_row(
    theme: &ArtisanTheme,
    palette: &Palette,
    engine: &EngineLimits,
    mark: Option<artisan_api::EngineId>,
    limit: &Limit,
) -> Element {
    let icon = Aqua::Box().styles([Aqua::Style()
        .width(px(GLYPH))
        .height(px(GLYPH))
        .flex_shrink(0.0)
        .align(AlignItems::CENTER)
        .justify(JustifyContent::CENTER)]);
    let icon = match mark {
        Some(engine) => icon
            .child(source_asset_glyph(
                engine_icon(theme, engine),
                if engine == artisan_api::EngineId::OpenCode { 12.8 } else { GLYPH },
                palette.ink.to_reference_paint(),
            ))
            .build(),
        None => icon.build(),
    };
    let percent = limit.used_percent.round().clamp(0.0, 100.0);
    Aqua::Box()
        .styles([Aqua::Style()
            .width(pct(100.0))
            .height(px(LIMIT_ROW))
            .align(AlignItems::CENTER)
            .gap(10.0)])
        .child(icon)
        .child(
            Aqua::Box()
                .styles([Aqua::Style().width(px(LIMIT_LABEL)).flex_shrink(0.0).clip(true)])
                .child(small_label(theme, &limit.label, palette.ink_dim))
                .build(),
        )
        .child(
            Aqua::Box()
                .styles([Aqua::Style().flex_grow(1.0).min_width(px(0.0))])
                .child(crate::ui::progress::bar(theme, limit.used_percent, crate::ui::progress::HEIGHT))
                .build(),
        )
        .build()
        .with_key(format!("artisan-account-limit-{}-{}", engine.engine, limit.label))
        .aria_label(format!(
            "{} {} limit: {percent:.0}% used",
            engine.engine.display_name(),
            limit.label
        ))
        .tooltip(0.5, {
            let theme = *theme;
            let engine = engine.engine;
            let limit = limit.clone();
            move || {
                let now = chrono::Utc::now().timestamp_millis();
                let outlook = crate::ui::chrome::limits::outlook(&limit, now);
                crate::ui::hint_card::hint_card(
                    &theme,
                    &crate::ui::chrome::limits::title(engine, &limit),
                    &crate::ui::chrome::limits::explanation(&limit, outlook, crate::ui::local_time::occasion),
                )
            }
        })
}

fn polar(lift: f32) -> Element {
    Aqua::Canvas()
        .styles([Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0))])
        .on_paint(move |bounds| vec![polar_face(bounds, lift)])
        .build()
        .exclude_from_hit_testing()
}

#[must_use]
fn portrait(account: &Account, edge: f32, radius: f32, fallback: f32) -> Element {
    let frame = Aqua::Box().styles([Aqua::Style()
        .width(px(edge))
        .height(px(edge))
        .flex_shrink(0.0)
        .radius(px(radius))
        .clip(true)
        .align(AlignItems::CENTER)
        .justify(JustifyContent::CENTER)]);
    match &account.avatar {
        Some(image) => frame
            .child(
                Media::new(ImageRef::with_lease(image.id(), lease(image)))
                    .width(edge)
                    .height(edge)
                    .fit(ImageFit::Cover)
                    .styles([Aqua::Style().radius(px(radius))])
                    .build(),
            )
            .build(),
        None => frame
            .child(source_asset_glyph(
                AssetId::TABLER_USER,
                fallback,
                POLAR_HAZE.to_reference_paint(),
            ))
            .build(),
    }
    .exclude_from_hit_testing()
}

fn lease(image: &Arc<ImageHandle>) -> Arc<dyn std::any::Any + Send + Sync> {
    Arc::clone(image) as Arc<dyn std::any::Any + Send + Sync>
}

fn highlight(palette: &Palette, lit: Highlight, now: Instant, height: f32, radius: f32) -> Element {
    let scale = lit.scale(now);
    Aqua::Box()
        .styles([hover_material(palette)
            .absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .radius(px(radius))
            .opacity(lit.opacity(now))
            .transform(
                Transform2D::translate(0.0, -height)
                    .then(Transform2D::scale(1.0, scale))
                    .then(Transform2D::translate(0.0, height)),
            )])
        .build()
        .exclude_from_hit_testing()
}

fn glass(palette: &Palette) -> Element {
    let rim_shade = palette.rim_shade.to_reference_paint();
    let base = palette.ink.with_alpha(0.05).to_reference_paint();
    let near = palette.glass_near.with_alpha(0.22);
    let far = palette.glass_far.with_alpha(0.16);
    let sheen = (WHITE.with_alpha(0.08), WHITE.with_alpha(0.0));
    Aqua::Canvas()
        .styles([Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0))])
        .on_paint(move |bounds| {
            let corners = aqua_core::geometry::CornerRadii::uniform(RADIUS);
            let gradient = |angle, from: Oklch, to: Oklch, end| DisplayItem::GpuiBackground {
                bounds,
                corners,
                background: GpuiBackground::LinearGradient {
                    angle,
                    color_space: GpuiGradientColorSpace::Oklab,
                    stops: [stop(0.0, from), stop(end, to)],
                },
            };
            vec![
                DisplayItem::BackdropBlurRounded {
                    bounds,
                    radius: BLUR,
                    corners,
                },
                DisplayItem::GpuiBackground {
                    bounds,
                    corners,
                    background: GpuiBackground::Solid(base),
                },
                gradient(145.0, near, far, 1.0),
                gradient(180.0, sheen.0, sheen.1, 0.44),
                DisplayItem::GpuiShadow {
                    bounds,
                    corners,
                    layer: aqua_core::scene::GpuiShadowLayer {
                        dx: 0.0,
                        dy: 1.0,
                        blur: 0.0,
                        spread: 0.0,
                        inset: true,
                        color: WHITE.with_alpha(0.1).to_reference_paint(),
                    },
                },
                DisplayItem::GpuiShadow {
                    bounds,
                    corners,
                    layer: aqua_core::scene::GpuiShadowLayer {
                        dx: 0.0,
                        dy: -1.0,
                        blur: 0.0,
                        spread: 0.0,
                        inset: true,
                        color: rim_shade,
                    },
                },
            ]
        })
        .build()
        .exclude_from_hit_testing()
}

fn label(theme: &ArtisanTheme, text: &str, color: Oklch, weight: FontWeight) -> Element {
    Aqua::Text(text.to_owned())
        .text_style(TextStyle {
            families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
            size_px: TEXT,
            line_height_px: LINE,
            weight,
            ..TextStyle::default()
        })
        .wrap(false)
        .overflow(aqua_text::overflow::OverflowMode::EndEllipsis)
        .gpui_color(color.to_reference_paint())
        .build()
}

fn small_label(theme: &ArtisanTheme, text: &str, color: Oklch) -> Element {
    Aqua::Text(text.to_owned())
        .text_style(TextStyle {
            families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
            size_px: LIMIT_TEXT,
            line_height_px: LIMIT_LINE,
            weight: FontWeight::NORMAL,
            ..TextStyle::default()
        })
        .wrap(false)
        .overflow(aqua_text::overflow::OverflowMode::EndEllipsis)
        .gpui_color(color.to_reference_paint())
        .build()
}

fn inline_glyph(source: &'static str, edge: f32, tint: Oklch) -> Element {
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

fn vertical(top: Oklch, bottom: Oklch, end: f32) -> GpuiBackground {
    GpuiBackground::LinearGradient {
        angle: 180.0,
        color_space: GpuiGradientColorSpace::Oklab,
        stops: [stop(0.0, top), stop(end, bottom)],
    }
}

fn stop(percentage: f32, color: Oklch) -> GpuiGradientStop {
    GpuiGradientStop {
        percentage,
        color: color.to_reference_paint(),
    }
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
