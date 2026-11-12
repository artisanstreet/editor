use crate::shell::layout::{self, CARD_RADIUS, Size};
use crate::ui::theme::{ArtisanTheme, Elevation};
use aqua_core::{CornerRadii, Insets};
use aqua_text::{FontFamily, TextStyle};
use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Element, Text},
    style::{Length, Style},
};

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
    let close = crate::ui::button::Button::icon(
        "settings-close",
        theme,
        editor_assets::AssetId::TABLER_X,
        "Close settings",
    )
    .debug_selector("settings-close")
    .variant(crate::ui::button::ButtonVariant::Ghost)
    .muted()
    .on_activate(move |_| close())
    .build();
    let page = UiBox::new()
        .style(
            Style::new()
                .flex_grow(1.0)
                .min_width(Length::Px(0.0))
                .min_height(Length::Px(0.0))
                .corners(CornerRadii {
                    top_left: CARD_RADIUS,
                    ..CornerRadii::default()
                })
                .gpui_border(
                    Insets {
                        top: 1.0,
                        left: 1.0,
                        ..Insets::default()
                    },
                    theme.colors.line.to_reference_paint(),
                    false,
                )
                .gpui_background(aqua_core::GpuiBackground::Solid(
                    theme.colors.background.to_reference_paint(),
                ))
                .gpui_shadows(
                    theme
                        .elevation
                        .drops(Elevation::Raised)
                        .iter()
                        .copied()
                        .map(Into::into)
                        .collect(),
                ),
        )
        .clip(true)
        .child(page)
        .build();
    let inset = (layout::TITLEBAR_HEIGHT - theme.density.control_sm) / 2.0;
    let strip = UiBox::new()
        .height(layout::TITLEBAR_HEIGHT)
        .style(
            Style::new()
                .flex_shrink(0.0)
                .align(AlignItems::CENTER)
                .justify(JustifyContent::END)
                .padding_insets(Insets {
                    right: inset,
                    ..Insets::default()
                }),
        )
        .child(close)
        .build();
    UiBox::new()
        .layout("row")
        .width(Length::Pct(100.0))
        .height(dimensions.size.height)
        .radius(CARD_RADIUS)
        .clip(true)
        .style(
            Style::new()
                .gpui_background(aqua_core::GpuiBackground::Solid(
                    theme.colors.frame.to_reference_paint(),
                ))
                .gpui_border(
                    Insets::uniform(1.0),
                    theme.colors.line.to_reference_paint(),
                    false,
                ),
        )
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
                        .min_width(Length::Px(0.0))
                        .min_height(Length::Px(0.0)),
                )
                .child(strip)
                .child(page)
                .build(),
        )
        .build()
}

fn base_text(theme: &ArtisanTheme, size: f32) -> TextStyle {
    TextStyle {
        families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
        size_px: size,
        ..TextStyle::default()
    }
}

pub fn failure_notice(theme: &ArtisanTheme, text: String, dismiss: impl Fn() + 'static) -> Element {
    let button = crate::ui::button::Button::text("dismiss-failure", *theme, "Dismiss")
        .variant(crate::ui::button::ButtonVariant::Ghost)
        .on_activate(move |_| dismiss())
        .build();
    UiBox::new()
        .layout("row")
        .width(Length::Pct(100.0))
        .gap(8.0)
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .padding_insets(Insets {
                    left: 14.0,
                    right: 14.0,
                    top: 8.0,
                    bottom: 8.0,
                })
                .gpui_background(aqua_core::GpuiBackground::Solid(
                    theme.colors.surface.to_reference_paint(),
                ))
                .gpui_border(
                    Insets {
                        bottom: 1.0,
                        ..Insets::default()
                    },
                    theme.colors.line.to_reference_paint(),
                    false,
                )
                .gpui_text_color(theme.colors.text.to_reference_paint())
                .text_style(base_text(theme, 14.0)),
        )
        .child(
            UiBox::new()
                .style(Style::new().flex_grow(1.0))
                .child(Text::new(text).build())
                .build(),
        )
        .child(button)
        .build()
}

