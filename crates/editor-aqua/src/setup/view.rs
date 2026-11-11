use super::model::{Event, Kind, Machine, Phase, Setup};
use crate::ui::button::{Button, ButtonVariant};
use crate::ui::chrome::card::{self, Surfaces, hex};
use crate::ui::theme::{ArtisanTheme, Oklch};
use aqua::{Aqua, Style, pct, px};
use aqua_core::{CursorIcon, DisplayItem, GpuiBackground, Insets, Transform2D};
use aqua_text::{FontFamily, FontWeight, TextStyle};
use aqua_ui::element::Element;
use aqua_ui::{AlignItems, JustifyContent};
use editor_assets::AssetId;
use std::rc::Rc;
use std::sync::Arc;

pub const WINDOW: (f64, f64) = (480.0, 640.0);
pub const SPIN_STEPS: u32 = 24;
const WIDTH: f32 = 400.0;
const INSET: f32 = 8.0;
const PADDING: f32 = 16.0;
const HEADER_PADDING: f32 = 24.0;
const WORDMARK_HEIGHT: f32 = 72.0;
const WORDMARK_WIDTH: f32 = WORDMARK_HEIGHT * 136.675 / 225.15;
const ROW_RADIUS: f32 = 0.4 * 16.0;
const ROW_HEIGHT: f32 = 58.0;
const COMPACT_ROW_HEIGHT: f32 = 36.0;
const ROOMY_ROWS: usize = 4;
const ERROR: Oklch = Oklch::new(0.66, 0.2, 22.0);

pub struct Props<'a> {
    pub setup: &'a Setup,
    pub spin: u32,
    pub emit: Rc<dyn Fn(Event)>,
}

#[must_use]
pub fn screen(theme: &ArtisanTheme, props: &Props<'_>) -> Element {
    let palette = Surfaces::of(theme);
    let header = Aqua::Box()
        .layout("row")
        .styles([Aqua::Style()
            .padding(px(HEADER_PADDING))
            .gap(HEADER_PADDING)
            .align(AlignItems::END)
            .justify(JustifyContent::SPACE_BETWEEN)])
        .child(wordmark(palette.ink))
        .child(heading(theme, &palette, props.setup))
        .build();
    let body = Aqua::Box()
        .layout("column")
        .styles([Aqua::Style().padding(px(PADDING)).gap(PADDING)])
        .children(match &props.setup.phase {
            Phase::Choosing => choosing(theme, &palette, props),
            Phase::Installing { line } => vec![working(theme, &palette, props, line)],
            Phase::Failed { message } => failed(theme, &palette, props, message),
            Phase::Done => vec![text(
                theme,
                "Opening Artisan Editor…",
                14.0,
                20.0,
                palette.ink,
                FontWeight::NORMAL,
            )],
        })
        .build();
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
                .child(card::inset(theme, vec![body]))
                .build(),
        )
        .build();
    let card = Aqua::Box()
        .layout("column")
        .styles([Aqua::Style().width(px(WIDTH))])
        .child(card::outlined(
            palette.ink,
            palette.bg,
            card::RADIUS,
            card::rest_shadows(),
            vec![stack],
        ))
        .build()
        .with_key("artisan-setup-card");
    Aqua::Box()
        .layout("column")
        .styles([Aqua::Style()
            .width(pct(100.0))
            .height(pct(100.0))
            .align(AlignItems::CENTER)
            .justify(JustifyContent::CENTER)
            .gpui_background(GpuiBackground::Solid(
                theme.colors.frame.to_reference_paint(),
            ))
            .text_style(style(theme, 14.0, 16.8, FontWeight::NORMAL))])
        .child(card)
        .build()
}

fn heading(theme: &ArtisanTheme, palette: &Surfaces, setup: &Setup) -> Element {
    let (title, detail) = match &setup.kind {
        Kind::Fresh => (
            "Set up Artisan Editor",
            format!("Version {}", setup.version),
        ),
        Kind::Update { from } => (
            "Update Artisan Editor",
            format!("{from} to {}", setup.version),
        ),
    };
    Aqua::Box()
        .layout("column")
        .styles([Aqua::Style()
            .flex_grow(1.0)
            .min_width(px(0.0))
            .gap(4.0)
            .align(AlignItems::END)])
        .child(text(
            theme,
            title,
            18.0,
            24.0,
            palette.ink,
            FontWeight::MEDIUM,
        ))
        .child(text(
            theme,
            &detail,
            12.0,
            16.0,
            palette.ink_dim,
            FontWeight::NORMAL,
        ))
        .build()
}

