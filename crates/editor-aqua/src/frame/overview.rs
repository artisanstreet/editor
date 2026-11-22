use crate::core::Core;
use crate::inspector::{AgentCard, Inspector, InspectorAction, agent_card, render_agent_card};
use crate::shell::context_meter::ContextReading;
use crate::ui::hint_card::{Phrase, hint_card};
use crate::ui::kit::{Divider, IconSize, Inset, Row, RowItem, Space, Spaced, Surface, TextRole};
use crate::ui::scroll_edge::{Edge, ScrollEdges};
use crate::ui::theme::{ArtisanTheme, Oklch};
use aqua_core::Role;
use aqua_core::scene::GpuiQuadPulse;
use aqua_text::overflow::OverflowMode;
use aqua_ui::element::{Box as UiBox, Element, Scroll};
use aqua_ui::style::{Length, Style};
use aqua_ui::{AlignItems, JustifyContent};
use artisan_api::{EngineId, ItemId, Status, TextContent, ThreadId};
use editor_assets::AssetId;
use std::rc::Rc;

const SURFACE: Surface = Surface::Inset;
const WIDTH: f32 = 320.0;
const DOT: f32 = 6.0;
const PULSE_MS: u32 = 1_600;
pub const AGENTS_KEY: &str = "artisan-overview-agents";
pub const AGENTS_EDGE_TOP: &str = "artisan-overview-agents-edge-top";
pub const AGENTS_EDGE_BOTTOM: &str = "artisan-overview-agents-edge-bottom";
const OUTER_WIDTH: f32 = WIDTH + 2.0 * Inset::Frame.px();

#[must_use]
pub(crate) fn fits(content_width: f32) -> bool {
    content_width - crate::conversation::view::FULL_CHAT_WIDTH >= OUTER_WIDTH
}

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

impl std::fmt::Debug for Subagent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subagent").finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub struct Overview {
    pub project: Option<String>,
    pub machine: Option<String>,
    pub branch: Option<String>,
    pub context: Option<ContextReading>,
    pub agents: Vec<Subagent>,
}

