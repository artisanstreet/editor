use crate::shell::application::BuildError;
use crate::ui::kit::surface::frame;
use crate::ui::kit::{Corners, Radius, Space, Spaced, TextRole, concentric};
use crate::ui::theme::ArtisanTheme;
use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Element},
    style::{Length, Style},
};

const EDGE_INSET: f32 = 2.0;
const CONVERSATION_WIDTH: f32 = 768.0;
const FORGES_WIDTH: f32 = 560.0;

pub(crate) struct WorkspaceElements {
    pub body: Element,
    pub settings_dialog: Option<Element>,
    pub palette: Option<Element>,
    pub resource_preview: Option<Element>,
    pub copy_menu: Option<Element>,
    pub image_preview: Option<Element>,
}

pub(crate) fn workspace(
    workspace_key: u64,
    theme: &ArtisanTheme,
    elements: WorkspaceElements,
) -> Element {
    let mut root = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .style(frame(theme))
        .style(TextRole::Label.inherit(theme))
        .child(elements.body);
    for overlay in [
        elements.settings_dialog,
        elements.palette,
        elements.resource_preview,
        elements.copy_menu,
        elements.image_preview,
    ]
    .into_iter()
    .flatten()
    {
        root = root.child(overlay);
    }
    root.build()
        .with_key(workspace_key)
        .debug_selector("workspace-root")
}

pub(crate) struct ComposerParts {
    pub questionnaires: Option<Element>,
    pub attachments: Option<Element>,
    pub mention_overlay: Option<Element>,
    pub thread_error: Option<Element>,
    pub shift: f32,
    pub measurement: std::rc::Rc<std::cell::Cell<Option<f32>>>,
    pub above: Option<Element>,
}

pub(crate) type InitialHeight =
    std::rc::Rc<dyn Fn(usize, &crate::conversation::fold::RowKey) -> f32>;

pub(crate) struct Transcript {
    pub estimated_height: f32,
    pub row: crate::conversation::view::transcript::LayoutBatchedRowFactory,
    pub revision: crate::conversation::view::transcript::RowRevision,
    pub viewport: std::rc::Rc<dyn Fn(aqua_ui::collection_runtime::CollectionViewport)>,
    pub initial_height: InitialHeight,
    pub overlays: Vec<Element>,
    pub drawer: Option<Element>,
}

pub(crate) fn conversation_virtual(
    workspace: &crate::shell::workspace::Workspace,
    core: &crate::core::Core,
    theme: &ArtisanTheme,
    viewport_width: f32,
    transcript: Transcript,
    parts: ComposerParts,
) -> Result<Element, BuildError> {
    let Transcript {
        estimated_height,
        row,
        revision,
        viewport,
        initial_height,
        overlays,
        drawer,
    } = transcript;
    let card = if workspace.conversation.agent.is_some() {
        None
    } else {
        Some(measured_composer(workspace, core, theme, parts)?)
    };
    Ok(crate::conversation::view::layout_batched_thread_element(
        &workspace.conversation,
        estimated_height,
        row,
        revision,
        viewport,
        initial_height,
        overlays,
        card,
        drawer,
        viewport_width,
    ))
}

fn measured_composer(
    workspace: &crate::shell::workspace::Workspace,
    core: &crate::core::Core,
    theme: &ArtisanTheme,
    mut parts: ComposerParts,
) -> Result<Element, BuildError> {
    let composer_measurement = std::rc::Rc::clone(&parts.measurement);
    let above_composer = parts.above.take();
    let shift = parts.shift;
    let owner = workspace.composer.as_ref().ok_or(BuildError::NoComposer)?;
    let (host_id, instance, placeholder) = {
        let mut composer = owner.controller.borrow_mut();
        let question = workspace
            .conversation
            .question_pages
            .current_question(&composer.questionnaire)
            .cloned();
        (
            composer.target.host().clone(),
            composer.host_instance(),
            composer.placeholder(question.as_ref()),
        )
    };
    let host = instance
        .and_then(|instance| core.host_if_instance(&host_id, instance))
        .ok_or(BuildError::ComposerHostGone { host: host_id })?;
    let card = owner.surface(
        theme,
        host,
        placeholder,
        parts.questionnaires,
        parts.attachments,
        parts.mention_overlay,
        parts.thread_error,
        core.clock().now(),
    );
    let measure = aqua_ui::element::Canvas::new()
        .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
        .on_prepaint(move |tree, node| {
            if let Some(rect) = tree
                .get(node)
                .and_then(|node| node.parent)
                .and_then(|parent| tree.rect(parent))
                && rect.h.is_finite()
                && rect.h >= 0.0
            {
                composer_measurement.set(Some(rect.h));
            }
        })
        .build()
        .exclude_from_hit_testing();
    let card = UiBox::new()
        .width(Length::Pct(100.0))
        .child(card)
        .child(measure)
        .build();
    let mut column = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .spaced(Space::S8)
        .style(Style::new().transform(aqua_core::Transform2D::translate(0.0, shift)));
    if let Some(above) = above_composer {
        column = column.child(above);
    }
    Ok(column
        .child(card)
        .build()
        .with_key(crate::shell::composer_glide::DOCK_KEY))
}

