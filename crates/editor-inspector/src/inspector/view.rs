use super::{Entry, Inspector};
use editor_core::Core;
use editor_ui::{
    asset::source_asset_glyph,
    button::Button,
    theme::{ArtisanTheme, RadiusStep, RadiusTokens},
};
use aqua_core::{Insets, Role};
use aqua_text::{FontFamily, FontWeight, TextStyle};
use aqua_ui::{
    collection::List,
    element::{Box as UiBox, Button as UiButton, Element, Text},
    style::{Length, Style},
};
use artisan_api::{Collection, Status, StepStatus, TextContent};
use editor_assets::AssetId;
use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc};

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

fn glyph(theme: ArtisanTheme, icon: AssetId) -> Element {
    source_asset_glyph(icon, 16.0, theme.colors.text_muted.to_reference_paint())
}

fn row(theme: ArtisanTheme) -> UiBox {
    UiBox::new()
        .width(Length::Pct(100.0))
        .gap(8.0)
        .radius(RadiusTokens::value(RadiusStep::Md))
        .style(
            Style::new()
                .min_width(Length::Px(0.0))
                .padding_insets(Insets {
                    left: 8.0,
                    right: 8.0,
                    top: 8.0,
                    bottom: 8.0,
                })
                .align(aqua_ui::AlignItems::CENTER)
                .font_size(theme.typography.control_text),
        )
}

fn row_hover(theme: ArtisanTheme) -> Style {
    let (top, bottom) = editor_ui::hover::hover_fill_stops(&theme, theme.colors.text);
    Style::new().gpui_background(aqua::GpuiBackground::LinearGradient {
        angle: 180.0,
        color_space: aqua::GpuiGradientColorSpace::Oklab,
        stops: [
            aqua::GpuiGradientStop {
                percentage: 0.0,
                color: top.to_reference_paint(),
            },
            aqua::GpuiGradientStop {
                percentage: 1.0,
                color: bottom.to_reference_paint(),
            },
        ],
    })
}