impl Overview {
    #[must_use]
    pub(crate) fn of(core: &Core, inspector: &Inspector, context: Option<ContextReading>) -> Self {
        let host = inspector.open().and_then(|(host, _)| core.host(host));
        let project = host.and_then(|host| {
            inspector
                .open()
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
                .summary()
                .agents
                .iter()
                .map(|agent| Subagent {
                    id: agent.id.clone(),
                    thread: agent.thread.clone(),
                    label: agent.label.clone(),
                    task: match &agent.description {
                        TextContent::Inline(text) => {
                            text.lines().next().unwrap_or_default().to_owned()
                        }
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
        self.project.is_some()
            || self.machine.is_some()
            || self.branch.is_some()
            || self.context.is_some()
    }
}

#[must_use]
pub fn agents_edge_color(theme: &ArtisanTheme) -> Oklch {
    crate::ui::chrome::card::Surfaces::of(theme).inner
}

#[must_use]
pub fn card(
    theme: &ArtisanTheme,
    overview: &Overview,
    reduced: bool,
    emit: &Rc<dyn Fn(InspectorAction)>,
    panel: &OverviewPanel<'_>,
) -> Option<Element> {
    if !overview.has_facts() && overview.agents.is_empty() {
        return None;
    }
    let facts = [
        overview
            .project
            .as_deref()
            .map(|project| fact(theme, AssetId::TABLER_FOLDER, "Project", project)),
        overview
            .machine
            .as_deref()
            .map(|machine| fact(theme, AssetId::TABLER_DEVICE_LAPTOP, "Machine", machine)),
        overview
            .branch
            .as_deref()
            .map(|branch| fact(theme, AssetId::TABLER_GIT_BRANCH, "Branch", branch)),
        overview
            .context
            .as_ref()
            .map(|reading| context(theme, reading)),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    let rule = (!facts.is_empty() && !overview.agents.is_empty())
        .then(|| Divider::Rule.build(theme, SURFACE.padding_px()));
    let agents = (!overview.agents.is_empty()).then(|| {
        let rows = UiBox::new().layout("column").children(
            overview
                .agents
                .iter()
                .map(|agent| subagent(theme, agent, reduced, Rc::clone(emit))),
        );
        let list = Scroll::new()
            .layout("column")
            .preserve_offset(true)
            .style(shrinking())
            .child(rows.build())
            .build()
            .with_key(AGENTS_KEY)
            .role(Role::List)
            .aria_label(editor_ui::locale::count(
                overview.agents.len(),
                "agent",
                "agents",
            ));
        let edge = agents_edge_color(theme);
        UiBox::new()
            .layout("column")
            .style(shrinking())
            .child(list)
            .child(panel.edges.band(AGENTS_EDGE_TOP, Edge::Top, edge))
            .child(panel.edges.band(AGENTS_EDGE_BOTTOM, Edge::Bottom, edge))
            .build()
    });
    let fixed = UiBox::new()
        .layout("column")
        .style(Style::new().flex_shrink(0.0))
        .children(facts)
        .children(rule)
        .build();
    let content = UiBox::new()
        .layout("column")
        .style(shrinking())
        .child(fixed)
        .children(agents)
        .build();
    Some(frame(
        SURFACE
            .build(theme, vec![content])
            .style(Style::new().max_height(Length::Px(panel.height))),
        AlignItems::START,
        "artisan-thread-overview",
    ))
}

#[derive(Debug)]
pub struct OverviewPanel<'a> {
    height: f32,
    edges: &'a ScrollEdges,
}

impl<'a> OverviewPanel<'a> {
    #[must_use]
    pub fn in_window(window_height: f32, edges: &'a ScrollEdges) -> Self {
        Self {
            height: panel_height(window_height),
            edges,
        }
    }
}

fn shrinking() -> Style {
    Style::new().flex_shrink(1.0).min_height(Length::Px(0.0))
}

#[must_use]
pub(crate) fn panel_height(viewport_height: f32) -> f32 {
    2.0f32
        .mul_add(
            -Inset::Frame.px(),
            viewport_height - crate::shell::layout::TITLEBAR_HEIGHT,
        )
        .max(0.0)
}

#[must_use]
pub(crate) fn panel(theme: &ArtisanTheme, inspector: Element) -> Element {
    frame(
        SURFACE
            .build(theme, vec![inspector])
            .style(Style::new().min_height(Length::Pct(0.0))),
        AlignItems::STRETCH,
        "artisan-thread-inspector",
    )
}

fn frame(surface: Element, align: AlignItems, key: &'static str) -> Element {
    UiBox::new()
        .layout("row")
        .padded(Space::from(Inset::Frame))
        .style(
            Style::new()
                .width(Length::Px(OUTER_WIDTH))
                .flex_shrink(0.0)
                .align(align),
        )
        .child(surface.style(Style::new().flex_grow(1.0)))
        .build()
        .with_key(key)
        .debug_selector(key)
}

fn fact(theme: &ArtisanTheme, icon: AssetId, title: &str, value: &str) -> Element {
    RowItem::new(
        format!("artisan-overview-fact-{title}"),
        *theme,
        Row::Key,
        SURFACE,
        format!("{title}: {value}"),
    )
    .leading(IconSize::Md.glyph(icon, theme.colors.text_muted.to_reference_paint()))
    .body(
        UiBox::new()
            .layout("row")
            .spaced(Row::Heading.gap())
            .style(body())
            .child(label(theme, title, theme.colors.text_muted, Span::Fit))
            .child(label(theme, value, theme.colors.text, Span::Trail))
            .build(),
    )
    .build()
}

fn context(theme: &ArtisanTheme, reading: &ContextReading) -> Element {
    let percent = reading.percent().round();
    let theme = *theme;
    RowItem::new(
        "artisan-overview-context",
        theme,
        Row::Key,
        SURFACE,
        reading.percent_sentence(),
    )
    .leading(IconSize::Md.glyph(
        AssetId::TABLER_ARROWS_HORIZONTAL,
        theme.colors.text_muted.to_reference_paint(),
    ))
    .body(crate::ui::progress::bar(
        &theme,
        reading.percent(),
        crate::ui::progress::HEIGHT,
    ))
    .hint(move || {
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
    .build()
}

fn subagent(
    theme: &ArtisanTheme,
    agent: &Subagent,
    reduced: bool,
    emit: Rc<dyn Fn(InspectorAction)>,
) -> Element {
    let icon = agent.engine.map_or(AssetId::TABLER_BOT_ID, |engine| {
        crate::ui::engine::engine_icon(theme, engine)
    });
    let color = match agent.status {
        Status::Running | Status::AwaitingUser => theme.colors.text,
        Status::Done => theme.colors.unread,
        Status::Failed => theme.colors.red.solid,
        Status::Declined | Status::Cancelled => theme.colors.text_muted,
    };
    let open = InspectorAction::OpenAgent {
        id: agent.id.clone(),
        thread: agent.thread.clone(),
    };
    let tooltip = agent.card.clone();
    let theme = *theme;
    RowItem::new(
        format!("artisan-overview-agent-{}", agent.id),
        theme,
        Row::MenuItem,
        SURFACE,
        agent.label.clone(),
    )
    .role(Role::Button)
    .leading(IconSize::Md.glyph(icon, theme.colors.text_muted.to_reference_paint()))
    .body(
        UiBox::new()
            .layout("row")
            .spaced(Row::MenuItem.gap())
            .style(body())
            .child(label(&theme, &agent.label, theme.colors.text, Span::Fit))
            .child(label(
                &theme,
                &agent.task,
                theme.colors.text_muted,
                Span::Fill,
            ))
            .build(),
    )
    .trailing(
        UiBox::new()
            .width(IconSize::Md.px())
            .height(IconSize::Md.px())
            .style(
                Style::new()
                    .flex_shrink(0.0)
                    .align(AlignItems::CENTER)
                    .justify(JustifyContent::CENTER),
            )
            .child(dot(color, agent.status == Status::Running && !reduced))
            .build(),
    )
    .on_activate(move || emit(open.clone()))
    .hint(move || render_agent_card(&tooltip, &theme))
    .build()
}

fn body() -> Style {
    Style::new()
        .align(AlignItems::CENTER)
        .flex_grow(1.0)
        .min_width(Length::Pct(0.0))
}

#[expect(
    clippy::disallowed_methods,
    reason = "the agent status dot is a painted status mark, not chrome"
)]
fn dot(color: Oklch, pulsing: bool) -> Element {
    let mut style = Style::new()
        .flex_shrink(0.0)
        .gpui_background(aqua_core::GpuiBackground::Solid(color.to_reference_paint()));
    if pulsing {
        style = style.gpui_quad_pulse(GpuiQuadPulse {
            start_ms: 0,
            period_ms: PULSE_MS,
            min_opacity: 0.4,
        });
    }
    UiBox::new()
        .width(DOT)
        .height(DOT)
        .radius(DOT / 2.0)
        .style(style)
        .build()
}

const FIT_WIDEST_PCT: f32 = 70.0;

#[derive(Clone, Copy)]
enum Span {
    Fit,
    Fill,
    Trail,
}

fn label(theme: &ArtisanTheme, words: &str, color: Oklch, span: Span) -> Element {
    let text = TextRole::Label
        .text(theme, words.to_owned())
        .gpui_source_color(color.to_reference_text_color())
        .wrap(false)
        .overflow(OverflowMode::EndEllipsis)
        .build();
    let base = Style::new()
        .justify(JustifyContent::START)
        .min_width(Length::Pct(0.0))
        .clip(true);
    let style = match span {
        Span::Fit => base.flex_shrink(0.0).max_width(Length::Pct(FIT_WIDEST_PCT)),
        Span::Fill => base.flex_grow(1.0),
        Span::Trail => base.justify(JustifyContent::END).flex_grow(1.0),
    };
    UiBox::new().layout("row").style(style).child(text).build()
}