pub(crate) fn conversation_body(
    workspace: &crate::shell::workspace::Workspace,
    core: &crate::core::Core,
    theme: &ArtisanTheme,
    viewport_width: f32,
    body: Element,
    overlays: Vec<Element>,
    parts: ComposerParts,
) -> Result<Element, BuildError> {
    let card = if workspace.conversation.agent.is_some() {
        None
    } else {
        Some(measured_composer(workspace, core, theme, parts)?)
    };
    Ok(crate::conversation::view::thread_frame(
        body,
        overlays,
        card,
        None,
        viewport_width,
        1.0,
    ))
}

pub(crate) fn centered_body(content: Element) -> Element {
    UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .justify(JustifyContent::CENTER),
        )
        .child(content)
        .build()
}

pub(crate) fn conversation_edge_style(edge: crate::ui::scroll_edge::Edge) -> Style {
    let radius = concentric(Radius::Shell.px(), EDGE_INSET);
    let (top, bottom, corners) = match edge {
        crate::ui::scroll_edge::Edge::Top => (Some(EDGE_INSET), None, Corners::Top),
        crate::ui::scroll_edge::Edge::Bottom => (None, Some(EDGE_INSET), Corners::Bottom),
    };
    Style::new()
        .width(Length::Auto)
        .absolute_at(Some(EDGE_INSET), top, Some(EDGE_INSET), bottom)
        .with(&corners.style(radius))
}

pub(crate) fn conversation_notice(theme: &ArtisanTheme, text: String) -> Element {
    centered_body(
        TextRole::Label
            .text(theme, text)
            .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
            .build(),
    )
}

pub(crate) struct Sky {
    pub opacity: f32,
    pub transform: aqua_core::Transform2D,
}

fn sky(
    theme: &ArtisanTheme,
    composer_measurement: std::rc::Rc<std::cell::Cell<Option<f32>>>,
    reduced_motion: bool,
    look: &Sky,
) -> Element {
    crate::ui::sky::artwork(theme, composer_measurement, reduced_motion)
        .style(Style::new().opacity(look.opacity).transform(look.transform))
}

pub(crate) fn new_conversation(
    workspace: &crate::shell::workspace::Workspace,
    core: &crate::core::Core,
    theme: &ArtisanTheme,
    parts: ComposerParts,
    reduced_motion: bool,
    docked: Option<f32>,
    look: Option<&Sky>,
) -> Result<Element, BuildError> {
    let composer_measurement = std::rc::Rc::clone(&parts.measurement);
    let artwork = look.map(|look| {
        sky(
            theme,
            std::rc::Rc::clone(&composer_measurement),
            reduced_motion,
            look,
        )
    });
    let card = measured_composer(workspace, core, theme, parts)?;
    let body = match docked {
        Some(width) => crate::conversation::view::thread_frame(
            UiBox::new().build(),
            Vec::new(),
            Some(card),
            None,
            width,
            1.0,
        ),
        None => centered_conversation(card),
    };
    Ok(UiBox::new()
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .clip(true)
        .children(artwork)
        .child(body.with_key("new-thread-body"))
        .build()
        .with_key(crate::shell::composer_glide::STAGE_KEY))
}

pub(crate) fn under_setting_sky(
    theme: &ArtisanTheme,
    composer_measurement: std::rc::Rc<std::cell::Cell<Option<f32>>>,
    reduced_motion: bool,
    look: Option<&Sky>,
    content: Element,
) -> Element {
    UiBox::new()
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .clip(true)
        .children(look.map(|look| sky(theme, composer_measurement, reduced_motion, look)))
        .child(
            UiBox::new()
                .width(Length::Pct(100.0))
                .height(Length::Pct(100.0))
                .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
                .child(content)
                .build()
                .with_key("thread-under-sky"),
        )
        .build()
}

pub(crate) fn centered_conversation(content: Element) -> Element {
    UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .padded(Space::S24)
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .justify(JustifyContent::CENTER),
        )
        .child(
            UiBox::new()
                .width(Length::Pct(100.0))
                .style(Style::new().max_width(Length::Px(CONVERSATION_WIDTH)))
                .child(content)
                .build(),
        )
        .build()
}

pub(crate) fn home(theme: &ArtisanTheme, forges: Element) -> Element {
    let content = UiBox::new()
        .layout("column")
        .spaced(Space::S8)
        .style(Style::new().align(AlignItems::CENTER))
        .child(
            TextRole::Heading
                .text(theme, "Choose a Forge")
                .build()
                .role(aqua_core::Role::Heading(1)),
        )
        .child(
            UiBox::new()
                .width(Length::Pct(100.0))
                .style(Style::new().max_width(Length::Px(FORGES_WIDTH)))
                .child(forges)
                .build(),
        )
        .build();
    UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .justify(JustifyContent::CENTER),
        )
        .child(content)
        .build()
}
