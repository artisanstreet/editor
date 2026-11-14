use super::{Entry, Inspector};
use aqua_text::StyledSpan;
use aqua_text::overflow::OverflowMode;
use aqua_ui::{
    AlignItems, JustifyContent,
    collection::List,
    element::{Box as UiBox, Element, Text},
    style::{Length, Style},
};
use artisan_api::{Collection, Status, StepStatus, TextContent};
use editor_assets::AssetId;
use editor_core::Core;
use editor_ui::kit::{
    Button, ButtonTone, Density, Divider, IconSize, Row, RowItem, Space, Spaced, Surface,
    TextRole, Tooltip,
};
use editor_ui::theme::{ArtisanTheme, Oklch};
use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc};

const LIST: Surface = Surface::Inset;
const ROW: Row = Row::ListItem(Density::Regular);
const HEADING: Row = Row::ListHeading;
const CARD: Surface = Surface::Tooltip(Tooltip::Card);
const CARD_WIDTH: f32 = 320.0;
const VALUE_WIDTH: f32 = 150.0;
const DOT: f32 = 6.0;
const PULSE_MS: u32 = 1_600;
const PULSE_FLOOR: f32 = 0.4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InspectorAction {
    Retry,
    OpenAgent {
        id: artisan_api::ItemId,
        thread: Option<artisan_api::ThreadId>,
    },
    OpenPlan,
    OpenStep(artisan_api::ContentRef),
    OpenTerminal(artisan_api::ItemId),
}

fn muted_glyph(theme: &ArtisanTheme, icon: AssetId) -> Element {
    IconSize::Md.glyph(icon, theme.colors.text_muted.to_reference_paint())
}

fn fit(text: Text) -> Element {
    text.overflow(OverflowMode::EndEllipsis)
        .style(Style::new().min_width(Length::Pct(0.0)).flex_shrink(1.0))
        .build()
}

fn inked(theme: &ArtisanTheme, role: TextRole, words: impl Into<String>, ink: Oklch) -> Text {
    role.text(theme, words.into())
        .gpui_source_color(ink.to_reference_text_color())
}

fn static_row(children: Vec<Element>) -> Element {
    UiBox::new()
        .width(Length::Pct(100.0))
        .height(ROW.height().unwrap_or_default())
        .padded(ROW.pad().x)
        .spaced(ROW.gap())
        .style(Style::new().min_width(Length::Pct(0.0)).align(AlignItems::CENTER))
        .children(children)
        .build()
}

#[expect(
    clippy::disallowed_methods,
    reason = "the agent status dot is a painted status mark, not chrome"
)]
fn status_dot(color: Oklch, pulse: Option<u32>) -> Element {
    let mut style = Style::new().gpui_background(aqua_core::GpuiBackground::Solid(
        color.to_reference_paint(),
    ));
    if let Some(start_ms) = pulse {
        style = style.gpui_quad_pulse(aqua_core::scene::GpuiQuadPulse {
            start_ms,
            period_ms: PULSE_MS,
            min_opacity: PULSE_FLOOR,
        });
    }
    let edge = IconSize::Md.px();
    UiBox::new()
        .width(edge)
        .height(edge)
        .style(
            Style::new()
                .flex_shrink(0.0)
                .align(AlignItems::CENTER)
                .justify(JustifyContent::CENTER),
        )
        .child(
            UiBox::new()
                .width(DOT)
                .height(DOT)
                .radius(DOT / 2.0)
                .style(style)
                .build(),
        )
        .build()
}

#[derive(Clone)]
pub struct AgentCard {
    engine: Option<artisan_api::EngineId>,
    model: Option<String>,
    settings: Vec<(AssetId, String)>,
    purpose: String,
}

