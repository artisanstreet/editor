use crate::core::Core;
use crate::ui::chrome::card::{self, INSET_RADIUS, Surfaces};
use crate::inspector::{AgentCard, Inspector, InspectorAction, agent_card, render_agent_card};
use crate::shell::context_meter::ContextReading;
use crate::ui::hint_card::{Phrase, hint_card};
use crate::ui::asset::source_asset_glyph;
use crate::ui::theme::{ArtisanTheme, Oklch};
use aqua::{Aqua, Style, pct, px};
use aqua_core::scene::GpuiQuadPulse;
use aqua_core::{CursorIcon, GpuiBackground, GpuiGradientColorSpace, GpuiGradientStop, Insets, Role};
use aqua_text::overflow::OverflowMode;
use aqua_text::{FontFamily, FontWeight, TextStyle};
use aqua_ui::{AlignItems, JustifyContent};
use aqua_ui::element::Element;
use artisan_api::{EngineId, ItemId, Status, TextContent, ThreadId};
use editor_assets::AssetId;
use std::rc::Rc;

pub const MARGIN: f32 = card::RADIUS - INSET_RADIUS;
const PADDING: f32 = 16.0;
const WIDTH: f32 = 320.0;
const FACT_GAP: f32 = 12.0;
const TEXT: f32 = 14.0;
const LINE: f32 = 20.0;
const GLYPH: f32 = 16.0;
const ROW_PAD: f32 = 8.0;
const ROW_RADIUS: f32 = INSET_RADIUS - (PADDING - ROW_PAD);
const DOT: f32 = 6.0;
const PULSE_MS: u32 = 1_600;

#[derive(Clone)]
pub struct Subagent {
    pub id: ItemId,
    pub thread: Option<ThreadId>,
    pub label: String,
    pub task: String,
    pub status: Status,
    pub engine: Option<EngineId>,
    pub card: AgentCard,
}

#[derive(Clone)]
pub struct Overview {
    pub project: Option<String>,
    pub machine: Option<String>,
    pub branch: Option<String>,
    pub context: Option<ContextReading>,
    pub agents: Vec<Subagent>,
}

impl Overview {
    #[must_use]
    pub fn of(core: &Core, inspector: &Inspector, context: Option<ContextReading>) -> Self {
        let host = inspector.open.as_ref().and_then(|(host, _)| core.host(host));
        let project = host.and_then(|host| {
            inspector
                .open
                .as_ref()
                .and_then(|(_, thread)| host.threads.get(thread))
                .map(|thread| &thread.project_id)
        });
        Self {
            project: project
                .and_then(|project| host?.projects.get(project))
                .map(|project| project.name.clone()),
            machine: host.map(|host| host.name.clone()),
            branch: project.and_then(|project| host?.repository(project)?.branch.clone()),
            context,
            agents: inspector
                .summary
                .agents
                .iter()
                .map(|agent| Subagent {
                    id: agent.id.clone(),
                    thread: agent.thread.clone(),
                    label: agent.label.clone(),
                    task: match &agent.description {
                        TextContent::Inline(text) => text.lines().next().unwrap_or_default().to_owned(),
                        TextContent::Stored(_) => String::new(),
                    },
                    status: agent.status,
                    engine: agent.engine,
                    card: agent_card(agent, host),
                })
                .collect(),
        }
    }

    fn has_facts(&self) -> bool {
        self.project.is_some() || self.machine.is_some() || self.branch.is_some() || self.context.is_some()
    }
}

