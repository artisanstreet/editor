use super::model::{Choice, Choices, Discovery, Job, Location, Machine, Setup, State};
use crate::frame::sign_in::card::{body, framed, header, wordmark};
use crate::ui::chrome::card::{hex, key_face};
use crate::ui::kit::surface;
use crate::ui::kit::{
    Button, ButtonTone, Density, IconSize, Radius, Row, RowItem, Space, Spaced, Surface, TextRole,
};
use crate::ui::theme::{ArtisanTheme, Oklch};
use aqua_core::DisplayItem;
use aqua_ui::element::{Box as UiBox, Canvas, Element, Scroll, Text};
use aqua_ui::style::{Length, Style};
use aqua_ui::{AlignItems, JustifyContent};
use std::rc::Rc;
use std::sync::Arc;

pub const WINDOW: (f64, f64) = (480.0, 640.0);
const WIDTH: f32 = 400.0;
const ROOMY_ROWS: usize = 4;
const VISIBLE_ROWS: u16 = 8;

pub struct Props<'a> {
    pub setup: &'a Setup,
    pub reduced_motion: bool,
    pub emit: Rc<dyn Fn(Choice)>,
}

impl std::fmt::Debug for Props<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Props").finish_non_exhaustive()
    }
}

#[must_use]
pub fn screen(theme: &ArtisanTheme, props: &Props<'_>) -> Element {
    let content = match props.setup.state() {
        State::Choosing(choices) => choosing(theme, props, choices),
        State::Installing { job, line } => vec![working(theme, props, job, line)],
        State::Cancelling { .. } => vec![stopping(theme, props)],
        State::Failed { job, error } => failed(
            theme,
            props,
            job,
            &crate::core::Problem::caused_by("Setup couldn't finish.", error),
        ),
        State::Done { .. } => {
            vec![wrapped(
                TextRole::Label.text(theme, "Opening Artisan Editor…"),
            )]
        }
    };
    let card = UiBox::new()
        .layout("column")
        .width(WIDTH)
        .child(framed(
            theme,
            header(
                AlignItems::END,
                vec![wordmark(theme), heading(theme, props.setup)],
            ),
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
        .aria_language(crate::shell::host::INTERFACE_LANGUAGE)
}

fn heading(theme: &ArtisanTheme, setup: &Setup) -> Element {
    let version = setup.version();
    let (title, detail) = match setup.state() {
        State::Choosing(_) => ("Set up Artisan Editor", format!("Version {version}")),
        State::Installing { job, .. }
        | State::Cancelling { job }
        | State::Failed { job, .. }
        | State::Done { job } => match job {
            Job::Fresh(_) => ("Set up Artisan Editor", format!("Version {version}")),
            Job::Update { from } => ("Update Artisan Editor", format!("{from} to {version}")),
        },
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
        .child(wrapped(TextRole::Heading.text(theme, title)).role(aqua_core::Role::Heading(1)))
        .child(muted(theme, TextRole::Caption, &detail))
        .build()
}

fn choosing(theme: &ArtisanTheme, props: &Props<'_>, choices: &Choices) -> Vec<Element> {
    let change = Rc::clone(&props.emit);
    let begin = Rc::clone(&props.emit);
    let folder = UiBox::new()
        .layout("column")
        .style(Style::new().flex_grow(1.0).min_width(Length::Pct(0.0)))
        .child(wrapped(
            TextRole::Label.text(theme, choices.location().path().display().to_string()),
        ))
        .build();
    let location = UiBox::new()
        .layout("row")
        .spaced(Space::S12)
        .style(Style::new().align(AlignItems::CENTER))
        .child(folder)
        .children(
            matches!(choices.location(), Location::Movable { .. }).then(|| {
                Button::text(
                    "artisan-setup-change",
                    *theme,
                    ButtonTone::Neutral,
                    "Change…",
                )
                .on_activate(move |_| change(Choice::ChangeLocation))
                .build()
            }),
        )
        .build();
    let roomy = choices.machines().len() <= ROOMY_ROWS;
    let gap = if roomy { Space::S8 } else { Space::S4 };
    let rows = UiBox::new()
        .layout("column")
        .spaced(gap)
        .children(
            choices
                .machines()
                .iter()
                .enumerate()
                .map(|(index, machine)| {
                    machine_row(
                        theme,
                        machine,
                        index,
                        machine == choices.selected(),
                        roomy,
                        &props.emit,
                    )
                }),
        )
        .build();
    let row_height = Row::ListItem(Density::Regular)
        .height()
        .expect("invariant: list rows have a fixed height");
    let visible = f32::from(VISIBLE_ROWS);
    let machines = Scroll::new()
        .layout("column")
        .preserve_offset(true)
        .style(
            Style::new()
                .max_height(Length::Px(
                    gap.px().mul_add(visible - 1.0, row_height * visible),
                ))
                .flex_shrink(0.0),
        )
        .child(rows)
        .build()
        .with_key("artisan-setup-machines");
    let install = UiBox::new()
        .layout("row")
        .style(Style::new().justify(JustifyContent::END))
        .child(
            Button::text(
                "artisan-setup-install",
                *theme,
                ButtonTone::Primary,
                "Install",
            )
            .on_activate(move |_| begin(Choice::Started))
            .build(),
        )
        .build();
    let machines = UiBox::new()
        .layout("column")
        .spaced(Space::S8)
        .child(machines)
        .children(discovery(theme, props, choices.discovery()))
        .build();
    vec![
        section(theme, "Install location", location),
        section(theme, "Where should agents work?", machines),
        install,
    ]
}

fn discovery(theme: &ArtisanTheme, props: &Props<'_>, discovery: &Discovery) -> Option<Element> {
    match discovery {
        Discovery::Found => None,
        Discovery::Searching => Some(muted(
            theme,
            TextRole::Caption,
            "Looking for WSL distributions…",
        )),
        Discovery::Failed(reason) => {
            let retry = Rc::clone(&props.emit);
            Some(
                crate::ui::kit::ProblemNotice::new(
                    "artisan-setup-distros-problem",
                    *theme,
                    &crate::core::Problem::detailed(
                        "Couldn't list WSL distributions.",
                        reason.clone(),
                    ),
                )
                .action(
                    Button::text(
                        "artisan-setup-find-distros",
                        *theme,
                        ButtonTone::Neutral,
                        "Look again",
                    )
                    .on_activate(move |_| retry(Choice::FindDistros))
                    .build(),
                )
                .build(),
            )
        }
    }
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
    emit: &Rc<dyn Fn(Choice)>,
) -> Element {
    let pick = Rc::clone(emit);
    let row = if roomy {
        Row::ListItem(Density::Tall)
    } else {
        Row::ListItem(Density::Regular)
    };
    let radius = Radius::Field.px();
    let face = UiBox::new()
        .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
        .child(key_face(theme, radius, row.height().unwrap_or_default()))
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
    .face(face)
    .leading(radio(theme, selected))
    .selected(selected)
    .on_activate(move || pick(Choice::Selected(index)));
    if roomy {
        item.detail(machine.detail())
    } else {
        item
    }
    .build()
}

fn radio(theme: &ArtisanTheme, selected: bool) -> Element {
    let ink = theme.colors.text;
    let ring = if selected {
        ink
    } else {
        theme.colors.text_muted
    };
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

fn spinner(theme: &ArtisanTheme, reduced_motion: bool) -> Element {
    let arc = picture(
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 30 30" fill="none" stroke="{ink}" stroke-width="3" stroke-linecap="round"><circle cx="15" cy="15" r="8.5" stroke-opacity="0.18"/><path d="M15 6.5a8.5 8.5 0 0 1 8.5 8.5"/></svg>"#,
            ink = hex(theme.colors.text),
        ),
        IconSize::Lg,
    );
    crate::ui::motion::WorkingPulse::default().wrap(arc, reduced_motion)
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

fn working(theme: &ArtisanTheme, props: &Props<'_>, job: &Job, line: &str) -> Element {
    let title = match job {
        Job::Fresh(_) => "Installing…",
        Job::Update { .. } => "Updating…",
    };
    let detail = if line.is_empty() {
        "Getting ready"
    } else {
        line
    };
    progress(theme, props, title, detail)
}

fn stopping(theme: &ArtisanTheme, props: &Props<'_>) -> Element {
    progress(
        theme,
        props,
        "Stopping the installer…",
        "Setup closes once it has stopped",
    )
}

fn progress(theme: &ArtisanTheme, props: &Props<'_>, title: &str, detail: &str) -> Element {
    UiBox::new()
        .layout("row")
        .spaced(Space::S12)
        .style(Style::new().align(AlignItems::START))
        .child(spinner(theme, props.reduced_motion))
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

fn failed(
    theme: &ArtisanTheme,
    props: &Props<'_>,
    job: &Job,
    problem: &crate::core::Problem,
) -> Vec<Element> {
    let retry = Rc::clone(&props.emit);
    let back = Rc::clone(&props.emit);
    let mut notice = crate::ui::kit::ProblemNotice::new("artisan-setup-problem", *theme, problem);
    if matches!(job, Job::Fresh(_)) {
        notice = notice.action(
            Button::text("artisan-setup-back", *theme, ButtonTone::Neutral, "Back")
                .on_activate(move |_| back(Choice::Back))
                .build(),
        );
    }
    vec![
        notice
            .action(
                Button::text(
                    "artisan-setup-retry",
                    *theme,
                    ButtonTone::Neutral,
                    "Try again",
                )
                .on_activate(move |_| retry(Choice::Started))
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
