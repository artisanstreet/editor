use crate::shell::layout::{self, Size};
use crate::ui::kit::pane::sheet;
use crate::ui::kit::surface::frame;
use crate::ui::kit::{
    Corners, Finish, IconButton, IconButtonSize, Radius, Seam, Space, Spaced, TextRole, concentric,
};
use crate::ui::theme::ArtisanTheme;
use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Element},
    style::{Length, Style},
};

const EDGE_INSET: f32 = 2.0;
const STRIP_PAD: Space = Space::S8;
const CONVERSATION_WIDTH: f32 = 768.0;
const FORGES_WIDTH: f32 = 560.0;

pub struct WorkspaceElements {
    pub body: Element,
    pub settings_dialog: Option<Element>,
    pub palette: Option<Element>,
    pub resource_preview: Option<Element>,
    pub copy_menu: Option<Element>,
    pub image_preview: Option<Element>,
}

pub fn settings_dialog(
    theme: ArtisanTheme,
    viewport: Size,
    sidebar: Element,
    page: Element,
    dismiss: std::rc::Rc<dyn Fn()>,
) -> crate::ui::dialog::Dialog {
    let dimensions = layout::settings_layout(viewport);
    let panel = settings_panel(theme, viewport, sidebar, page, std::rc::Rc::clone(&dismiss));
    crate::ui::dialog::Dialog::new("settings-dialog", theme, "Settings", panel)
        .bare()
        .soft_backdrop()
        .max_width(dimensions.size.width)
        .on_dismiss(move |_| dismiss())
}

pub fn settings_panel(
    theme: ArtisanTheme,
    viewport: Size,
    sidebar: Element,
    page: Element,
    dismiss: std::rc::Rc<dyn Fn()>,
) -> Element {
    let dimensions = layout::settings_layout(viewport);
    let close = std::rc::Rc::clone(&dismiss);
    let close = IconButton::new(
        "settings-close",
        theme,
        editor_assets::AssetId::TABLER_X,
        "Close settings",
        IconButtonSize::Md,
        Finish::Plain,
    )
    .muted()
    .on_activate(move |_| close())
    .build()
    .debug_selector("settings-close");
    let page = UiBox::new()
        .style(
            Style::new()
                .flex_grow(1.0)
                .min_width(Length::Pct(0.0))
                .min_height(Length::Pct(0.0)),
        )
        .style(Corners::TopLeft.style(Radius::Panel.px()))
        .style(Seam::TopLeft.style(&theme))
        .style(sheet(&theme))
        .clip(true)
        .child(page)
        .build();
    let strip = UiBox::new()
        .height(layout::TITLEBAR_HEIGHT)
        .padded(STRIP_PAD)
        .style(
            Style::new()
                .flex_shrink(0.0)
                .align(AlignItems::CENTER)
                .justify(JustifyContent::END),
        )
        .child(close)
        .build();
    UiBox::new()
        .layout("row")
        .width(Length::Pct(100.0))
        .height(dimensions.size.height)
        .clip(true)
        .style(Radius::Shell.style())
        .style(frame(&theme))
        .style(Seam::Around.style(&theme))
        .child(
            UiBox::new()
                .width(dimensions.navigation_width)
                .height(Length::Pct(100.0))
                .style(Style::new().flex_shrink(0.0))
                .child(sidebar)
                .build(),
        )
        .child(
            UiBox::new()
                .layout("column")
                .height(Length::Pct(100.0))
                .style(
                    Style::new()
                        .flex_grow(1.0)
                        .min_width(Length::Pct(0.0))
                        .min_height(Length::Pct(0.0)),
                )
                .child(strip)
                .child(page)
                .build(),
        )
        .build()
}

pub fn workspace(workspace_key: u64, theme: &ArtisanTheme, elements: WorkspaceElements) -> Element {
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

pub struct ComposerParts {
    pub nested: bool,
    pub placeholder: String,
    pub questionnaires: Option<Element>,
    pub attachments: Option<Element>,
    pub mention_overlay: Option<Element>,
    pub thread_error: Option<Element>,
}

pub fn conversation(
    workspace: &crate::shell::workspace::Workspace,
    core: &crate::core::Core,
    theme: &ArtisanTheme,
    viewport_width: f32,
    transcript: Element,
    overlays: Vec<Element>,
    parts: ComposerParts,
) -> anyhow::Result<Element> {
    let owner = workspace
        .composer
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("conversation composer is not mounted"))?;
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
    let host = core
        .host_if_instance(
            &host_id,
            instance.ok_or_else(|| anyhow::anyhow!("conversation Forge is not available"))?,
        )
        .ok_or_else(|| anyhow::anyhow!("conversation Forge is not available"))?;
    let card = owner.surface(
        *theme,
        host,
        placeholder,
        parts.questionnaires,
        parts.attachments,
        parts.mention_overlay,
        parts.thread_error,
        std::time::Instant::now(),
    );
    Ok(crate::conversation::view::thread_frame(
        transcript,
        overlays,
        Some(card),
        viewport_width,
        workspace.conversation.is_landing(),
    ))
}