fn choosing(theme: &ArtisanTheme, palette: &Surfaces, props: &Props<'_>) -> Vec<Element> {
    let change = Rc::clone(&props.emit);
    let begin = Rc::clone(&props.emit);
    let folder = Aqua::Box()
        .layout("column")
        .styles([Aqua::Style().flex_grow(1.0).min_width(px(0.0))])
        .child(text(
            theme,
            &props.setup.location.display().to_string(),
            13.0,
            18.0,
            palette.ink,
            FontWeight::NORMAL,
        ))
        .build();
    let mut location = Aqua::Box()
        .layout("row")
        .styles([Aqua::Style().gap(12.0).align(AlignItems::CENTER)])
        .child(folder);
    if props.setup.relocatable {
        location = location.child(
            Button::text("artisan-setup-change", *theme, "Change…")
                .variant(ButtonVariant::Outline)
                .on_activate(move |_| change(Event::ChangeLocation))
                .build(),
        );
    }
    let location = location.build();
    let roomy = props.setup.machines.len() <= ROOMY_ROWS;
    let machines = Aqua::Box()
        .layout("column")
        .styles([Aqua::Style().gap(if roomy { 8.0 } else { 4.0 })])
        .children(
            props
                .setup
                .machines
                .iter()
                .enumerate()
                .map(|(index, machine)| {
                    machine_row(
                        theme,
                        palette,
                        machine,
                        index,
                        index == props.setup.selected,
                        roomy,
                        &props.emit,
                    )
                }),
        )
        .build();
    let install = Aqua::Box()
        .layout("row")
        .styles([Aqua::Style().justify(JustifyContent::END)])
        .child(
            Button::text("artisan-setup-install", *theme, "Install")
                .on_activate(move |_| begin(Event::Started))
                .build(),
        )
        .build();
    vec![
        section(theme, palette, "Install location", location),
        section(theme, palette, "Where should agents work?", machines),
        install,
    ]
}

fn section(theme: &ArtisanTheme, palette: &Surfaces, label: &str, content: Element) -> Element {
    Aqua::Box()
        .layout("column")
        .styles([Aqua::Style().gap(8.0)])
        .child(text(
            theme,
            label,
            12.0,
            16.0,
            palette.ink_dim,
            FontWeight::MEDIUM,
        ))
        .child(content)
        .build()
}

fn machine_row(
    theme: &ArtisanTheme,
    palette: &Surfaces,
    machine: &Machine,
    index: usize,
    selected: bool,
    roomy: bool,
    emit: &Rc<dyn Fn(Event)>,
) -> Element {
    let pick = Rc::clone(emit);
    let height = if roomy {
        ROW_HEIGHT
    } else {
        COMPACT_ROW_HEIGHT
    };
    let title_ink = if selected {
        palette.ink
    } else {
        palette.ink_dim
    };
    let label = Aqua::Box()
        .layout("row")
        .styles([Style::new()
            .absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .padding_insets(Insets::hv(12.0, 0.0))
            .gap(12.0)
            .align(AlignItems::CENTER)])
        .child(radio(palette, selected))
        .child(
            Aqua::Box()
                .layout("column")
                .styles([Aqua::Style().flex_grow(1.0).min_width(px(0.0)).gap(2.0)])
                .child(text(
                    theme,
                    &machine.title(),
                    14.0,
                    20.0,
                    title_ink,
                    FontWeight::MEDIUM,
                ))
                .children(roomy.then(|| {
                    text(
                        theme,
                        machine.detail(),
                        12.0,
                        16.0,
                        palette.ink_dim,
                        FontWeight::NORMAL,
                    )
                }))
                .build(),
        )
        .build()
        .exclude_from_hit_testing();
    Aqua::Button()
        .styles([Aqua::Style().radius(px(ROW_RADIUS))])
        .child(card::key_face(palette, ROW_RADIUS, height))
        .child(label)
        .on_activate(move || pick(Event::Selected(index)))
        .build()
        .pressed_style(Style::new().transform(Transform2D::translate(0.0, 1.5)))
        .with_key(format!("artisan-setup-machine-{index}"))
        .aria_label(machine.title())
        .aria_selected(selected)
        .cursor(CursorIcon::Pointer)
}

