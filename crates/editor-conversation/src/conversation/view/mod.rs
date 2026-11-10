pub mod compaction;
pub mod factory;
pub mod motion;
pub mod navigator;
pub mod notice;
pub mod request;
pub mod resources;
pub mod reveal;
pub mod row;
pub mod row_cache;
pub mod row_sources;
pub mod source_actions;
pub mod status;
pub mod transcript;
pub mod work;

use crate::conversation::adapter::{Action, RowModel};
use crate::conversation::disclosure::RowDisclosure;
use crate::conversation::fold::Row;
use editor_ui::button::{Button, ButtonVariant};
use editor_ui::theme::ArtisanTheme;
use aqua_core::Insets;
use aqua_ui::element::{Box as UiBox, Element};
use aqua_ui::style::{Length, Style};
use aqua_ui::{AlignItems, JustifyContent};
use std::rc::Rc;

pub type Dispatch = Rc<dyn Fn(Action)>;

pub fn scoped_dispatch(
    conversation: &crate::conversation::controller::Conversation,
    emit: Rc<dyn Fn(crate::conversation::adapter::ScopedAction)>,
) -> Option<Dispatch> {
    let scope = crate::conversation::adapter::ActionScope::new(conversation)?;
    Some(Rc::new(move |action| emit(scope.bind(action))))
}

pub fn load_row(row: &Row, theme: ArtisanTheme, dispatch: Dispatch) -> Option<Element> {
    let (key, label, action) = match row {
        Row::EarlierWork { first, .. } => (
            format!("earlier-work-{first}"),
            "Load earlier work",
            Action::LoadEarlierWork(*first),
        ),
        Row::Gap => (
            "load-newer".to_owned(),
            "Load newer messages",
            Action::LoadNewer,
        ),
        _ => return None,
    };
    let button = Button::text(key, theme, label)
        .variant(ButtonVariant::Secondary)
        .on_activate(move |_| {
            dispatch(action.clone());
        })
        .build();
    Some(
        UiBox::new()
            .layout("row")
            .width(Length::Pct(100.0))
            .style(Style::new().justify(JustifyContent::CENTER))
            .child(button)
            .build(),
    )
}

pub fn row_frame(
    model: &RowModel,
    disclosure: RowDisclosure,
    held: bool,
    content: Option<Element>,
    theme: ArtisanTheme,
) -> Element {
    row_frame_with_body_observer(model, disclosure, held, content, theme, None)
}

fn row_frame_with_body_observer(
    model: &RowModel,
    disclosure: RowDisclosure,
    held: bool,
    content: Option<Element>,
    theme: ArtisanTheme,
    body_observer: Option<Rc<dyn Fn(aqua_core::Rect)>>,
) -> Element {
    let gap = disclosure.gap.unwrap_or(model.gap);
    let deferred = disclosure.clip.is_none() && disclosure.fraction.is_none() && disclosure.hidden;
    let chained = match model.row.semantic() {
        Row::WorkEntry { id, .. } => model
            .items
            .get(id)
            .is_some_and(|item| crate::conversation::activity::category(item).is_some()),
        Row::Text { slot, .. } => slot.is_card(),
        _ => false,
    };
    let content = content.map(|content| {
        if !chained {
            return content;
        }
        let rule = UiBox::new()
            .width(2.0)
            .fill(theme.colors.line.to_paint())
            .style(Style::new().absolute_at(
                Some(theme.spacing.steps(1.75)),
                Some(-gap),
                None,
                Some(0.0),
            ))
            .build();
        UiBox::new()
            .width(Length::Pct(100.0))
            .style(Style::new().padding_insets(Insets {
                left: theme.spacing.steps(6.0),
                ..Insets::default()
            }))
            .child(rule)
            .child(content)
            .build()
    });
    let mut column_style = Style::new()
        .max_width(Length::Px(768.0))
        .padding_insets(Insets {
            left: 24.0,
            right: 24.0,
            top: if content.is_some() && !held && !deferred {
                gap
            } else {
                0.0
            },
            bottom: if content.is_some() && matches!(model.row, Row::Status(_)) {
                theme.spacing.steps(2.0)
            } else {
                0.0
            },
        });
    column_style = column_style.align_self(aqua_ui::AlignSelf::CENTER);
    let mut column = UiBox::new().width(Length::Pct(100.0)).style(column_style);
    if let Some(content) = content {
        column = column.child(content);
    }
    if let Some(observer) = body_observer {
        column = column.child(
            aqua_ui::element::Canvas::new()
                .width(Length::Pct(100.0))
                .height(Length::Pct(100.0))
                .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
                .on_observe(move |tree, node| {
                    if let Some(bounds) = tree.pre_transform_rect(node) {
                        observer(bounds);
                    }
                })
                .build()
                .exclude_from_hit_testing(),
        );
    }
    let mut body = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .child(column.build());
    if let Some(clip) = disclosure.clip {
        body = body.height(clip.height).clip(true).opacity(clip.opacity);
    } else if held || deferred {
        body = body.height(0.0).clip(true);
    }
    UiBox::new()
        .width(Length::Pct(100.0))
        .style(Style::new().padding_insets(Insets {
            bottom: model.end,
            ..Insets::default()
        }))
        .child(body.build())
        .build()
}