#[must_use]
pub fn card(
    theme: &ArtisanTheme,
    overview: &Overview,
    reduced: bool,
    emit: Rc<dyn Fn(InspectorAction)>,
) -> Option<Element> {
    if !overview.has_facts() && overview.agents.is_empty() {
        return None;
    }
    let surfaces = Surfaces::of(theme);
    let facts = [
        overview
            .project
            .as_deref()
            .map(|project| fact(theme, &surfaces, AssetId::TABLER_FOLDER, "Project", project)),
        overview
            .machine
            .as_deref()
            .map(|machine| fact(theme, &surfaces, AssetId::TABLER_DEVICE_LAPTOP, "Machine", machine)),
        overview
            .branch
            .as_deref()
            .map(|branch| fact(theme, &surfaces, AssetId::TABLER_GIT_BRANCH, "Branch", branch)),
        overview
            .context
            .as_ref()
            .map(|reading| context(theme, &surfaces, reading)),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    let rule = (!facts.is_empty() && !overview.agents.is_empty())
        .then(|| card::double_rule(theme, PADDING));
    let agents = overview
        .agents
        .iter()
        .map(|agent| subagent(theme, &surfaces, agent, reduced, Rc::clone(&emit)));
    let content = Aqua::Box()
        .layout("column")
        .styles([Aqua::Style().padding(px(PADDING)).gap(8.0)])
        .children((!facts.is_empty()).then(|| {
            Aqua::Box()
                .layout("column")
                .styles([Aqua::Style().gap(FACT_GAP)])
                .children(facts)
                .build()
        }))
        .children(rule)
        .child(
            Aqua::Box()
                .layout("column")
                .styles([Aqua::Style().margin_insets(Insets {
                    left: -ROW_PAD,
                    right: -ROW_PAD,
                    ..Insets::default()
                })])
                .children(agents)
                .build(),
        )
        .build();
    Some(
        Aqua::Box()
            .layout("row")
            .styles([Aqua::Style()
                .width(px(WIDTH))
                .flex_shrink(0.0)
                .align(AlignItems::START)
                .margin_insets(Insets {
                    top: MARGIN,
                    right: MARGIN,
                    bottom: MARGIN,
                    ..Insets::default()
                })])
            .child(card::inset(theme, vec![content]))
            .build()
            .with_key("artisan-thread-overview")
            .debug_selector("artisan-thread-overview"),
    )
}

fn fact(theme: &ArtisanTheme, surfaces: &Surfaces, icon: AssetId, title: &str, value: &str) -> Element {
    Aqua::Box()
        .layout("row")
        .styles([Aqua::Style().align(AlignItems::CENTER).gap(8.0)])
        .child(source_asset_glyph(icon, GLYPH, surfaces.ink_dim.to_reference_paint()))
        .child(label(theme, title, surfaces.ink_dim, Span::Fit))
        .child(label(theme, value, surfaces.ink, Span::Trail))
        .build()
        .aria_label(format!("{title}: {value}"))
}

fn context(theme: &ArtisanTheme, surfaces: &Surfaces, reading: &ContextReading) -> Element {
    let percent = reading.percent().round();
    let theme = *theme;
    Aqua::Box()
        .layout("row")
        .styles([Aqua::Style().align(AlignItems::CENTER).gap(8.0)])
        .child(source_asset_glyph(
            AssetId::TABLER_ARROWS_HORIZONTAL,
            GLYPH,
            surfaces.ink_dim.to_reference_paint(),
        ))
        .child(
            Aqua::Box()
                .styles([Aqua::Style().flex_grow(1.0).min_width(px(0.0))])
                .child(crate::ui::progress::bar(&theme, reading.percent(), crate::ui::progress::HEIGHT))
                .build(),
        )
        .build()
        .aria_label(reading.percent_sentence())
        .tooltip_with(0.5, crate::ui::hint_card::presence(), move || {
            hint_card(
                &theme,
                "Context Window",
                &[
                    Phrase::Muted("The context window is ".to_owned()),
                    Phrase::Strong(format!("{percent:.0}%")),
                    Phrase::Muted(" full.".to_owned()),
                ],
            )
        })
}

fn subagent(
    theme: &ArtisanTheme,
    surfaces: &Surfaces,
    agent: &Subagent,
    reduced: bool,
    emit: Rc<dyn Fn(InspectorAction)>,
) -> Element {
    let icon = agent.engine.map_or(AssetId::TABLER_BOT_ID, |engine| {
        crate::ui::engine::engine_icon(theme, engine)
    });
    let color = match agent.status {
        Status::Running | Status::AwaitingUser => surfaces.ink,
        Status::Done => theme.colors.unread,
        Status::Failed => theme.colors.red.solid,
        Status::Declined | Status::Cancelled => surfaces.ink_dim,
    };
    let mut dot = Style::new()
        .width(px(DOT))
        .height(px(DOT))
        .radius(px(DOT / 2.0))
        .gpui_background(GpuiBackground::Solid(color.to_reference_paint()));
    if agent.status == Status::Running && !reduced {
        dot = dot.gpui_quad_pulse(GpuiQuadPulse {
            start_ms: 0,
            period_ms: PULSE_MS,
            min_opacity: 0.4,
        });
    }
    let open = InspectorAction::OpenAgent {
        id: agent.id.clone(),
        thread: agent.thread.clone(),
    };
    let tooltip = agent.card.clone();
    let theme = *theme;
    Aqua::Button()
        .styles([Aqua::Style()
            .width(pct(100.0))
            .align(AlignItems::CENTER)
            .gap(8.0)
            .padding(px(ROW_PAD))
            .radius(px(ROW_RADIUS))])
        .child(source_asset_glyph(icon, GLYPH, surfaces.ink_dim.to_reference_paint()))
        .child(label(&theme, &agent.label, surfaces.ink, Span::Fit))
        .child(label(&theme, &agent.task, surfaces.ink_dim, Span::Fill))
        .child(
            Aqua::Box()
                .styles([Aqua::Style()
                    .width(px(GLYPH))
                    .height(px(GLYPH))
                    .flex_shrink(0.0)
                    .align(AlignItems::CENTER)
                    .justify(JustifyContent::CENTER)])
                .child(Aqua::Box().styles([dot]).build())
                .build(),
        )
        .on_activate(move || emit(open.clone()))
        .build()
        .with_key(format!("artisan-overview-agent-{}", agent.id))
        .role(Role::Button)
        .aria_label(agent.label.clone())
        .cursor(CursorIcon::Pointer)
        .hover_style(hover(&theme, surfaces.ink))
        .tooltip_with(0.5, crate::ui::hint_card::presence(), move || {
            render_agent_card(&tooltip, theme)
        })
}

fn hover(theme: &ArtisanTheme, ink: Oklch) -> Style {
    let (top, bottom) = crate::ui::hover::hover_fill_stops(theme, ink);
    Style::new().gpui_background(GpuiBackground::LinearGradient {
        angle: 180.0,
        color_space: GpuiGradientColorSpace::Oklab,
        stops: [
            GpuiGradientStop {
                percentage: 0.0,
                color: top.to_reference_paint(),
            },
            GpuiGradientStop {
                percentage: 1.0,
                color: bottom.to_reference_paint(),
            },
        ],
    })
}

#[derive(Clone, Copy)]
enum Span {
    Fit,
    Fill,
    Trail,
}

fn label(theme: &ArtisanTheme, words: &str, color: Oklch, span: Span) -> Element {
    let text = Aqua::Text(words.to_owned())
        .text_style(TextStyle {
            families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
            size_px: TEXT,
            line_height_px: LINE,
            weight: FontWeight::NORMAL,
            ..TextStyle::default()
        })
        .wrap(false)
        .overflow(OverflowMode::EndEllipsis)
        .gpui_color(color.to_reference_paint())
        .build();
    let (grow, justify) = match span {
        Span::Fit => (0.0, JustifyContent::START),
        Span::Fill => (1.0, JustifyContent::START),
        Span::Trail => (1.0, JustifyContent::END),
    };
    Aqua::Box()
        .layout("row")
        .styles([Aqua::Style()
            .justify(justify)
            .flex_grow(grow)
            .flex_shrink(1.0)
            .min_width(px(0.0))
            .clip(true)])
        .child(text)
        .build()
}