pub fn conversation_virtual(
    workspace: &crate::shell::workspace::Workspace,
    core: &crate::core::Core,
    theme: &ArtisanTheme,
    viewport_width: f32,
    estimated_height: f32,
    row: crate::conversation::view::transcript::LayoutBatchedRowFactory,
    revision: crate::conversation::view::transcript::RowRevision,
    viewport: std::rc::Rc<dyn Fn(aqua_ui::collection_runtime::CollectionViewport)>,
    initial_height: std::rc::Rc<dyn Fn(usize, &crate::conversation::fold::RowKey) -> f32>,
    overlays: Vec<Element>,
    parts: ComposerParts,
    composer_measurement: std::rc::Rc<std::cell::Cell<Option<f32>>>,
    above_composer: Option<Element>,
) -> anyhow::Result<Element> {
    if workspace.conversation.agent.is_some() {
        return Ok(crate::conversation::view::layout_batched_thread_element(
            &workspace.conversation,
            estimated_height,
            row,
            revision,
            viewport,
            initial_height,
            overlays,
            None,
            viewport_width,
        ));
    }
    let card = measured_composer(
        workspace,
        core,
        theme,
        parts,
        composer_measurement,
        above_composer,
    )?;
    Ok(crate::conversation::view::layout_batched_thread_element(
        &workspace.conversation,
        estimated_height,
        row,
        revision,
        viewport,
        initial_height,
        overlays,
        Some(card),
        viewport_width,
    ))
}

fn measured_composer(
    workspace: &crate::shell::workspace::Workspace,
    core: &crate::core::Core,
    theme: &ArtisanTheme,
    parts: ComposerParts,
    composer_measurement: std::rc::Rc<std::cell::Cell<Option<f32>>>,
    above_composer: Option<Element>,
) -> anyhow::Result<Element> {
    let owner = workspace
        .composer
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("conversation composer is not mounted"))?;
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
    let host = core
        .host_if_instance(
            &host_id,
            instance.ok_or_else(|| anyhow::anyhow!("conversation Forge is not available"))?,
        )
        .ok_or_else(|| anyhow::anyhow!("conversation Forge is not available"))?;
    let card = owner.surface(
        *theme,
        host,
        placeholder,
        parts.questionnaires,
        parts.attachments,
        parts.mention_overlay,
        parts.thread_error,
        std::time::Instant::now(),
    );
    let measure = aqua_ui::element::Canvas::new()
        .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
        .on_prepaint(move |tree, node| {
            if let Some(rect) = tree
                .get(node)
                .and_then(|node| node.parent)
                .and_then(|parent| tree.rect(parent))
            {
                if rect.h.is_finite() && rect.h >= 0.0 {
                    composer_measurement.set(Some(rect.h));
                }
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
        .spaced(Space::S8);
    if let Some(above) = above_composer {
        column = column.child(above);
    }
    Ok(column.child(card).build())
}

pub fn conversation_body(
    workspace: &crate::shell::workspace::Workspace,
    core: &crate::core::Core,
    theme: &ArtisanTheme,
    viewport_width: f32,
    body: Element,
    overlays: Vec<Element>,
    parts: ComposerParts,
    composer_measurement: std::rc::Rc<std::cell::Cell<Option<f32>>>,
    above_composer: Option<Element>,
) -> anyhow::Result<Element> {
    let card = if workspace.conversation.agent.is_some() {
        None
    } else {
        Some(measured_composer(
            workspace,
            core,
            theme,
            parts,
            composer_measurement,
            above_composer,
        )?)
    };
    Ok(crate::conversation::view::thread_frame(
        body,
        overlays,
        card,
        viewport_width,
        false,
    ))
}

pub fn centered_body(content: Element) -> Element {
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

pub fn conversation_edge_style(edge: crate::ui::scroll_edge::Edge) -> Style {
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

pub fn conversation_load_error(theme: &ArtisanTheme, text: String) -> Element {
    UiBox::new()
        .width(Length::Pct(100.0))
        .style(
            Style::new()
                .absolute_at(Some(0.0), Some(Space::S8.px()), Some(0.0), None)
                .justify(JustifyContent::CENTER),
        )
        .child(
            TextRole::Label
                .text(theme, text)
                .gpui_source_color(theme.colors.destructive_text.to_reference_text_color())
                .build(),
        )
        .build()
}

pub fn conversation_notice(theme: &ArtisanTheme, text: String, error: bool) -> Element {
    let color = if error {
        theme.colors.destructive_text
    } else {
        theme.colors.text_muted
    };
    centered_body(
        TextRole::Label
            .text(theme, text)
            .gpui_source_color(color.to_reference_text_color())
            .build(),
    )
}

pub fn new_conversation(
    workspace: &crate::shell::workspace::Workspace,
    core: &crate::core::Core,
    theme: &ArtisanTheme,
    parts: ComposerParts,
    composer_measurement: std::rc::Rc<std::cell::Cell<Option<f32>>>,
    reduced_motion: bool,
) -> anyhow::Result<Element> {
    let artwork = crate::ui::sky::artwork(theme, composer_measurement.clone(), reduced_motion);
    let card = measured_composer(workspace, core, theme, parts, composer_measurement, None)?;
    Ok(UiBox::new()
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .clip(true)
        .child(artwork)
        .child(centered_conversation(card))
        .build())
}

pub fn centered_conversation(content: Element) -> Element {
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

pub fn home(theme: &ArtisanTheme, forges: Element) -> Element {
    let content = UiBox::new()
        .layout("column")
        .spaced(Space::S8)
        .style(Style::new().align(AlignItems::CENTER))
        .child(TextRole::Heading.text(theme, "Choose a Forge").build())
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