pub fn measured_row_frame(
    model: &RowModel,
    disclosure: RowDisclosure,
    held: bool,
    content: Option<Element>,
    theme: ArtisanTheme,
    body_observer: Rc<dyn Fn(aqua_core::Rect)>,
    painted: Rc<dyn Fn(aqua_core::Rect)>,
) -> Element {
    let frame =
        row_frame_with_body_observer(model, disclosure, held, content, theme, Some(body_observer));
    let probe = aqua_ui::element::Canvas::new()
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
        .on_observe(move |tree, node| {
            if let Some(bounds) = tree.pre_transform_rect(node) {
                painted(bounds);
            }
        })
        .build()
        .exclude_from_hit_testing();
    UiBox::new()
        .width(Length::Pct(100.0))
        .child(frame)
        .child(probe)
        .build()
}

pub fn thread_frame(
    transcript: Element,
    overlays: Vec<Element>,
    composer: Option<Element>,
    viewport_width: f32,
    landing: bool,
) -> Element {
    let mut body = UiBox::new()
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0));
    let transcript = UiBox::new()
        .style(Style::new().absolute_at(Some(2.0), Some(2.0), Some(2.0), Some(2.0)))
        .clip(true)
        .opacity(if landing { 0.0 } else { 1.0 })
        .child(transcript)
        .build();
    body = body.child(transcript);
    for overlay in overlays {
        body = body.child(overlay);
    }
    let mut root = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .child(body.build());
    if let Some(composer) = composer {
        let bottom = if viewport_width >= 640.0 { 24.0 } else { 16.0 };
        let column = UiBox::new()
            .width(Length::Pct(100.0))
            .style(Style::new().max_width(Length::Px(768.0)))
            .child(composer)
            .build();
        let shell = UiBox::new()
            .layout("row")
            .style(
                Style::new()
                    .absolute_at(Some(0.0), None, Some(0.0), Some(bottom))
                    .padding_insets(Insets {
                        left: 24.0,
                        right: 24.0,
                        ..Insets::default()
                    })
                    .justify(JustifyContent::CENTER)
                    .align(AlignItems::CENTER),
            )
            .child(column)
            .build();
        root = root.child(shell);
    }
    root.build()
}

pub fn observed_row_frame(
    model: &RowModel,
    disclosure: RowDisclosure,
    held: bool,
    content: Option<Element>,
    theme: ArtisanTheme,
    observed: Rc<dyn Fn(aqua_core::Rect)>,
) -> Element {
    let frame = row_frame(model, disclosure, held, content, theme);
    let probe = aqua_ui::element::Canvas::new()
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
        .on_observe(move |tree, node| {
            if let Some(bounds) = tree.pre_transform_rect(node) {
                observed(bounds);
            }
        })
        .build()
        .exclude_from_hit_testing();
    UiBox::new()
        .width(Length::Pct(100.0))
        .child(frame)
        .child(probe)
        .build()
}

pub fn thread_element(
    conversation: &crate::conversation::controller::Conversation,
    estimated_height: f32,
    row: Rc<dyn Fn(usize, &crate::conversation::fold::RowKey) -> Element>,
    viewport: Rc<dyn Fn(aqua_ui::collection_runtime::CollectionViewport)>,
    initial_height: Rc<dyn Fn(usize, &crate::conversation::fold::RowKey) -> f32>,
    overlays: Vec<Element>,
    composer: Option<Element>,
    viewport_width: f32,
) -> Element {
    let list = transcript::list(
        conversation,
        estimated_height,
        row,
        viewport,
        initial_height,
    );
    thread_frame(
        list,
        overlays,
        composer,
        viewport_width,
        conversation.is_landing(),
    )
}

pub fn try_thread_element(
    conversation: &crate::conversation::controller::Conversation,
    estimated_height: f32,
    row: transcript::PreparedRowFactory,
    viewport: Rc<dyn Fn(aqua_ui::collection_runtime::CollectionViewport)>,
    initial_height: Rc<dyn Fn(usize, &crate::conversation::fold::RowKey) -> f32>,
    overlays: Vec<Element>,
    composer: Option<Element>,
    viewport_width: f32,
) -> Element {
    let list = transcript::try_list(
        conversation,
        estimated_height,
        row,
        viewport,
        initial_height,
    );
    thread_frame(
        list,
        overlays,
        composer,
        viewport_width,
        conversation.is_landing(),
    )
}

pub fn batched_thread_element(
    conversation: &crate::conversation::controller::Conversation,
    estimated_height: f32,
    row: transcript::BatchedRowFactory,
    viewport: Rc<dyn Fn(aqua_ui::collection_runtime::CollectionViewport)>,
    initial_height: Rc<dyn Fn(usize, &crate::conversation::fold::RowKey) -> f32>,
    overlays: Vec<Element>,
    composer: Option<Element>,
    viewport_width: f32,
) -> Element {
    let list = transcript::batched_list(
        conversation,
        estimated_height,
        row,
        viewport,
        initial_height,
    );
    thread_frame(
        list,
        overlays,
        composer,
        viewport_width,
        conversation.is_landing(),
    )
}

pub fn layout_batched_thread_element(
    conversation: &crate::conversation::controller::Conversation,
    estimated_height: f32,
    row: transcript::LayoutBatchedRowFactory,
    revision: transcript::RowRevision,
    viewport: Rc<dyn Fn(aqua_ui::collection_runtime::CollectionViewport)>,
    initial_height: Rc<dyn Fn(usize, &crate::conversation::fold::RowKey) -> f32>,
    overlays: Vec<Element>,
    composer: Option<Element>,
    viewport_width: f32,
) -> Element {
    let list = transcript::layout_batched_list(
        conversation,
        estimated_height,
        row,
        revision,
        viewport,
        initial_height,
    );
    thread_frame(
        list,
        overlays,
        composer,
        viewport_width,
        conversation.is_landing(),
    )
}