fn text(theme: ArtisanTheme, value: impl Into<String>) -> Element {
    Text::new(value.into())
        .overflow(aqua_text::overflow::OverflowMode::EndEllipsis)
        .style(Style::new().min_width(Length::Px(0.0)).flex_shrink(1.0))
        .font_size(theme.typography.control_text)
        .gpui_source_color(theme.colors.text.to_reference_text_color())
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
    use editor_ui::glass::{GlassStrength, glass_card_shadows, glass_layers};
    use editor_ui::theme::{Elevation, Opacity};
    let icon = card.engine.map_or(AssetId::TABLER_BOT_ID, |engine| {
        editor_ui::engine::engine_icon(&theme, engine)
    });
    let title = card
        .model
        .clone()
        .or_else(|| card.engine.map(|engine| engine.display_name().to_owned()))
        .unwrap_or_else(|| "Agent".into());
    let font = TextStyle {
        size_px: theme.typography.control_text,
        line_height_px: 18.0,
        weight: FontWeight(600),
        ..TextStyle::default()
    };
    let mut content = UiBox::new().layout("column").gap(12.0).padding(16.0).child(
        UiBox::new()
            .gap(8.0)
            .style(Style::new().align(aqua_ui::AlignItems::CENTER))
            .child(source_asset_glyph(
                icon,
                16.0,
                theme.colors.text.to_reference_paint(),
            ))
            .child(
                Text::new(title)
                    .overflow(aqua_text::overflow::OverflowMode::EndEllipsis)
                    .style(Style::new().min_width(Length::Px(0.0)).flex_shrink(1.0))
                    .text_style(font)
                    .gpui_source_color(theme.colors.text.to_reference_text_color())
                    .build(),
            )
            .build(),
    );
    if !card.settings.is_empty() {
        let mut settings = UiBox::new().layout("column").gap(4.0);
        for (icon, value) in &card.settings {
            let font = TextStyle {
                size_px: theme.typography.label_text,
                line_height_px: 19.0,
                ..TextStyle::default()
            };
            settings = settings.child(
                UiBox::new()
                    .gap(8.0)
                    .style(Style::new().align(aqua_ui::AlignItems::CENTER))
                    .child(glyph(theme, *icon))
                    .child(
                        Text::new(value.clone())
                            .text_style(font)
                            .gpui_source_color(theme.colors.text.to_reference_text_color())
                            .build(),
                    )
                    .build(),
            );
        }
        content = content.child(settings.build());
    }
    if !card.purpose.is_empty() {
        let font = TextStyle {
            size_px: theme.typography.label_text,
            line_height_px: 19.0,
            ..TextStyle::default()
        };
        content = content.child(
            Text::new(card.purpose.clone())
                .text_style(font)
                .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
                .wrap(true)
                .build(),
        );
    }
    let radius = RadiusTokens::value(RadiusStep::X2l);
    UiBox::new()
        .radius(radius)
        .style(
            Style::new()
                .max_width(Length::Px(320.0))
                .border_of(1.0, theme.colors.text.at(Opacity::GlassBorder).to_paint())
                .shadows(
                    glass_card_shadows(&theme, Elevation::Overlay)
                        .into_iter()
                        .map(Into::into)
                        .collect(),
                ),
        )
        .child(glass_layers(&theme, GlassStrength::Strong, radius))
        .child(content.build())
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
    let mut root = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .style(
            Style::new()
                .min_height(Length::Px(0.0))
                .padding_insets(Insets {
                    top: 10.0,
                    bottom: 10.0,
                    left: 0.0,
                    right: 0.0,
                })
                .gpui_source_text_color(theme.colors.text.to_reference_text_color()),
        );
    if let Some(error) = &inspector.error {
        let retry = Rc::clone(&action);
        root = root
            .child(
                UiBox::new()
                    .style(Style::new().padding_insets(Insets {
                        left: 10.0,
                        right: 10.0,
                        top: 0.0,
                        bottom: 0.0,
                    }))
                    .child(text(theme, error.clone()))
                    .build(),
            )
            .child(
                Button::text("inspector-retry", theme, "Retry")
                    .on_activate(move |_| retry(InspectorAction::Retry))
                    .build(),
            );
    }
    let row_action = Rc::clone(&action);
    let list = List::new()
        .items(entries.into_iter().enumerate())
        .key(|(index, _)| *index)
        .row(move |(index, entry)| {
            let mut content = row(theme);
            match entry {
                Entry::Heading(label) => {
                    content = content
                        .style(Style::new().padding_insets(Insets {
                            left: 8.0,
                            right: 8.0,
                            top: 10.0,
                            bottom: 4.0,
                        }))
                        .child(
                            Text::new(*label)
                                .overflow(aqua_text::overflow::OverflowMode::EndEllipsis)
                                .style(Style::new().min_width(Length::Px(0.0)).flex_shrink(1.0))
                                .font_size(theme.typography.label_text)
                                .gpui_source_color(
                                    theme.colors.text_muted.to_reference_text_color(),
                                )
                                .build(),
                        )
                }
                Entry::Fact(label) => {
                    let (icon, title, value) = match *label {
                        "Project" => (AssetId::TABLER_FOLDER, "Project", values[0].clone()),
                        "Forge" => (AssetId::TABLER_DEVICE_LAPTOP, "Machine", values[1].clone()),
                        "Branch" => (AssetId::TABLER_GIT_BRANCH, "Branch", values[2].clone()),
                        "Worktree" => (AssetId::TABLER_FOLDER_CODE, "Worktree", values[3].clone()),
                        _ => (AssetId::TABLER_FILE_DIFF, "Changes", None),
                    };
                    content = content.child(glyph(theme, icon)).child(
                        UiBox::new()
                            .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                            .child(text(theme, title))
                            .build(),
                    );
                    if *label == "Edit operations" {
                        if let Some(edits) = &summary.edits {
                            content = content
                                .child(
                                    Text::new(format!("+{}", edits.added))
                                        .gpui_source_color(
                                            theme.colors.green.text.to_reference_text_color(),
                                        )
                                        .build(),
                                )
                                .child(
                                    Text::new(format!("−{}", edits.removed))
                                        .gpui_source_color(
                                            theme.colors.red.text.to_reference_text_color(),
                                        )
                                        .build(),
                                );
                        }
                    } else {
                        content = content.child(
                            UiBox::new()
                                .clip(true)
                                .style(
                                    Style::new()
                                        .min_width(Length::Px(0.0))
                                        .max_width(Length::Px(150.0)),
                                )
                                .child(text(theme, value.unwrap_or_else(|| "—".into())))
                                .build(),
                        );
                    }
                }
                Entry::Agent(index) => {
                    let agent = &summary.agents[*index];
                    let callback = Rc::clone(&row_action);
                    let index = *index;
                    let agent_id = agent.id.clone();
                    let agent_thread = agent.thread.clone();
                    let description = match &agent.description {
                        TextContent::Inline(value) => {
                            value.lines().next().unwrap_or_default().to_owned()
                        }
                        TextContent::Stored(_) => "Stored description. Open agent to read.".into(),
                    };
                    let color = match agent.status {
                        Status::Running | Status::AwaitingUser => theme.colors.text,
                        Status::Done => theme.colors.unread,
                        Status::Failed => theme.colors.red.solid,
                        Status::Declined | Status::Cancelled => theme.colors.text_muted,
                    };
                    let mut dot_style = Style::new().gpui_background(
                        aqua_core::GpuiBackground::Solid(color.to_reference_paint()),
                    );
                    if agent.status == Status::Running && !reduced_motion {
                        let start_ms = *pulse_starts
                            .borrow_mut()
                            .entry(index)
                            .or_insert_with(editor_ui::clock::pulse_ms);
                        dot_style = dot_style.gpui_quad_pulse(aqua_core::scene::GpuiQuadPulse {
                            start_ms,
                            period_ms: 1_600,
                            min_opacity: 0.4,
                        });
                    }
                    let dot = UiBox::new()
                        .width(6.0)
                        .height(6.0)
                        .radius(3.0)
                        .style(dot_style)
                        .build();
                    let icon = agent.engine.map_or(AssetId::TABLER_BOT_ID, |engine| {
                        editor_ui::engine::engine_icon(&theme, engine)
                    });
                    let inner = UiBox::new()
                        .width(Length::Pct(100.0))
                        .gap(8.0)
                        .style(Style::new().align(aqua_ui::AlignItems::CENTER))
                        .child(source_asset_glyph(
                            icon,
                            16.0,
                            theme.colors.text_muted.to_reference_paint(),
                        ))
                        .child(text(theme, agent.label.clone()))
                        .child(
                            UiBox::new()
                                .clip(true)
                                .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                                .child(
                                    Text::new(description)
                                        .overflow(aqua_text::overflow::OverflowMode::EndEllipsis)
                                        .style(
                                            Style::new()
                                                .min_width(Length::Px(0.0))
                                                .flex_shrink(1.0),
                                        )
                                        .gpui_source_color(
                                            theme.colors.text_muted.to_reference_text_color(),
                                        )
                                        .build(),
                                )
                                .build(),
                        )
                        .child(
                            UiBox::new()
                                .width(16.0)
                                .height(16.0)
                                .style(
                                    Style::new()
                                        .align(aqua_ui::AlignItems::CENTER)
                                        .justify(aqua_ui::JustifyContent::CENTER),
                                )
                                .child(dot)
                                .build(),
                        )
                        .build();
                    content = content.child(
                        UiButton::new()
                            .width(Length::Pct(100.0))
                            .on_activate(move || {
                                callback(InspectorAction::OpenAgent {
                                    id: agent_id.clone(),
                                    thread: agent_thread.clone(),
                                })
                            })
                            .child(inner)
                            .build()
                            .role(Role::Button)
                            .cursor(aqua_core::CursorIcon::Pointer)
                            .hover_style(row_hover(theme))
                            .aria_label(agent.label.clone())
                            .with_key(format!("inspector-agent-{}", agent.id))
                            .tooltip(0.5, {
                                let card = cards[index].clone();
                                move || render_agent_card(&card, theme)
                            }),
                    );
                }
                Entry::PlanCollection => {
                    let callback = Rc::clone(&row_action);
                    content = content.child(
                        Button::text(
                            "inspector-checklist",
                            theme,
                            format!("Read {} steps", summary.plan.len()),
                        )
                        .on_activate(move |_| callback(InspectorAction::OpenPlan))
                        .build(),
                    );
                }
                Entry::Plan(index) => {
                    if let Collection::Inline(steps) = &summary.plan {
                        if let Some(step) = steps.get(*index) {
                            match &step.text {
                                TextContent::Inline(value) => {
                                    let resolved = matches!(
                                        step.status,
                                        StepStatus::Done | StepStatus::Cancelled
                                    );
                                    let font = TextStyle {
                                        size_px: theme.typography.control_text,
                                        strikethrough: resolved,
                                        ..TextStyle::default()
                                    };
                                    content = content.child(
                                        Text::new(value.clone())
                                            .overflow(
                                                aqua_text::overflow::OverflowMode::EndEllipsis,
                                            )
                                            .style(
                                                Style::new()
                                                    .min_width(Length::Px(0.0))
                                                    .flex_shrink(1.0),
                                            )
                                            .text_style(font)
                                            .gpui_source_color(if resolved {
                                                theme.colors.text_muted.to_reference_text_color()
                                            } else {
                                                theme.colors.text.to_reference_text_color()
                                            })
                                            .build(),
                                    );
                                }
                                TextContent::Stored(source) => {
                                    let source = source.clone();
                                    let status = match step.status {
                                        StepStatus::Pending => "○",
                                        StepStatus::InProgress => "●",
                                        StepStatus::Done => "✓",
                                        StepStatus::Cancelled => "−",
                                    };
                                    let callback = Rc::clone(&row_action);
                                    let index = *index;
                                    content = content.child(
                                        Button::text(
                                            format!("inspector-plan-{index}"),
                                            theme,
                                            format!("{status} Read step"),
                                        )
                                        .on_activate(move |_| {
                                            callback(InspectorAction::OpenStep(source.clone()))
                                        })
                                        .build(),
                                    );
                                }
                            }
                        }
                    }
                }
                Entry::Terminal(index) => {
                    let terminal = &summary.terminals[*index];
                    let label = match &terminal.command {
                        TextContent::Inline(value) => value
                            .lines()
                            .find(|line| !line.trim().is_empty())
                            .unwrap_or("Shell")
                            .to_owned(),
                        TextContent::Stored(_) => "Stored shell command".into(),
                    };
                    let callback = Rc::clone(&row_action);
                    let item = terminal.id.clone();
                    let icon = if terminal.watch {
                        AssetId::TABLER_EYE
                    } else {
                        AssetId::TABLER_TERMINAL_2
                    };
                    let font = TextStyle {
                        families: vec![FontFamily::Named(theme.typography.mono.family.to_owned())],
                        size_px: theme.typography.label_text,
                        weight: FontWeight::NORMAL,
                        ..TextStyle::default()
                    };
                    content = content.child(
                        UiButton::new()
                            .width(Length::Pct(100.0))
                            .gap(8.0)
                            .on_activate(move || {
                                callback(InspectorAction::OpenTerminal(item.clone()))
                            })
                            .child(glyph(theme, icon))
                            .child(
                                Text::new(label.clone())
                                    .overflow(aqua_text::overflow::OverflowMode::EndEllipsis)
                                    .style(Style::new().min_width(Length::Px(0.0)).flex_shrink(1.0))
                                    .text_style(font)
                                    .gpui_source_color(theme.colors.text.to_reference_text_color())
                                    .build(),
                            )
                            .build()
                            .role(Role::Button)
                            .cursor(aqua_core::CursorIcon::Pointer)
                            .hover_style(row_hover(theme))
                            .aria_label(label),
                    );
                }
                Entry::Workflow(index) => {
                    let workflow = &summary.workflows[*index];
                    content = content
                        .child(glyph(theme, AssetId::TABLER_SITEMAP))
                        .child(text(
                            theme,
                            workflow.name.clone().unwrap_or_else(|| "Workflow".into()),
                        ));
                }
            }
            let closes = rows
                .get(index + 1)
                .is_none_or(|entry| matches!(entry, Entry::Heading(_)));
            let mut wrapper =
                UiBox::new()
                    .width(Length::Pct(100.0))
                    .style(Style::new().padding_insets(Insets {
                        left: 10.0,
                        right: 10.0,
                        top: 0.0,
                        bottom: if closes { 10.0 } else { 0.0 },
                    }));
            if closes {
                wrapper = wrapper.style(Style::new().border_edges(
                    Insets {
                        bottom: 1.0,
                        ..Insets::default()
                    },
                    theme.colors.line.to_paint(),
                ));
            }
            wrapper.child(content.build()).build()
        })
        .virtualize()
        .style(Style::new().flex_grow(1.0).min_height(Length::Px(0.0)))
        .build();
    root = root.child(list);
    for panel in [source, plan].into_iter().flatten() {
        root = root.child(
            UiBox::new()
                .width(Length::Pct(100.0))
                .style(
                    Style::new()
                        .padding_insets(Insets {
                            left: 10.0,
                            right: 10.0,
                            top: 10.0,
                            bottom: 10.0,
                        })
                        .border_edges(
                            Insets {
                                bottom: 1.0,
                                ..Insets::default()
                            },
                            theme.colors.line.to_paint(),
                        ),
                )
                .child(panel)
                .build(),
        );
    }
    Ok(root.build())
}
