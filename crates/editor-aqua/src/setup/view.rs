use super::model::{Event, Kind, Machine, Phase, Setup};
use crate::frame::sign_in::card::{body, framed, header, wordmark};
use crate::ui::chrome::card::{Surfaces, hex, key_face};
use crate::ui::kit::surface::{self, Tone, tint};
use crate::ui::kit::{
    Button, ButtonTone, Density, IconSize, Radius, Row, RowItem, Space, Spaced, Surface,
    TextRole,
};
use crate::ui::theme::{ArtisanTheme, Oklch};
use aqua_core::DisplayItem;
use aqua_ui::element::{Box as UiBox, Canvas, Element, Text};
use aqua_ui::style::{Length, Style};
use aqua_ui::{AlignItems, JustifyContent};
use std::rc::Rc;
use std::sync::Arc;

pub const WINDOW: (f64, f64) = (480.0, 640.0);
pub const SPIN_STEPS: u32 = 24;
const WIDTH: f32 = 400.0;
const ROOMY_ROWS: usize = 4;

pub struct Props<'a> {
    pub setup: &'a Setup,
    pub spin: u32,
    pub emit: Rc<dyn Fn(Event)>,
}

#[must_use]
pub fn screen(theme: &ArtisanTheme, props: &Props<'_>) -> Element {
    let content = match &props.setup.phase {
        Phase::Choosing => choosing(theme, props),
        Phase::Installing { line } => vec![working(theme, props, line)],
        Phase::Failed { message } => failed(theme, props, message),
        Phase::Done => vec![wrapped(TextRole::Label.text(theme, "Opening Artisan Editor…"))],
    };
    let card = UiBox::new()
        .layout("column")
        .width(WIDTH)
        .child(framed(
            theme,
            header(AlignItems::END, vec![wordmark(theme), heading(theme, props.setup)]),
            vec![body(content).spaced(Space::S16).build()],
        ))
        .build()
        .with_key("artisan-setup-card");
    UiBox::new()
        .layout("column")
        .style(
            surface::frame(theme)
                .width(Length::Pct(100.0))
                .height(Length::Pct(100.0))
                .align(AlignItems::CENTER)
                .justify(JustifyContent::CENTER),
        )
        .style(TextRole::Label.inherit(theme))
        .child(card)
        .build()
}

fn heading(theme: &ArtisanTheme, setup: &Setup) -> Element {
    let (title, detail) = match &setup.kind {
        Kind::Fresh => ("Set up Artisan Editor", format!("Version {}", setup.version)),
        Kind::Update { from } => ("Update Artisan Editor", format!("{from} to {}", setup.version)),
    };
    UiBox::new()
        .layout("column")
        .spaced(Space::S4)
        .style(
            Style::new()
                .flex_grow(1.0)
                .min_width(Length::Pct(0.0))
                .align(AlignItems::END),
        )
        .child(wrapped(TextRole::Heading.text(theme, title)))
        .child(muted(theme, TextRole::Caption, &detail))
        .build()
}

fn choosing(theme: &ArtisanTheme, props: &Props<'_>) -> Vec<Element> {
    let change = Rc::clone(&props.emit);
    let begin = Rc::clone(&props.emit);
    let folder = UiBox::new()
        .layout("column")
        .style(Style::new().flex_grow(1.0).min_width(Length::Pct(0.0)))
        .child(wrapped(
            TextRole::Label.text(theme, props.setup.location.display().to_string()),
        ))
        .build();
    let location = UiBox::new()
        .layout("row")
        .spaced(Space::S12)
        .style(Style::new().align(AlignItems::CENTER))
        .child(folder)
        .children(props.setup.relocatable.then(|| {
            Button::text("artisan-setup-change", *theme, ButtonTone::Neutral, "Change…")
                .on_activate(move |_| change(Event::ChangeLocation))
                .build()
        }))
        .build();
    let roomy = props.setup.machines.len() <= ROOMY_ROWS;
    let machines = UiBox::new()
        .layout("column")
        .spaced(if roomy { Space::S8 } else { Space::S4 })
        .children(props.setup.machines.iter().enumerate().map(|(index, machine)| {
            machine_row(
                theme,
                machine,
                index,
                index == props.setup.selected,
                roomy,
                &props.emit,
            )
        }))
        .build();
    let install = UiBox::new()
        .layout("row")
        .style(Style::new().justify(JustifyContent::END))
        .child(
            Button::text("artisan-setup-install", *theme, ButtonTone::Primary, "Install")
                .on_activate(move |_| begin(Event::Started))
                .build(),
        )
        .build();
    vec![
        section(theme, "Install location", location),
        section(theme, "Where should agents work?", machines),
        install,
    ]
}