fn radio(palette: &Surfaces, selected: bool) -> Element {
    let ring = if selected {
        palette.ink
    } else {
        palette.ink_dim
    };
    let dot = if selected {
        format!(
            r#"<circle cx="8" cy="8" r="3.5" fill="{}"/>"#,
            hex(palette.ink)
        )
    } else {
        String::new()
    };
    picture(
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><circle cx="8" cy="8" r="6.75" fill="none" stroke="{}" stroke-width="1.5"/>{dot}</svg>"#,
            hex(ring)
        ),
        16.0,
    )
}

fn spinner(palette: &Surfaces, spin: u32) -> Element {
    picture(
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 30 30" fill="none" stroke="{ink}" stroke-width="3" stroke-linecap="round"><circle cx="15" cy="15" r="8.5" stroke-opacity="0.18"/><path transform="rotate({angle} 15 15)" d="M15 6.5a8.5 8.5 0 0 1 8.5 8.5"/></svg>"#,
            ink = hex(palette.ink),
            angle = spin % SPIN_STEPS * 360 / SPIN_STEPS,
        ),
        20.0,
    )
}

fn picture(svg: String, size: f32) -> Element {
    let source: Arc<str> = svg.into();
    Aqua::Canvas()
        .styles([Aqua::Style()
            .width(px(size))
            .height(px(size))
            .flex_shrink(0.0)])
        .on_paint(move |bounds| {
            vec![DisplayItem::GpuiSvg {
                bounds,
                source: Arc::clone(&source),
                tint: None,
                opacity: 1.0,
            }]
        })
        .build()
        .exclude_from_hit_testing()
}

fn working(theme: &ArtisanTheme, palette: &Surfaces, props: &Props<'_>, line: &str) -> Element {
    let title = match props.setup.kind {
        Kind::Fresh => "Installing…",
        Kind::Update { .. } => "Updating…",
    };
    let detail = if line.is_empty() {
        "Getting ready"
    } else {
        line
    };
    Aqua::Box()
        .layout("row")
        .styles([Aqua::Style().gap(12.0).align(AlignItems::START)])
        .child(spinner(palette, props.spin))
        .child(
            Aqua::Box()
                .layout("column")
                .styles([Aqua::Style().flex_grow(1.0).min_width(px(0.0)).gap(4.0)])
                .child(text(
                    theme,
                    title,
                    14.0,
                    20.0,
                    palette.ink,
                    FontWeight::MEDIUM,
                ))
                .child(text(
                    theme,
                    detail,
                    12.0,
                    16.0,
                    palette.ink_dim,
                    FontWeight::NORMAL,
                ))
                .build(),
        )
        .build()
}

fn failed(
    theme: &ArtisanTheme,
    palette: &Surfaces,
    props: &Props<'_>,
    message: &str,
) -> Vec<Element> {
    let retry = Rc::clone(&props.emit);
    vec![
        text(
            theme,
            "Setup couldn't finish",
            14.0,
            20.0,
            palette.ink,
            FontWeight::MEDIUM,
        ),
        text(theme, message, 13.0, 18.0, ERROR, FontWeight::NORMAL),
        Aqua::Box()
            .layout("row")
            .styles([Aqua::Style().justify(JustifyContent::END)])
            .child(
                Button::text("artisan-setup-retry", *theme, "Try again")
                    .on_activate(move |_| retry(Event::Started))
                    .build(),
            )
            .build(),
    ]
}

fn wordmark(ink: Oklch) -> Element {
    let source: Arc<str> = editor_assets::get(AssetId::ARTISAN_WORDMARK_STACKED)
        .source
        .into();
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
        .aria_label("Artisan")
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