fn effort_label(word: &str) -> String {
    if let Some(level) = artisan_api::ThinkingLevel::from_native(word) {
        return level.label().into();
    }
    let mut output = String::new();
    for (index, part) in word.split(['_', '-']).enumerate() {
        if index > 0 {
            output.push(' ');
        }
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            if index == 0 {
                output.extend(first.to_uppercase());
            } else {
                output.push(first);
            }
            output.push_str(chars.as_str());
        }
    }
    output
}

pub fn agent_card(
    agent: &artisan_api::InspectorAgent,
    host: Option<&editor_core::HostData>,
) -> AgentCard {
    let config = agent.config.as_ref();
    let engine = config.map(|config| config.engine).or(agent.engine);
    let id = config
        .and_then(|config| config.model.clone())
        .or_else(|| agent.model.clone());
    let model = host.and_then(|host| {
        host.engines
            .get(&engine?)?
            .models
            .iter()
            .find(|model| Some(&model.id) == id.as_ref())
    });
    let mut settings = Vec::new();
    if let Some(effort) = config
        .and_then(|config| config.effort.clone())
        .or_else(|| model.and_then(|model| model.default_effort.clone()))
    {
        settings.push((AssetId::TABLER_BRAIN, effort_label(&effort)));
    }
    if let Some(config) = config {
        let speeds = model
            .map(|model| model.speeds.as_slice())
            .unwrap_or_default();
        let speed = speeds
            .iter()
            .find(|speed| {
                config
                    .speed
                    .as_ref()
                    .map_or(speed.default, |id| &speed.id == id)
            })
            .map_or_else(|| "Normal".to_owned(), |speed| artisan_api::SpeedLevel::from_native(&speed.id).map_or_else(|| speed.label.clone(), |level| level.label().into()));
        settings.push((AssetId::TABLER_BOLT_FILLED, speed));
    }
    if let Some(tokens) = config
        .and_then(artisan_api::EngineConfig::context_window_override)
        .or_else(|| model.and_then(|model| model.context_window))
    {
        let label = if tokens >= 1_000_000 && tokens.is_multiple_of(1_000_000) {
            format!("{}M", tokens / 1_000_000)
        } else {
            format!("{}K", tokens / 1_000)
        };
        settings.push((AssetId::TABLER_ARROWS_HORIZONTAL, label));
    }
    if let Some(config) = config {
        settings.push((
            AssetId::TABLER_LOCK,
            match config.permissions {
                artisan_api::Permissions::Auto => "Auto",
                artisan_api::Permissions::Full => "Full access",
            }
            .into(),
        ));
    }
    AgentCard {
        engine,
        model: model.map(|model| model.name.clone()).or(id),
        settings,
        purpose: match &agent.description {
            TextContent::Inline(text) => text.trim().to_owned(),
            TextContent::Stored(_) => "Open the agent to read its task.".into(),
        },
    }
}

pub fn render_agent_card(card: &AgentCard, theme: ArtisanTheme) -> Element {
    let icon = card.engine.map_or(AssetId::TABLER_BOT_ID, |engine| {
        editor_ui::engine::engine_icon(&theme, engine)
    });
    let title = card
        .model
        .clone()
        .or_else(|| card.engine.map(|engine| engine.display_name().to_owned()))
        .unwrap_or_else(|| "Agent".into());
    let mut content = UiBox::new().layout("column").spaced(Space::S12).child(
        UiBox::new()
            .spaced(Space::S8)
            .style(Style::new().align(AlignItems::CENTER))
            .child(IconSize::Md.glyph(icon, theme.colors.text.to_reference_paint()))
            .child(fit(TextRole::Title.text(&theme, title)))
            .build(),
    );
    if !card.settings.is_empty() {
        let mut settings = UiBox::new().layout("column").spaced(Space::S4);
        for (icon, value) in &card.settings {
            settings = settings.child(
                UiBox::new()
                    .spaced(Space::S8)
                    .style(Style::new().align(AlignItems::CENTER))
                    .child(
                        IconSize::Md
                            .glyph(*icon, theme.colors.text_muted_glass.to_reference_paint()),
                    )
                    .child(TextRole::Caption.text(&theme, value.clone()).build())
                    .build(),
            );
        }
        content = content.child(settings.build());
    }
    if !card.purpose.is_empty() {
        content = content.child(
            inked(
                &theme,
                TextRole::Caption,
                card.purpose.clone(),
                theme.colors.text_muted_glass,
            )
            .wrap(true)
            .build(),
        );
    }
    CARD.build(&theme, vec![content.build()])
        .style(Style::new().max_width(Length::Px(CARD_WIDTH)))
}