fn section(theme: &ArtisanTheme, label: &str, content: Element) -> Element {
    UiBox::new()
        .layout("column")
        .spaced(Space::S8)
        .child(muted(theme, TextRole::Caption, label))
        .child(content)
        .build()
}

fn machine_row(
    theme: &ArtisanTheme,
    machine: &Machine,
    index: usize,
    selected: bool,
    roomy: bool,
    emit: &Rc<dyn Fn(Event)>,
) -> Element {
    let pick = Rc::clone(emit);
    let row = match roomy {
        true => Row::ListItem(Density::Tall),
        false => Row::ListItem(Density::Regular),
    };
    let radius = Radius::Field.px();
    let face = UiBox::new()
        .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
        .child(key_face(
            &Surfaces::of(theme),
            radius,
            row.height().unwrap_or_default(),
        ))
        .build()
        .exclude_from_hit_testing();
    let item = RowItem::new(
        format!("artisan-setup-machine-{index}"),
        *theme,
        row,
        Surface::Inset,
        machine.title(),
    )
    .nested(radius, 0.0)
    .underlay(Some(face))
    .leading(radio(theme, selected))
    .selected(selected)
    .on_activate(move || pick(Event::Selected(index)));
    match roomy {
        true => item.detail(machine.detail()),
        false => item,
    }
    .build()
}

fn radio(theme: &ArtisanTheme, selected: bool) -> Element {
    let ink = theme.colors.text;
    let ring = if selected { ink } else { theme.colors.text_muted };
    let dot = if selected {
        format!(r#"<circle cx="8" cy="8" r="3.5" fill="{}"/>"#, hex(ink))
    } else {
        String::new()
    };
    picture(
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><circle cx="8" cy="8" r="6.75" fill="none" stroke="{}" stroke-width="1.5"/>{dot}</svg>"#,
            hex(ring)
        ),
        IconSize::Md,
    )
}

fn spinner(theme: &ArtisanTheme, spin: u32) -> Element {
    picture(
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 30 30" fill="none" stroke="{ink}" stroke-width="3" stroke-linecap="round"><circle cx="15" cy="15" r="8.5" stroke-opacity="0.18"/><path transform="rotate({angle} 15 15)" d="M15 6.5a8.5 8.5 0 0 1 8.5 8.5"/></svg>"#,
            ink = hex(theme.colors.text),
            angle = spin % SPIN_STEPS * 360 / SPIN_STEPS,
        ),
        IconSize::Lg,
    )
}

fn picture(svg: String, size: IconSize) -> Element {
    let source: Arc<str> = svg.into();
    Canvas::new()
        .style(
            Style::new()
                .width(Length::Px(size.px()))
                .height(Length::Px(size.px()))
                .flex_shrink(0.0),
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
        .exclude_from_hit_testing()
}

fn working(theme: &ArtisanTheme, props: &Props<'_>, line: &str) -> Element {
    let title = match props.setup.kind {
        Kind::Fresh => "Installing…",
        Kind::Update { .. } => "Updating…",
    };
    let detail = if line.is_empty() { "Getting ready" } else { line };
    UiBox::new()
        .layout("row")
        .spaced(Space::S12)
        .style(Style::new().align(AlignItems::START))
        .child(spinner(theme, props.spin))
        .child(
            UiBox::new()
                .layout("column")
                .spaced(Space::S4)
                .style(Style::new().flex_grow(1.0).min_width(Length::Pct(0.0)))
                .child(wrapped(TextRole::LabelStrong.text(theme, title)))
                .child(muted(theme, TextRole::Caption, detail))
                .build(),
        )
        .build()
}

fn failed(theme: &ArtisanTheme, props: &Props<'_>, message: &str) -> Vec<Element> {
    let retry = Rc::clone(&props.emit);
    vec![
        wrapped(TextRole::LabelStrong.text(theme, "Setup couldn't finish")),
        colored(TextRole::Label.text(theme, message.to_owned()), tint(theme, Tone::Danger)),
        UiBox::new()
            .layout("row")
            .style(Style::new().justify(JustifyContent::END))
            .child(
                Button::text("artisan-setup-retry", *theme, ButtonTone::Neutral, "Try again")
                    .on_activate(move |_| retry(Event::Started))
                    .build(),
            )
            .build(),
    ]
}

fn muted(theme: &ArtisanTheme, role: TextRole, words: &str) -> Element {
    colored(role.text(theme, words.to_owned()), theme.colors.text_muted)
}

fn colored(text: Text, color: Oklch) -> Element {
    wrapped(text.gpui_source_color(color.to_reference_text_color()))
}

fn wrapped(text: Text) -> Element {
    text.wrap(true).build()
}