pub fn workspace(workspace_key: u64, theme: &ArtisanTheme, elements: WorkspaceElements) -> Element {
    let mut root = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .fill(theme.colors.frame.to_paint())
        .style(
            Style::new()
                .text_style(base_text(theme, 14.0))
                .gpui_text_color(theme.colors.text.to_reference_paint()),
        )
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

pub fn palette_dialog(
    theme: &ArtisanTheme,
    palette: &crate::shell::palette::Palette,
    query: Element,
    on_select: std::rc::Rc<dyn Fn(crate::core::Screen)>,
    on_dismiss: impl Fn(crate::ui::dialog::DialogDismissReason) + 'static,
) -> crate::ui::dialog::Dialog {
    let mut content = UiBox::new().layout("column").gap(4.0).child(query);
    if let Some(error) = &palette.search_error {
        content = content.child(Text::new(error.clone()).font_size(12.0).build());
    }
    for (index, entry) in palette.entries.iter().enumerate() {
        let screen = entry.screen.clone();
        let select = std::rc::Rc::clone(&on_select);
        content = content.child(
            crate::ui::action_row::ActionRow::new(index, *theme, entry.label.clone())
                .secondary(entry.detail.clone())
                .selected(index == palette.highlighted)
                .on_activate(move |_| select(screen.clone()))
                .build(),
        );
    }
    crate::ui::dialog::Dialog::new(
        "command-palette",
        *theme,
        "Search threads and commands",
        content.build(),
    )
    .on_dismiss(on_dismiss)
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
        .gap(8.0);
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
    let radius = crate::ui::chrome::card::RADIUS - 2.0;
    let (top, bottom, corners) = match edge {
        crate::ui::scroll_edge::Edge::Top => (
            Some(2.0),
            None,
            CornerRadii {
                top_left: radius,
                top_right: radius,
                ..CornerRadii::default()
            },
        ),
        crate::ui::scroll_edge::Edge::Bottom => (
            None,
            Some(2.0),
            CornerRadii {
                bottom_left: radius,
                bottom_right: radius,
                ..CornerRadii::default()
            },
        ),
    };
    Style::new()
        .width(Length::Auto)
        .absolute_at(Some(2.0), top, Some(2.0), bottom)
        .corners(corners)
}

pub fn conversation_load_error(theme: &ArtisanTheme, text: String) -> Element {
    UiBox::new()
        .width(Length::Pct(100.0))
        .style(
            Style::new()
                .absolute_at(Some(0.0), Some(8.0), Some(0.0), None)
                .justify(JustifyContent::CENTER),
        )
        .child(
            Text::new(text)
                .text_style(base_text(theme, theme.typography.control_text))
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
        Text::new(text)
            .text_style(base_text(theme, theme.typography.control_text))
            .gpui_color(color.to_reference_paint())
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
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .justify(JustifyContent::CENTER)
                .padding_insets(Insets {
                    left: 24.0,
                    right: 24.0,
                    ..Insets::default()
                }),
        )
        .child(
            UiBox::new()
                .width(Length::Pct(100.0))
                .style(Style::new().max_width(Length::Px(768.0)))
                .child(content)
                .build(),
        )
        .build()
}

pub fn home(theme: &ArtisanTheme, forges: Element) -> Element {
    let title = Text::new("Choose a Forge")
        .text_style(base_text(theme, 28.0))
        .gpui_color(theme.colors.text.to_reference_paint())
        .build();
    let content = UiBox::new()
        .layout("column")
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .padding_insets(Insets {
                    bottom: 32.0,
                    ..Insets::default()
                }),
        )
        .child(title)
        .child(
            UiBox::new()
                .width(Length::Pct(100.0))
                .style(
                    Style::new()
                        .max_width(Length::Px(560.0))
                        .margin_insets(Insets {
                            top: 8.0,
                            ..Insets::default()
                        }),
                )
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