fn fact_row(
    theme: &ArtisanTheme,
    label: &str,
    values: &[Option<String>; 4],
    edits: Option<&artisan_api::EditTotals>,
) -> Element {
    let (icon, title, value) = match label {
        "Project" => (AssetId::TABLER_FOLDER, "Project", values[0].clone()),
        "Forge" => (AssetId::TABLER_DEVICE_LAPTOP, "Machine", values[1].clone()),
        "Branch" => (AssetId::TABLER_GIT_BRANCH, "Branch", values[2].clone()),
        "Worktree" => (AssetId::TABLER_FOLDER_CODE, "Worktree", values[3].clone()),
        _ => (AssetId::TABLER_FILE_DIFF, "Changes", None),
    };
    let mut children = vec![
        muted_glyph(theme, icon),
        UiBox::new()
            .style(Style::new().flex_grow(1.0).min_width(Length::Pct(0.0)))
            .child(fit(TextRole::Label.text(theme, title)))
            .build(),
    ];
    match (label, edits) {
        ("Edit operations", Some(edits)) => {
            children.push(
                inked(
                    theme,
                    TextRole::Label,
                    format!("+{}", edits.added),
                    theme.colors.green.text,
                )
                .build(),
            );
            children.push(
                inked(
                    theme,
                    TextRole::Label,
                    format!("−{}", edits.removed),
                    theme.colors.red.text,
                )
                .build(),
            );
        }
        ("Edit operations", None) => {}
        _ => children.push(
            UiBox::new()
                .clip(true)
                .style(
                    Style::new()
                        .min_width(Length::Pct(0.0))
                        .max_width(Length::Px(VALUE_WIDTH)),
                )
                .child(fit(TextRole::Label.text(
                    theme,
                    value.unwrap_or_else(|| "—".into()),
                )))
                .build(),
        ),
    }
    static_row(children)
}

fn agent_row(
    theme: ArtisanTheme,
    agent: &artisan_api::InspectorAgent,
    card: AgentCard,
    pulse: Option<u32>,
    action: Rc<dyn Fn(InspectorAction)>,
) -> Element {
    let description = match &agent.description {
        TextContent::Inline(value) => value.lines().next().unwrap_or_default().to_owned(),
        TextContent::Stored(_) => "Stored description. Open agent to read.".into(),
    };
    let color = match agent.status {
        Status::Running | Status::AwaitingUser => theme.colors.text,
        Status::Done => theme.colors.unread,
        Status::Failed => theme.colors.red.solid,
        Status::Declined | Status::Cancelled => theme.colors.text_muted,
    };
    let icon = agent.engine.map_or(AssetId::TABLER_BOT_ID, |engine| {
        editor_ui::engine::engine_icon(&theme, engine)
    });
    let body = UiBox::new()
        .width(Length::Pct(100.0))
        .spaced(ROW.gap())
        .style(Style::new().align(AlignItems::CENTER))
        .child(fit(TextRole::Label.text(&theme, agent.label.clone())))
        .child(
            UiBox::new()
                .clip(true)
                .style(Style::new().flex_grow(1.0).min_width(Length::Pct(0.0)))
                .child(fit(inked(
                    &theme,
                    TextRole::Label,
                    description,
                    theme.colors.text_muted,
                )))
                .build(),
        )
        .build();
    let id = agent.id.clone();
    let thread = agent.thread.clone();
    RowItem::new(
        format!("inspector-agent-{}", agent.id),
        theme,
        ROW,
        LIST,
        agent.label.clone(),
    )
    .leading(muted_glyph(&theme, icon))
    .body(body)
    .trailing(status_dot(color, pulse))
    .on_activate(move || {
        action(InspectorAction::OpenAgent {
            id: id.clone(),
            thread: thread.clone(),
        })
    })
    .hint(move || render_agent_card(&card, theme))
    .build()
}

fn plan_row(
    theme: ArtisanTheme,
    index: usize,
    step: &artisan_api::PlanStep,
    action: Rc<dyn Fn(InspectorAction)>,
) -> Element {
    match &step.text {
        TextContent::Inline(value) => {
            let resolved = matches!(step.status, StepStatus::Done | StepStatus::Cancelled);
            let ink = match resolved {
                true => theme.colors.text_muted,
                false => theme.colors.text,
            };
            let mut struck = TextRole::Label.style(&theme);
            struck.strikethrough = resolved;
            static_row(vec![fit(
                inked(&theme, TextRole::Label, value.clone(), ink)
                    .spans(vec![StyledSpan::new(value.clone(), struck)]),
            )])
        }
        TextContent::Stored(source) => {
            let source = source.clone();
            let status = match step.status {
                StepStatus::Pending => "○",
                StepStatus::InProgress => "●",
                StepStatus::Done => "✓",
                StepStatus::Cancelled => "−",
            };
            Button::text(
                format!("inspector-plan-{index}"),
                theme,
                ButtonTone::Quiet,
                format!("{status} Read step"),
            )
            .within(LIST)
            .on_activate(move |_| action(InspectorAction::OpenStep(source.clone())))
            .build()
        }
    }
}

fn terminal_row(
    theme: ArtisanTheme,
    terminal: &artisan_api::InspectorTerminal,
    action: Rc<dyn Fn(InspectorAction)>,
) -> Element {
    let label = match &terminal.command {
        TextContent::Inline(value) => value
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("Shell")
            .to_owned(),
        TextContent::Stored(_) => "Stored shell command".into(),
    };
    let icon = match terminal.watch {
        true => AssetId::TABLER_EYE,
        false => AssetId::TABLER_TERMINAL_2,
    };
    let item = terminal.id.clone();
    RowItem::new(
        format!("inspector-terminal-{}", terminal.id),
        theme,
        ROW,
        LIST,
        label.clone(),
    )
    .leading(muted_glyph(&theme, icon))
    .body(fit(TextRole::Code.text(&theme, label)))
    .on_activate(move || action(InspectorAction::OpenTerminal(item.clone())))
    .build()
}

pub fn inspector_view(
    core: &Core,
    inspector: &Inspector,
    theme: ArtisanTheme,
    source: Option<Element>,
    plan: Option<Element>,
    reduced_motion: bool,
    pulse_starts: Rc<RefCell<HashMap<usize, u32>>>,
    action: Rc<dyn Fn(InspectorAction)>,
) -> Result<Element, &'static str> {
    if inspector.source.is_some() && source.is_none() {
        return Err("inspector source requires its source view");
    }
    if inspector.plan.is_some() && plan.is_none() {
        return Err("inspector checklist requires its collection view");
    }
    let host = inspector
        .open
        .as_ref()
        .and_then(|(host, _)| core.host(host));
    let project = host.and_then(|host| {
        inspector
            .open
            .as_ref()
            .and_then(|(_, thread)| host.threads.get(thread))
            .and_then(|thread| host.projects.get(&thread.project_id))
    });
    let values = [
        project.map(|project| project.name.clone()),
        host.map(|host| host.name.clone()),
        project.and_then(|project| host?.repository(&project.id)?.branch.clone()),
        project.and_then(|project| host?.repository(&project.id)?.worktree.clone()),
    ];
    let cards: Vec<_> = inspector
        .summary
        .agents
        .iter()
        .map(|agent| agent_card(agent, host))
        .collect();
    let entries = inspector.entries.clone();
    let rows = entries.clone();
    let summary = Arc::clone(&inspector.summary);
    let bleed = LIST.padding_px();
    let frame = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0));
    let mut root = LIST
        .padding()
        .into_iter()
        .fold(frame, Spaced::padded)
        .style(
            Style::new()
                .min_height(Length::Pct(0.0))
                .gpui_source_text_color(theme.colors.text.to_reference_text_color()),
        );
    if let Some(error) = &inspector.error {
        let retry = Rc::clone(&action);
        root = root
            .child(static_row(vec![fit(TextRole::Label.text(&theme, error.clone()))]))
            .child(
                Button::text("inspector-retry", theme, ButtonTone::Neutral, "Retry")
                    .within(LIST)
                    .on_activate(move |_| retry(InspectorAction::Retry))
                    .build(),
            );
    }
    let row_action = Rc::clone(&action);
    let list = List::new()
        .items(entries.into_iter().enumerate())
        .key(|(index, _)| *index)
        .row(move |(index, entry)| {
            let content = match entry {
                Entry::Heading(label) => {
                    RowItem::new(format!("inspector-heading-{label}"), theme, HEADING, LIST, *label)
                        .build()
                }
                Entry::Fact(label) => fact_row(&theme, label, &values, summary.edits.as_ref()),
                Entry::Agent(index) => {
                    let agent = &summary.agents[*index];
                    let pulse = (agent.status == Status::Running && !reduced_motion).then(|| {
                        *pulse_starts
                            .borrow_mut()
                            .entry(*index)
                            .or_insert_with(editor_ui::clock::pulse_ms)
                    });
                    agent_row(
                        theme,
                        agent,
                        cards[*index].clone(),
                        pulse,
                        Rc::clone(&row_action),
                    )
                }
                Entry::PlanCollection => {
                    let callback = Rc::clone(&row_action);
                    Button::text(
                        "inspector-checklist",
                        theme,
                        ButtonTone::Quiet,
                        format!("Read {} steps", summary.plan.len()),
                    )
                    .within(LIST)
                    .on_activate(move |_| callback(InspectorAction::OpenPlan))
                    .build()
                }
                Entry::Plan(index) => match &summary.plan {
                    Collection::Inline(steps) => steps.get(*index).map_or_else(
                        || static_row(Vec::new()),
                        |step| plan_row(theme, *index, step, Rc::clone(&row_action)),
                    ),
                    Collection::Stored(_) => static_row(Vec::new()),
                },
                Entry::Terminal(index) => terminal_row(
                    theme,
                    &summary.terminals[*index],
                    Rc::clone(&row_action),
                ),
                Entry::Workflow(index) => {
                    let workflow = &summary.workflows[*index];
                    static_row(vec![
                        muted_glyph(&theme, AssetId::TABLER_SITEMAP),
                        fit(TextRole::Label.text(
                            &theme,
                            workflow.name.clone().unwrap_or_else(|| "Workflow".into()),
                        )),
                    ])
                }
            };
            let closes = rows
                .get(index + 1)
                .is_none_or(|entry| matches!(entry, Entry::Heading(_)));
            match closes {
                true => UiBox::new()
                    .layout("column")
                    .width(Length::Pct(100.0))
                    .child(content)
                    .child(Divider::Rule.build(&theme, bleed))
                    .build(),
                false => content,
            }
        })
        .virtualize()
        .style(Style::new().flex_grow(1.0).min_height(Length::Pct(0.0)))
        .build();
    root = root.child(list);
    for panel in [source, plan].into_iter().flatten() {
        root = root
            .child(
                UiBox::new()
                    .width(Length::Pct(100.0))
                    .padded(ROW.pad().x)
                    .child(panel)
                    .build(),
            )
            .child(Divider::Rule.build(&theme, bleed));
    }
    Ok(root.build())
}
